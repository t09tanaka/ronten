//! Implementation of `ronten validate-concerns`: reads a concerns JSON
//! document (from a file, or from stdin when the path is `-` or omitted),
//! parses it, and runs the same semantic validation `ronten review` runs at
//! startup — without needing a git repository or launching a session. Output
//! is always a single JSON object on stdout, machine-readable:
//!
//! - Valid: `{"valid": true}`, exit 0.
//! - Invalid: `{"valid": false, "errors": [{"code", "message", "concern_id"?}]}`,
//!   exit [`exitcode::INPUT`] (10) — the same code `ronten review` exits with
//!   for invalid concerns.
//!
//! `errors` uses the same [`ValidationError`] shape `mapping::validate_concerns`
//! produces, so a caller sees identical `code`/`concern_id` values whether the
//! failure was caught here or by `review`'s startup check (which formats the
//! same errors for a human instead — see `mapping::format_validation_errors`).

use crate::exitcode;
use crate::mapping::{validate_concerns, ValidationError};
use crate::model::ConcernsInput;
use crate::review::read_concerns_source;
use crate::termsafe::sanitize;
use serde_json::json;
use std::path::PathBuf;

/// Entry point for the `validate-concerns` subcommand. Returns the process
/// exit code.
pub fn run(file: Option<PathBuf>) -> u8 {
    // Match `review --concerns -`'s convention: an explicit `-` or an
    // omitted path both mean "read from stdin".
    let spec = match &file {
        Some(path) => path.to_string_lossy().into_owned(),
        None => "-".to_string(),
    };

    // `concern_id` is only meaningful for per-concern semantic failures;
    // read/parse failures happen before any concern is even reached.
    let raw = match read_concerns_source(&spec) {
        Ok(raw) => raw,
        Err(e) => {
            return print_invalid(&[ValidationError {
                code: "READ_FAILED".to_string(),
                message: format!("failed to read concerns from {spec}: {e}"),
                concern_id: None,
            }]);
        }
    };

    let input: ConcernsInput = match serde_json::from_str(&raw) {
        Ok(input) => input,
        Err(e) => {
            return print_invalid(&[ValidationError {
                code: "INVALID_JSON".to_string(),
                message: format!("invalid concerns JSON: {e}"),
                concern_id: None,
            }]);
        }
    };

    match validate_concerns(&input) {
        Ok(()) => {
            println!("{}", json!({"valid": true}));
            0
        }
        Err(errors) => print_invalid(&errors),
    }
}

fn print_invalid(errors: &[ValidationError]) -> u8 {
    let body = json!({
        "valid": false,
        "errors": errors,
    });
    // Defense in depth, same as `review`'s startup path: each message may
    // already be clean, but a raw control character in the input (a path, a
    // serde error snippet) must never reach stdout unescaped.
    println!(
        "{}",
        sanitize(&serde_json::to_string(&body).expect("value serializes to JSON"))
    );
    exitcode::INPUT
}
