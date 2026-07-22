//! Orchestration for `ronten demo`: build a session from the embedded
//! fixture diff/concerns (no git repo required) and drive it through the
//! same serve/select/report loop as `ronten review`.

use crate::exitcode;
use crate::gitdiff::parse_unified_diff;
use crate::mapping::{resolve_mapping, validate_concerns};
use crate::model::ConcernsInput;
use crate::review::serve_session;
use crate::server::new_token;
use crate::session::{DraftSlot, Phase, SessionState};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// Parsed CLI arguments for `ronten demo`.
#[derive(clap::Args, Debug)]
pub struct DemoArgs {
    /// Bind port (0 = OS-assigned)
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// Do not open the browser automatically
    #[arg(long)]
    pub no_open: bool,
}

const DEMO_DIFF: &str = include_str!("../fixtures/demo.diff");
const DEMO_CONCERNS: &str = include_str!("../fixtures/demo-concerns.json");

/// Entry point for the `demo` subcommand. Returns the process exit code.
pub async fn run(args: DemoArgs) -> u8 {
    let input: ConcernsInput =
        serde_json::from_str(DEMO_CONCERNS).expect("embedded demo concerns fixture is valid JSON");
    validate_concerns(&input).expect("embedded demo concerns fixture passes validation");

    let files = parse_unified_diff(DEMO_DIFF);
    let mapping = resolve_mapping(&files, &input)
        .expect("embedded demo fixture resolves within the default resource budget");
    // No git repo behind the demo: digests are still real, commit oids are
    // absent, and the submit-time HEAD re-check is skipped (repo_root: None).
    let snapshot = crate::snapshot::ReviewSnapshot::without_git("demo", &files, &input);
    let summary = input.summary.clone();
    let token = new_token();
    let (tx, rx) = tokio::sync::watch::channel(());
    let state = Arc::new(SessionState {
        title: "ronten demo".to_string(),
        summary,
        files,
        mapping,
        input,
        token: token.clone(),
        session_id: new_token(),
        snapshot,
        repo_root: None,
        started_at: chrono::Utc::now(),
        deadline_at: None,
        phase: Mutex::new(Phase::Reviewing(DraftSlot::default())),
        outcome_tx: tx,
    });

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

    serve_session(state, rx, listener, url, args.no_open, None, None).await
}
