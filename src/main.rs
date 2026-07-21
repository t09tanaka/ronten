use clap::Parser;

mod assets;
mod demo;
mod gitdiff;
mod mapping;
mod model;
mod review;
mod schema_cmd;
mod server;
mod session;
mod snapshot;

pub mod exitcode {
    pub const APPROVED: u8 = 0;
    pub const REQUEST_CHANGES: u8 = 1;
    pub const ABORTED: u8 = 2;
    pub const TIMEOUT: u8 = 3;
    /// Invalid usage or invalid concerns JSON.
    pub const INPUT: u8 = 10;
    pub const BAD_BASE: u8 = 11;
    pub const NOT_A_REPO: u8 = 12;
    pub const EMPTY_DIFF: u8 = 13;
    pub const GIT_FAILED: u8 = 14;
    /// `--out` could not be used, either rejected up front (the target
    /// already exists, is tracked by git, sits inside `.git`, is the same
    /// file as `--concerns`, or is a directory/symlink — checked, and the
    /// review never starts) or because the final write failed after a
    /// submit outcome was reached (in which case this takes precedence over
    /// the approve/request-changes decision code).
    pub const OUT_FAILED: u8 = 15;
    /// The server task ended unexpectedly before any review outcome was
    /// resolved (e.g. the accept loop errored out).
    pub const SERVER_FAILED: u8 = 16;
    /// The worktree has uncommitted/untracked changes and `--dirty-policy`
    /// is `error` (the default): part of the change would be reviewed
    /// nowhere, so the review refuses to start.
    pub const DIRTY_WORKTREE: u8 = 17;
    /// The diff exceeds a hard resource budget (e.g. file count); review it
    /// in smaller pieces.
    pub const REVIEW_TOO_LARGE: u8 = 18;
}

#[derive(Parser, Debug)]
#[command(name = "ronten", version, about = "Concern-based PR review viewer")]
enum Cli {
    /// Start a review session for `git diff <base>...HEAD`
    Review(review::ReviewArgs),
    /// Print JSON Schemas for the input/output contract
    Schema(SchemaArgs),
    /// Launch the UI with an embedded sample session (no git required)
    Demo(demo::DemoArgs),
}

#[derive(clap::Args, Debug)]
struct SchemaArgs {
    /// Print only the input (concerns) schema
    #[arg(long, conflicts_with = "output")]
    input: bool,
    /// Print only the output (result) schema
    #[arg(long)]
    output: bool,
}

fn run(cli: Cli) -> u8 {
    match cli {
        Cli::Schema(a) => schema_cmd::run(a.input, a.output),
        Cli::Review(a) => block_on_and_exit_promptly(review::run(a)),
        Cli::Demo(a) => block_on_and_exit_promptly(demo::run(a)),
    }
}

/// Runs `fut` to completion, then shuts the runtime down WITHOUT waiting for
/// leftover blocking tasks. Dropping the runtime normally blocks until every
/// `spawn_blocking` task finishes — so a wedged git subprocess (stalled
/// filesystem, misbehaving hook) inside the submit-time freshness check
/// could hold the process open indefinitely even after the outcome was
/// decided and printed. `shutdown_background` makes the bounded-exit
/// guarantee hold; a truly wedged git child is left to the OS to reap.
fn block_on_and_exit_promptly(fut: impl std::future::Future<Output = u8>) -> u8 {
    let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    let code = runtime.block_on(fut);
    runtime.shutdown_background();
    code
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // --help/--version are "errors" in clap terms but must exit 0
            if e.use_stderr() {
                eprintln!("{e}");
                std::process::exit(exitcode::INPUT as i32);
            }
            print!("{e}");
            std::process::exit(0);
        }
    };
    std::process::exit(run(cli) as i32);
}
