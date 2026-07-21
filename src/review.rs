//! Orchestration for `ronten review`: resolve the repo and diff, build the
//! session state, bind a localhost server, and drive it to an outcome.

use crate::exitcode;
use crate::gitdiff::{
    compute_diff, current_branch, git_dirs, is_tracked, repo_root, worktree_status, GitError,
};
use crate::mapping::{resolve_mapping, validate_concerns};
use crate::model::{ConcernsInput, Decision};
use crate::server::{build_router, new_token, Outcome};
use crate::session::{DraftSlot, Phase, SessionState};
use crate::termsafe::sanitize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Parsed CLI arguments for `ronten review`.
#[derive(clap::Args, Debug)]
pub struct ReviewArgs {
    /// Base ref; the diff reviewed is `git diff <base>...HEAD`
    #[arg(long)]
    pub base: String,
    /// Path to concerns JSON; use `-` for stdin
    #[arg(long)]
    pub concerns: String,
    /// Also write the result JSON to this file
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Bind port (0 = OS-assigned)
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Do not open the browser automatically
    #[arg(long)]
    pub no_open: bool,
    /// Session display name (defaults to current branch name)
    #[arg(long)]
    pub title: Option<String>,
    /// Exit 3 if nothing is submitted within this duration (e.g. "30m")
    #[arg(long, value_parser = humantime::parse_duration)]
    pub timeout: Option<Duration>,
    /// What to do when the worktree is not clean (tracked changes, untracked
    /// files, or dirty submodules — none of which this review can show)
    #[arg(long, value_enum, default_value_t = DirtyPolicy::Error)]
    pub dirty_policy: DirtyPolicy,
}

/// Policy for a dirty worktree at review start. The default is `Error`: a
/// review that silently excludes uncommitted work — most dangerously a
/// brand-new file the agent forgot to `git add` — looks complete while part
/// of the change is reviewed nowhere.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum DirtyPolicy {
    /// Refuse to start (exit 17) until the worktree is clean.
    Error,
    /// Print what is excluded and start anyway.
    Warn,
    /// Start silently.
    Ignore,
}

/// Hard cap on the size of the concerns JSON input, to bound memory use
/// regardless of source (file or stdin).
pub const MAX_CONCERNS_BYTES: usize = 8 * 1024 * 1024;

/// Read concerns JSON from a file path or, when `spec` is `-`, from stdin.
///
/// Rejects input exceeding [`MAX_CONCERNS_BYTES`], regardless of source: both
/// the file and stdin paths bound the read itself (via `Read::take`) so an
/// unbounded stream or a huge file can't be read into memory in full before
/// the size is checked, and both read raw bytes and check the size *before*
/// attempting UTF-8 conversion — so an oversized input always reports as a
/// size error, never a UTF-8 error from a multibyte character split at the
/// `take` boundary.
pub(crate) fn read_concerns_source(spec: &str) -> std::io::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let limit = MAX_CONCERNS_BYTES as u64 + 1;
    if spec == "-" {
        std::io::stdin().take(limit).read_to_end(&mut buf)?;
    } else {
        std::fs::File::open(spec)?
            .take(limit)
            .read_to_end(&mut buf)?;
    }
    if buf.len() > MAX_CONCERNS_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("concerns input exceeds {MAX_CONCERNS_BYTES} bytes"),
        ));
    }
    String::from_utf8(buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// The absolute path `out` would resolve to once it exists, computed without
/// requiring `out` itself to exist: canonicalizes `out`'s parent directory
/// (falling back to the current directory when `out` is a bare file name)
/// and re-appends the file name. Returns `None` if the file name is missing
/// (e.g. `out` is `.` or `/`) or the parent can't be canonicalized (most
/// commonly: the parent directory doesn't exist).
fn out_prospective_abs(out: &Path) -> Option<PathBuf> {
    let name = out.file_name()?;
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let parent_canon = match parent {
        Some(parent) => std::fs::canonicalize(parent).ok()?,
        None => std::fs::canonicalize(std::env::current_dir().ok()?).ok()?,
    };
    Some(parent_canon.join(name))
}

/// The concerns input's path relative to the repo root, in the lexical,
/// `/`-separated form `git status` emits — the only form that can be
/// compared against status output without reopening the symlink-alias hole
/// described on [`drop_exempt`].
///
/// Deliberately never canonicalizes the concerns path's *final* component —
/// only its parent directory is resolved (same pattern as
/// [`out_prospective_abs`]), and the literal `file_name()` is re-appended.
/// Canonicalizing the leaf too would resolve a concerns path that is itself
/// a symlink to whatever it points at, so a gitignored `concerns.json ->
/// forgotten.rs` symlink would make this function return `forgotten.rs` as
/// the exempt path — hiding an unrelated untracked file that `git status`
/// only ever reports under the symlink's own name, never the target's.
/// Resolving only the parent still allows comparison against the
/// canonicalized repo root while leaving the leaf name exactly as `git
/// status` would report it.
///
/// Returns `None` — meaning "no exemption is possible" — when concerns come
/// from stdin (`concerns == "-"`, nothing on disk to compare), when the
/// concerns path has no file name, when the parent can't be canonicalized,
/// or when the resulting path falls outside the canonicalized repo root.
/// All of these are fail-closed: no exemption is applied and the dirty gate
/// is free to report the path dirty like any other.
fn concerns_repo_relative(concerns: &str, root: &Path) -> Option<String> {
    if concerns == "-" {
        return None;
    }
    let concerns_path = Path::new(concerns);
    let name = concerns_path.file_name()?;
    let parent = concerns_path.parent().filter(|p| !p.as_os_str().is_empty());
    let parent_canon = match parent {
        Some(parent) => std::fs::canonicalize(parent).ok()?,
        None => std::fs::canonicalize(std::env::current_dir().ok()?).ok()?,
    };
    let concerns_abs = parent_canon.join(name);
    let root_abs = std::fs::canonicalize(root).ok()?;
    let rel = concerns_abs.strip_prefix(&root_abs).ok()?;
    let rel = rel.to_str()?;
    Some(rel.replace(std::path::MAIN_SEPARATOR, "/"))
}

/// Checks-only preflight for `--out`, run before the dirty gate so a
/// rejection never depends on (or reports) worktree cleanliness. Does not
/// touch the filesystem beyond metadata reads — the actual reservation
/// happens later, after the dirty gate, via [`OutReservation::reserve`].
///
/// Rejects, in order:
/// 1. `out` resolves to the same file as `concerns` (skipped when concerns
///    comes from stdin, i.e. `concerns == "-"`).
/// 2. `out` resolves inside the repository's git directory (or, for a
///    worktree checkout, the shared common git directory).
/// 3. `out` resolves to a path that is tracked in the index (checked only
///    when `out` lexically falls under the repo root; a target outside the
///    repo can't be tracked by it, so the check is simply skipped there).
/// 4. `out` already exists — as a regular file, a directory, or a symlink.
///
/// A git-dir or tracked-file check that can't be answered (e.g. `git`
/// itself failed) is treated as a rejection: the safe default is not to
/// reserve a path whose safety can't be confirmed. A target whose
/// prospective absolute path can't be computed at all (most commonly: its
/// parent directory doesn't exist) is *not* rejected here — that surfaces
/// later, as an ordinary I/O error, when [`OutReservation::reserve`] tries to
/// create it.
fn preflight_out_checks(out: &Path, concerns_spec: &str, root: &Path) -> Result<(), String> {
    let out_abs = out_prospective_abs(out);

    // 1. Same file as --concerns.
    if concerns_spec != "-" {
        if let (Some(out_abs), Ok(concerns_abs)) =
            (out_abs.as_ref(), std::fs::canonicalize(concerns_spec))
        {
            if *out_abs == concerns_abs {
                return Err(format!(
                    "--out target {} is the same file as --concerns {concerns_spec}",
                    out.display()
                ));
            }
        }
    }

    if let Some(out_abs) = out_abs.as_ref() {
        // 2. Inside git's own directory.
        match git_dirs(root) {
            Ok(dirs) => {
                if dirs.iter().any(|d| out_abs.starts_with(d)) {
                    return Err(format!(
                        "--out target {} is inside the repository's git directory; choose a path outside .git",
                        out.display()
                    ));
                }
            }
            Err(GitError::GitFailed(msg)) => {
                return Err(format!(
                    "could not determine the repository's git directory: {}",
                    sanitize(msg.trim())
                ));
            }
            Err(_) => {}
        }

        // 3. Tracked in the index (only meaningful if out lexically falls
        // under the repo root).
        if let Ok(root_abs) = std::fs::canonicalize(root) {
            if let Ok(rel) = out_abs.strip_prefix(&root_abs) {
                if let Some(rel) = rel.to_str() {
                    let rel = rel.replace(std::path::MAIN_SEPARATOR, "/");
                    match is_tracked(root, &rel) {
                        Ok(true) => {
                            return Err(format!(
                                "--out target {} is a tracked file in this repository; choose an untracked path",
                                out.display()
                            ));
                        }
                        Ok(false) => {}
                        Err(GitError::GitFailed(msg)) => {
                            return Err(format!(
                                "could not check whether --out is tracked: {}",
                                sanitize(msg.trim())
                            ));
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }

    // 4. Already exists (file, directory, or symlink — checked without
    // following the symlink, so a dangling symlink is still rejected).
    match std::fs::symlink_metadata(out) {
        Ok(meta) if meta.is_dir() => {
            return Err(format!("--out target {} is a directory", out.display()));
        }
        Ok(_) => {
            return Err(format!(
                "--out target {} already exists; move or delete the previous result before re-running",
                out.display()
            ));
        }
        Err(_) => {}
    }

    Ok(())
}

/// An atomic no-clobber reservation on a `--out` target: an empty placeholder
/// file created with `O_CREAT|O_EXCL` (`create_new`), which is what actually
/// guarantees no-clobber — the preflight checks above narrow *why* a target
/// is unsafe, but only `create_new` closes the TOCTOU between "checked" and
/// "written".
///
/// Cleanup is RAII: unless [`disarm`](Self::disarm) is called (only on a
/// successful final write), dropping the reservation best-effort removes the
/// placeholder. Because `serve_session` owns the reservation for its entire
/// body, this covers every non-submit termination — abort, timeout, a
/// SIGINT, or an early-return error path — without each of those call sites
/// needing to know about `--out` at all.
pub(crate) struct OutReservation {
    path: PathBuf,
    armed: bool,
}

impl OutReservation {
    /// Creates the empty placeholder at `path`. The caller is expected to
    /// have already run [`preflight_out_checks`]; this only re-attempts the
    /// existence check atomically (via `create_new`) and surfaces any other
    /// I/O failure (e.g. a missing parent directory) as-is.
    fn reserve(path: PathBuf) -> std::io::Result<Self> {
        let mut open_options = std::fs::OpenOptions::new();
        open_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open_options.mode(0o600);
        }
        open_options.open(&path)?;
        Ok(Self { path, armed: true })
    }

    /// Marks the reservation as fulfilled: the placeholder has just been
    /// replaced (via `rename`) with the real output, so `Drop` must not
    /// delete it. Takes `self` by value so the disarmed drop happens right
    /// here rather than depending on a caller remembering to check a flag.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for OutReservation {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Drops the concerns input from `status.untracked` when it appears there
/// under an exact repo-relative path match — the only permissible dirty-gate
/// exemption. `tracked_changes` and `submodules_dirty` are never touched:
/// ronten only ever expects the concerns file to exist *untracked*, so a
/// tracked or dirty-submodule entry at that same path is a real uncommitted
/// change to review, not ronten's own input.
///
/// Comparison is plain string equality against `git status`'s own
/// repo-relative path (`concerns_rel`, from [`concerns_repo_relative`]) —
/// deliberately not a per-entry canonicalize. The previous implementation
/// canonicalized every status entry's path and compared it against the
/// canonicalized concerns path: a symlink aliasing a genuinely dirty tracked
/// file to the concerns argument would canonicalize to the same real file
/// and get silently dropped from `tracked_changes`, hiding a real
/// uncommitted change. Lexical string comparison against `untracked` only
/// closes that hole.
fn drop_exempt(
    mut status: crate::gitdiff::WorktreeStatus,
    concerns_rel: Option<&str>,
) -> crate::gitdiff::WorktreeStatus {
    if let Some(concerns_rel) = concerns_rel {
        status.untracked.retain(|p| p != concerns_rel);
    }
    status
}

/// Entry point for the `review` subcommand. Returns the process exit code.
pub async fn run(args: ReviewArgs) -> u8 {
    // 1. Resolve repo root.
    let root = match repo_root() {
        Ok(root) => root,
        Err(GitError::GitFailed(msg)) => {
            eprintln!("git failed: {}", sanitize(msg.trim()));
            return exitcode::GIT_FAILED;
        }
        Err(_) => {
            eprintln!("not a git repository (run ronten from inside a git worktree)");
            return exitcode::NOT_A_REPO;
        }
    };

    // 2. Read + parse + validate concerns.
    let raw = match read_concerns_source(&args.concerns) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("failed to read concerns from {}: {e}", args.concerns);
            return exitcode::INPUT;
        }
    };
    let input: ConcernsInput = match serde_json::from_str(&raw) {
        Ok(input) => input,
        Err(e) => {
            eprintln!("invalid concerns JSON: {}", sanitize(&e.to_string()));
            return exitcode::INPUT;
        }
    };
    if let Err(e) = validate_concerns(&input) {
        // `e` may already carry a sanitized `loc.path` token (see
        // `validate_concerns`), but the message is passed through `sanitize`
        // again regardless — it is cheap (a no-op scan) and makes this print
        // site safe on its own even if some future error string forgets to
        // sanitize a field itself.
        eprintln!("invalid concerns: {}", sanitize(&e));
        return exitcode::INPUT;
    }

    // 3. Compute the diff.
    let diff_output = match compute_diff(&root, &args.base) {
        Ok(output) => output,
        Err(GitError::BadBase(msg)) => {
            eprintln!("bad base ref {:?}: {}", args.base, sanitize(msg.trim()));
            return exitcode::BAD_BASE;
        }
        Err(GitError::GitFailed(msg)) => {
            eprintln!("git failed: {}", sanitize(msg.trim()));
            return exitcode::GIT_FAILED;
        }
        Err(GitError::NotARepo) => {
            eprintln!("not a git repository");
            return exitcode::NOT_A_REPO;
        }
        Err(GitError::BudgetExceeded(msg)) => {
            eprintln!("review too large: {msg}");
            return exitcode::REVIEW_TOO_LARGE;
        }
    };
    let files = diff_output.files;
    let diff_warnings = diff_output.warnings;
    // Pin the session to what was just diffed: the resolved endpoints plus
    // canonical digests of the diff and concerns input. Submit re-checks
    // HEAD against this snapshot, and the result JSON embeds it.
    let snapshot = crate::snapshot::ReviewSnapshot {
        base_ref: args.base.clone(),
        base_oid: Some(diff_output.base_oid),
        head_oid: Some(diff_output.head_oid),
        merge_base_oid: Some(diff_output.merge_base_oid),
        diff_sha256: crate::snapshot::diff_digest(&files),
        concerns_sha256: crate::snapshot::concerns_digest(&input),
    };

    // 3.5. `--out` preflight: checks only, no filesystem writes. Runs before
    // the dirty gate so a rejection is never entangled with (or shadowed by)
    // a dirty-worktree report. The actual reservation happens further below,
    // after the dirty gate, so the placeholder it creates never shows up as
    // an untracked file in that gate.
    if let Some(out) = &args.out {
        if let Err(e) = preflight_out_checks(out, &args.concerns, &root) {
            eprintln!("{e}");
            return exitcode::OUT_FAILED;
        }
    }

    // The diff above only ever covers `<base>...HEAD` (committed state); if
    // the agent forgot to commit some of its work — most dangerously a
    // brand-new file it never `git add`ed — those changes are reviewed
    // nowhere while the review looks complete. The dirty gate therefore
    // runs before *any* early return, including the empty-diff one below
    // (exactly the case where an agent committed nothing at all). The
    // concerns file is exempt only when it shows up untracked at exactly its
    // own repo-relative path — see [`drop_exempt`]. The `--out` destination
    // is not exempt at all: Task 1.1's preflight-then-reserve ordering means
    // `--out` never exists (tracked or untracked) at dirty-gate time, so no
    // exemption for it is needed.
    let dirty = match args.dirty_policy {
        DirtyPolicy::Ignore => None,
        DirtyPolicy::Error | DirtyPolicy::Warn => match worktree_status(&root) {
            Ok(status) => {
                let concerns_rel = concerns_repo_relative(&args.concerns, &root);
                let status = drop_exempt(status, concerns_rel.as_deref());
                (!status.is_clean()).then_some(status)
            }
            Err(GitError::BadBase(msg))
            | Err(GitError::GitFailed(msg))
            | Err(GitError::BudgetExceeded(msg)) => {
                // The gate cannot run at all. Under the (default) Error
                // policy an unverifiable worktree must not silently pass;
                // under Warn the review proceeds with a notice.
                if args.dirty_policy == DirtyPolicy::Error {
                    eprintln!(
                        "git status failed, cannot verify the worktree is clean: {}",
                        sanitize(msg.trim())
                    );
                    return exitcode::GIT_FAILED;
                }
                eprintln!(
                    "warning: git status failed ({}); could not verify the worktree is clean",
                    sanitize(msg.trim())
                );
                None
            }
            Err(GitError::NotARepo) => None,
        },
    };
    let print_dirty = |status: &crate::gitdiff::WorktreeStatus| {
        for path in &status.tracked_changes {
            eprintln!("  uncommitted change (tracked): {}", sanitize(path));
        }
        for path in &status.untracked {
            eprintln!("  untracked file: {}", sanitize(path));
        }
        for path in &status.submodules_dirty {
            eprintln!("  dirty submodule: {}", sanitize(path));
        }
    };
    if let Some(status) = &dirty {
        if args.dirty_policy == DirtyPolicy::Error {
            eprintln!(
                "worktree is not clean; this review would cover committed state only ({}...HEAD) and the following would be reviewed nowhere:",
                args.base
            );
            print_dirty(status);
            eprintln!("commit or stash them, or rerun with --dirty-policy warn to proceed anyway");
            return exitcode::DIRTY_WORKTREE;
        }
    }
    let dirty_warning = |status: &crate::gitdiff::WorktreeStatus| {
        eprintln!(
            "warning: worktree is not clean; this review covers committed state only ({}...HEAD) and the following are NOT included:",
            args.base
        );
        print_dirty(status);
    };

    if files.is_empty() {
        eprintln!("nothing to review: no diff between {} and HEAD", args.base);
        if let Some(status) = &dirty {
            dirty_warning(status);
        }
        return exitcode::EMPTY_DIFF;
    }

    if let Some(status) = &dirty {
        dirty_warning(status);
    }

    // 3.6. Reserve `--out` now that the review is definitely starting: an
    // empty placeholder created with `create_new` (O_EXCL), the atomic
    // no-clobber guarantee the checks above only prepared for. Deliberately
    // after the dirty gate above (so the placeholder itself is never flagged
    // as an untracked file) and after the empty-diff check below would have
    // returned (so a review that never starts doesn't leave one behind).
    let out_reservation = match &args.out {
        Some(path) => match OutReservation::reserve(path.clone()) {
            Ok(reservation) => Some(reservation),
            Err(e) => {
                eprintln!("failed to reserve --out target {}: {e}", path.display());
                return exitcode::OUT_FAILED;
            }
        },
        None => None,
    };

    // 4. Build the session state.
    let mut mapping = resolve_mapping(&files, &input);
    // Surface diff-level warnings (e.g. files too large to display) ahead
    // of mapping warnings.
    if !diff_warnings.is_empty() {
        let mut merged = diff_warnings;
        merged.append(&mut mapping.warnings);
        mapping.warnings = merged;
    }
    let title = args.title.clone().unwrap_or_else(|| current_branch(&root));
    let token = new_token();
    let summary = input.summary.clone();
    let (tx, rx) = watch::channel(());
    let state = Arc::new(SessionState {
        title,
        summary,
        files,
        mapping,
        input,
        token: token.clone(),
        session_id: new_token(),
        snapshot,
        repo_root: Some(root),
        started_at: chrono::Utc::now(),
        phase: Mutex::new(Phase::Reviewing(DraftSlot::default())),
        outcome_tx: tx,
    });

    // 5. Bind the listener (OS-assigned port when `port == 0`).
    let listener = match TcpListener::bind(("127.0.0.1", args.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("failed to bind 127.0.0.1:{}: {e}", args.port);
            return exitcode::INPUT;
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            eprintln!("failed to read local address: {e}");
            return exitcode::INPUT;
        }
    };
    let url = format!("http://127.0.0.1:{port}/r/{token}");

    serve_session(
        state,
        rx,
        listener,
        url,
        args.no_open,
        args.timeout,
        out_reservation,
    )
    .await
}

/// How long graceful shutdown may take after the outcome is known before the
/// server task is aborted outright. Bounds the tail of the process: a stalled
/// in-flight response (e.g. a client that stopped reading) can delay exit by
/// at most this long.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Waits for the next interrupt on the eagerly registered handler (unix);
/// elsewhere falls back to `ctrl_c()`, whose lazy registration is the best
/// available on that platform.
#[cfg(unix)]
async fn wait_interrupt(interrupt: &mut tokio::signal::unix::Signal) {
    interrupt.recv().await;
}

#[cfg(not(unix))]
async fn wait_interrupt(_interrupt: &mut ()) {
    let _ = tokio::signal::ctrl_c().await;
}

/// Serve a prepared session until it reaches an outcome, then print the result
/// and return the exit code. Shared by `review::run` and the demo command so
/// both drive an identical serve/select/report loop.
pub async fn serve_session(
    state: Arc<SessionState>,
    mut rx: watch::Receiver<()>,
    listener: TcpListener,
    url: String,
    no_open: bool,
    timeout: Option<Duration>,
    out: Option<OutReservation>,
) -> u8 {
    // Install the SIGINT handler BEFORE the banner: once the session URL is
    // visible, a ctrl-c must produce the clean abort exit (2). Without eager
    // registration there is a gap between printing the URL and the select!
    // loop's first poll in which a SIGINT hits the default disposition and
    // kills the process with no exit code at all (observed as a flaky
    // sigint test in CI). `signal()` registers the OS handler at call time,
    // so anything delivered after this line is queued, not fatal.
    #[cfg(unix)]
    let mut interrupt =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) {
            Ok(signal) => signal,
            Err(e) => {
                eprintln!("failed to install the SIGINT handler: {e}");
                return exitcode::SERVER_FAILED;
            }
        };
    #[cfg(not(unix))]
    let mut interrupt = ();

    eprintln!("Review session: {url}");
    let _ = std::io::stderr().flush();

    if !no_open {
        if let Err(e) = open::that(&url) {
            eprintln!("warning: could not open browser ({e}); open the URL above manually");
        }
    }

    // Serve with graceful shutdown driven by a Notify we fire once the outcome
    // is known, so in-flight responses (e.g. the submit ack) flush cleanly.
    let notify = Arc::new(tokio::sync::Notify::new());
    let shutdown = notify.clone();
    let router = build_router(state.clone());
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move { shutdown.notified().await });
    // Not fire-and-forget: this handle is also a `select!` branch below, so an
    // early server death is noticed instead of leaving the process hanging on
    // `rx.recv()` forever. axum 0.8's `serve` retries `accept()` errors
    // internally rather than returning them, so in practice the only way this
    // branch fires is the server task itself terminating unexpectedly (e.g. a
    // panic inside a handler unwinding the task). It is only ever awaited to
    // completion here or in the graceful-shutdown path after `outcome` is
    // known — `with_graceful_shutdown` means the server future does not
    // resolve on its own before `notify.notify_one()` is called, so this
    // branch winning the select is always unexpected.
    let mut server_handle = tokio::spawn(async move { server.await });

    // Every exit path races through the same atomic `try_finish`. If ctrl-c
    // or the deadline loses the race (a submit/abort handler already claimed
    // the terminal), the winner's outcome is authoritative and — because the
    // claim and the outcome publish are one atomic step — it is immediately
    // readable from the session phase. No path here can wait forever: the
    // watch channel is a pure wake-up, never the storage.
    let outcome = tokio::select! {
        // `biased` pins the outcome wake-up first, so an outcome that has
        // already been published always wins over a simultaneous server-task
        // death instead of `select!`'s default random pick occasionally
        // taking the server-death branch and misreporting SERVER_FAILED.
        biased;
        changed = rx.changed() => {
            match state.finished_outcome() {
                Some(outcome) => outcome,
                None => {
                    // Only reachable if the sender was dropped without a
                    // finish, which cannot happen while `state` is alive —
                    // but degrade to SERVER_FAILED rather than panic.
                    debug_assert!(changed.is_err());
                    eprintln!("outcome signal ended before a review outcome was recorded");
                    return exitcode::SERVER_FAILED;
                }
            }
        }
        _ = wait_interrupt(&mut interrupt) => {
            if state.try_finish(Outcome::Aborted) {
                Outcome::Aborted
            } else {
                // Lost the race to a submit/abort handler; its outcome is
                // already published (same lock as the claim), so this read
                // cannot block or miss.
                state.finished_outcome().unwrap_or(Outcome::Aborted)
            }
        }
        _ = async { tokio::time::sleep(timeout.unwrap()).await }, if timeout.is_some() => {
            if state.try_finish(Outcome::Timeout) {
                Outcome::Timeout
            } else {
                // Same losing-race read as the ctrl-c branch above.
                state.finished_outcome().unwrap_or(Outcome::Timeout)
            }
        }
        joined = &mut server_handle => {
            match joined {
                Ok(Ok(())) => eprintln!(
                    "server exited before a review outcome was recorded"
                ),
                Ok(Err(e)) => eprintln!("server error: {e}"),
                Err(e) => eprintln!("server task panicked: {e}"),
            }
            return exitcode::SERVER_FAILED;
        }
    };

    // Graceful shutdown with a hard deadline: in-flight responses (e.g. the
    // submit ack) get up to SHUTDOWN_GRACE to flush; after that — or on a
    // second ctrl-c — the server task is aborted so a stalled connection can
    // never hold the process open indefinitely.
    notify.notify_one();
    let finished_cleanly = tokio::select! {
        _ = &mut server_handle => true,
        _ = tokio::time::sleep(SHUTDOWN_GRACE) => false,
        _ = wait_interrupt(&mut interrupt) => false,
    };
    if !finished_cleanly {
        server_handle.abort();
        let _ = server_handle.await;
    }

    match outcome {
        Outcome::Submitted(result) => {
            let json = serde_json::to_string_pretty(&result).expect("result serializes to JSON");
            let decision_code = match result.decision {
                Decision::Approve => exitcode::APPROVED,
                Decision::RequestChanges => exitcode::REQUEST_CHANGES,
            };
            // `--out` is written before stdout: it is the durable,
            // machine-readable record, so it must be confirmed (or fail
            // loudly with OUT_FAILED) before the process risks losing the
            // result to a stdout error. stdout is still attempted
            // afterwards regardless of how this turns out, so a caller
            // piping stdout gets the JSON whenever the pipe is alive.
            let mut exit_code = decision_code;
            if let Some(reservation) = out {
                // The rename below replaces the reserved placeholder with
                // the real output; only on success does the reservation get
                // disarmed. On failure it stays armed, so it best-effort
                // removes the placeholder on drop rather than leaving an
                // empty file behind.
                match write_out_atomic(&reservation.path, &json) {
                    Ok(()) => reservation.disarm(),
                    Err(e) => {
                        eprintln!("failed to write {}: {e}", reservation.path.display());
                        exit_code = exitcode::OUT_FAILED;
                    }
                }
            }
            write_result_to_stdout(&json);
            exit_code
        }
        // Aborted and Timeout fall straight through to the implicit drop of
        // `out` at the end of this function: an armed `OutReservation`
        // best-effort removes its placeholder there, so these arms don't
        // need to touch `out` themselves.
        Outcome::Aborted => exitcode::ABORTED,
        Outcome::Timeout => exitcode::TIMEOUT,
    }
}

/// Writes the result JSON to stdout followed by a trailing newline, mirroring
/// `println!`'s framing without its panic-on-error behavior. A broken pipe
/// (the reader went away, e.g. `| head`) is expected and silently ignored:
/// the result is already durable in `--out` when that flag was given, and
/// there is no reader left to notice a stderr note either way. Any other
/// stdout error is unusual enough to surface on stderr. Either way this never
/// changes the process exit code — it is set by the decision (or by an
/// `--out` failure) before this runs.
fn write_result_to_stdout(json: &str) {
    let mut stdout = std::io::stdout();
    let result = stdout
        .write_all(json.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush());
    if let Err(e) = result {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!("failed to write result to stdout: {e}");
        }
    }
}

/// Writes `contents` to `path` atomically: writes to a sibling temp file
/// (`{filename}.tmp.{pid}.{n}`) in the same directory, flushes it to disk,
/// then renames it over `path`. This closes a race where a poller reading
/// `path` (e.g. an orchestrator watching for `--out` to appear) could
/// otherwise observe a partially-written file. The temp file is removed on a
/// best-effort basis if any step fails.
fn write_out_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--out path has no file name",
        )
    })?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let pid = std::process::id();

    // The temp name is predictable (pid + a small counter), so in a shared
    // writable directory another actor could plant a symlink at that exact
    // path ahead of time to redirect the write at some other file.
    // `create_new` (O_EXCL) refuses to open a path that already exists —
    // including a symlink — instead of following it the way `File::create`
    // would, closing that TOCTOU. The counter only exists to step past a
    // name collision with a leftover from a previous crash; a handful of
    // attempts is plenty since collisions here are expected to be rare.
    let mut attempt: u32 = 0;
    let (tmp_path, mut file) = loop {
        let tmp_name = format!("{}.tmp.{pid}.{attempt}", file_name.to_string_lossy());
        let tmp_path = match dir {
            Some(dir) => dir.join(&tmp_name),
            None => PathBuf::from(&tmp_name),
        };
        let mut open_options = std::fs::OpenOptions::new();
        open_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open_options.mode(0o600);
        }
        match open_options.open(&tmp_path) {
            Ok(f) => break (tmp_path, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 8 => {
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    };

    let write_result = (|| -> std::io::Result<()> {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file (not just stdin) exceeding `MAX_CONCERNS_BYTES` must be
    /// rejected with the size-cap message, not read to completion first —
    /// this exercises the `File::open(...).take(...)` bound directly rather
    /// than through the CLI (see `oversized_concerns_input_is_rejected` in
    /// `tests/review_flow.rs` for the end-to-end version).
    #[test]
    fn oversized_file_input_reports_size_error_not_utf8_error() {
        let dir = std::env::temp_dir().join(format!(
            "ronten-review-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge.json");

        // MAX_CONCERNS_BYTES + 1 bytes, all ASCII so a naive byte-count cap
        // and a char-boundary-respecting one would agree; the point here is
        // that the file path takes the same bytes-first-then-UTF-8 order as
        // stdin, so the error is the size message, never a UTF-8 decode error.
        let oversized = vec![b' '; MAX_CONCERNS_BYTES + 1];
        std::fs::write(&path, &oversized).unwrap();

        let err = read_concerns_source(path.to_str().unwrap()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("exceeds"),
            "expected a size-exceeded message, got: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
