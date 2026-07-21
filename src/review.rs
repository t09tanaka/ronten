//! Orchestration for `ronten review`: resolve the repo and diff, build the
//! session state, bind a localhost server, and drive it to an outcome.

use crate::exitcode;
use crate::gitdiff::{compute_diff, current_branch, repo_root, GitError};
use crate::mapping::{resolve_mapping, validate_concerns};
use crate::model::{ConcernsInput, Decision};
use crate::server::{build_router, new_token, Outcome};
use crate::session::{Draft, SessionState, Terminal};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;

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
}

/// Hard cap on the size of the concerns JSON input, to bound memory use
/// regardless of source (file or stdin).
pub const MAX_CONCERNS_BYTES: usize = 8 * 1024 * 1024;

/// Read concerns JSON from a file path or, when `spec` is `-`, from stdin.
///
/// Rejects input exceeding [`MAX_CONCERNS_BYTES`]. The stdin path bounds the
/// read itself (via `Read::take`) so an unbounded stream can't be read into
/// memory in full before the size is checked.
fn read_concerns_source(spec: &str) -> std::io::Result<String> {
    let raw = if spec == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .take(MAX_CONCERNS_BYTES as u64 + 1)
            .read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(spec)?
    };
    if raw.len() > MAX_CONCERNS_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("concerns input exceeds {MAX_CONCERNS_BYTES} bytes"),
        ));
    }
    Ok(raw)
}

/// Entry point for the `review` subcommand. Returns the process exit code.
pub async fn run(args: ReviewArgs) -> u8 {
    // 1. Resolve repo root.
    let root = match repo_root() {
        Ok(root) => root,
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
    };
    let files = diff_output.files;
    let diff_warnings = diff_output.warnings;
    if files.is_empty() {
        eprintln!("nothing to review: no diff between {} and HEAD", args.base);
        return exitcode::EMPTY_DIFF;
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
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let state = Arc::new(SessionState {
        title,
        summary,
        files,
        mapping,
        input,
        token: token.clone(),
        started_at: chrono::Utc::now(),
        draft: Mutex::new(Draft::default()),
        finished: Mutex::new(None),
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

/// Serve a prepared session until it reaches an outcome, then print the result
/// and return the exit code. Shared by `review::run` and the demo command so
/// both drive an identical serve/select/report loop.
pub async fn serve_session(
    state: Arc<SessionState>,
    mut rx: Receiver<Outcome>,
    listener: TcpListener,
    url: String,
    no_open: bool,
    timeout: Option<Duration>,
    out: Option<PathBuf>,
) -> u8 {
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
    let server_handle = tokio::spawn(async move {
        let _ = server.await;
    });

    // Every exit path races through the same compare-and-set terminal state.
    // If ctrl-c or the deadline loses the race (a submit/abort handler
    // already claimed the terminal), the handler's outcome is authoritative
    // and is already in flight on `rx` — HTTP 200 must never coexist with a
    // timeout/abort exit.
    let outcome = tokio::select! {
        o = rx.recv() => o.expect("outcome channel closed before an outcome was sent"),
        _ = tokio::signal::ctrl_c() => {
            if state.try_finish(Terminal::Aborted) {
                Outcome::Aborted
            } else {
                rx.recv().await.expect("outcome channel closed before an outcome was sent")
            }
        }
        _ = async { tokio::time::sleep(timeout.unwrap()).await }, if timeout.is_some() => {
            if state.try_finish(Terminal::TimedOut) {
                Outcome::Timeout
            } else {
                rx.recv().await.expect("outcome channel closed before an outcome was sent")
            }
        }
    };

    notify.notify_one();
    let _ = server_handle.await;

    match outcome {
        Outcome::Submitted(result) => {
            let json = serde_json::to_string_pretty(&result).expect("result serializes to JSON");
            println!("{json}");
            if let Some(path) = out {
                if let Err(e) = std::fs::write(&path, &json) {
                    eprintln!("warning: failed to write {}: {e}", path.display());
                }
            }
            match result.decision {
                Decision::Approve => exitcode::APPROVED,
                _ => exitcode::REQUEST_CHANGES,
            }
        }
        Outcome::Aborted => exitcode::ABORTED,
        Outcome::Timeout => exitcode::TIMEOUT,
    }
}
