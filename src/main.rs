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
    /// A submit outcome was reached but writing `--out` failed. Takes
    /// precedence over the approve/request-changes decision code.
    pub const OUT_FAILED: u8 = 15;
    /// The server task ended unexpectedly before any review outcome was
    /// resolved (e.g. the accept loop errored out).
    pub const SERVER_FAILED: u8 = 16;
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
        Cli::Review(a) => tokio::runtime::Runtime::new()
            .expect("failed to start tokio runtime")
            .block_on(review::run(a)),
        Cli::Demo(a) => tokio::runtime::Runtime::new()
            .expect("failed to start tokio runtime")
            .block_on(demo::run(a)),
    }
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
