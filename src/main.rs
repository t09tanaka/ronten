use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

mod gitdiff;
mod mapping;
mod model;
mod schema_cmd;

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
}

#[derive(Parser, Debug)]
#[command(name = "ronten", version, about = "Concern-based PR review viewer")]
enum Cli {
    /// Start a review session for `git diff <base>...HEAD`
    Review(ReviewArgs),
    /// Print JSON Schemas for the input/output contract
    Schema(SchemaArgs),
    /// Launch the UI with an embedded sample session (no git required)
    Demo(DemoArgs),
}

#[derive(clap::Args, Debug)]
struct ReviewArgs {
    /// Base ref; the diff reviewed is `git diff <base>...HEAD`
    #[arg(long)]
    base: String,
    /// Path to concerns JSON; use `-` for stdin
    #[arg(long)]
    concerns: String,
    /// Also write the result JSON to this file
    #[arg(long)]
    out: Option<PathBuf>,
    /// Bind port (0 = OS-assigned)
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Do not open the browser automatically
    #[arg(long)]
    no_open: bool,
    /// Session display name (defaults to current branch name)
    #[arg(long)]
    title: Option<String>,
    /// Exit 3 if nothing is submitted within this duration (e.g. "30m")
    #[arg(long, value_parser = humantime::parse_duration)]
    timeout: Option<Duration>,
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

#[derive(clap::Args, Debug)]
struct DemoArgs {
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long)]
    no_open: bool,
}

fn run(cli: Cli) -> u8 {
    match cli {
        Cli::Schema(a) => schema_cmd::run(a.input, a.output),
        Cli::Review(_) | Cli::Demo(_) => {
            eprintln!("not implemented yet");
            exitcode::INPUT
        }
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
