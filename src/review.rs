//! Orchestration for `ronten review`: resolve the repo and diff, build the
//! session state, bind a localhost server, and drive it to an outcome.

use crate::exitcode;
use crate::gitdiff::{compute_diff, current_branch, repo_root, worktree_status, GitError};
use crate::mapping::{resolve_mapping, validate_concerns};
use crate::model::{ConcernsInput, Decision};
use crate::server::{build_router, new_token, Outcome};
use crate::session::{DraftSlot, Phase, SessionState};
use std::io::{Read, Write};
use std::path::PathBuf;
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

/// Absolute, symlink-resolved forms of the paths ronten itself expects to
/// see in the worktree — the concerns file (unless read from stdin) and the
/// `--out` destination — so the dirty gate doesn't flag ronten's own inputs.
fn exempt_paths(concerns: &str, out: Option<&std::path::Path>) -> Vec<PathBuf> {
    let mut exempt = Vec::new();
    if concerns != "-" {
        if let Ok(p) = std::fs::canonicalize(concerns) {
            exempt.push(p);
        }
    }
    if let Some(out) = out {
        // `--out` usually doesn't exist yet, so canonicalize its parent and
        // re-append the file name (covers a leftover result from a previous
        // run sitting untracked in the worktree).
        if let (Some(parent), Some(name)) = (
            out.parent().filter(|p| !p.as_os_str().is_empty()),
            out.file_name(),
        ) {
            if let Ok(parent) = std::fs::canonicalize(parent) {
                exempt.push(parent.join(name));
            }
        } else if let (Ok(cwd), Some(name)) = (std::env::current_dir(), out.file_name()) {
            if let Ok(cwd) = std::fs::canonicalize(cwd) {
                exempt.push(cwd.join(name));
            }
        }
    }
    exempt
}

/// Drops status entries whose absolute path is in `exempt`. Entries that
/// cannot be resolved (e.g. a deleted tracked file) are kept: an
/// unresolvable path is a reason to show the entry, not to hide it.
fn drop_exempt(
    mut status: crate::gitdiff::WorktreeStatus,
    root: &std::path::Path,
    exempt: &[PathBuf],
) -> crate::gitdiff::WorktreeStatus {
    let keep = |path: &String| match std::fs::canonicalize(root.join(path)) {
        Ok(abs) => !exempt.iter().any(|e| e == &abs),
        Err(_) => true,
    };
    status.tracked_changes.retain(keep);
    status.untracked.retain(keep);
    status.submodules_dirty.retain(keep);
    status
}

/// Entry point for the `review` subcommand. Returns the process exit code.
pub async fn run(args: ReviewArgs) -> u8 {
    // 1. Resolve repo root.
    let root = match repo_root() {
        Ok(root) => root,
        Err(GitError::GitFailed(msg)) => {
            eprintln!("git failed: {}", msg.trim());
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
            eprintln!("invalid concerns JSON: {e}");
            return exitcode::INPUT;
        }
    };
    if let Err(e) = validate_concerns(&input) {
        eprintln!("invalid concerns: {e}");
        return exitcode::INPUT;
    }

    // 3. Compute the diff.
    let diff_output = match compute_diff(&root, &args.base) {
        Ok(output) => output,
        Err(GitError::BadBase(msg)) => {
            eprintln!("bad base ref {:?}: {}", args.base, msg.trim());
            return exitcode::BAD_BASE;
        }
        Err(GitError::GitFailed(msg)) => {
            eprintln!("git failed: {}", msg.trim());
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

    // The diff above only ever covers `<base>...HEAD` (committed state); if
    // the agent forgot to commit some of its work — most dangerously a
    // brand-new file it never `git add`ed — those changes are reviewed
    // nowhere while the review looks complete. The dirty gate therefore
    // runs before *any* early return, including the empty-diff one below
    // (exactly the case where an agent committed nothing at all). The
    // concerns file and the `--out` destination are exempt: ronten itself
    // asks for them to exist untracked in the worktree.
    let dirty = match args.dirty_policy {
        DirtyPolicy::Ignore => None,
        DirtyPolicy::Error | DirtyPolicy::Warn => match worktree_status(&root) {
            Ok(status) => {
                let exempt = exempt_paths(&args.concerns, args.out.as_deref());
                let status = drop_exempt(status, &root, &exempt);
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
                        msg.trim()
                    );
                    return exitcode::GIT_FAILED;
                }
                eprintln!(
                    "warning: git status failed ({}); could not verify the worktree is clean",
                    msg.trim()
                );
                None
            }
            Err(GitError::NotARepo) => None,
        },
    };
    let print_dirty = |status: &crate::gitdiff::WorktreeStatus| {
        for path in &status.tracked_changes {
            eprintln!("  uncommitted change (tracked): {path}");
        }
        for path in &status.untracked {
            eprintln!("  untracked file: {path}");
        }
        for path in &status.submodules_dirty {
            eprintln!("  dirty submodule: {path}");
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
        args.out,
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
    out: Option<PathBuf>,
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
            println!("{json}");
            let decision_code = match result.decision {
                Decision::Approve => exitcode::APPROVED,
                Decision::RequestChanges => exitcode::REQUEST_CHANGES,
            };
            if let Some(path) = out {
                if let Err(e) = write_out_atomic(&path, &json) {
                    eprintln!("failed to write {}: {e}", path.display());
                    return exitcode::OUT_FAILED;
                }
            }
            decision_code
        }
        Outcome::Aborted => exitcode::ABORTED,
        Outcome::Timeout => exitcode::TIMEOUT,
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
