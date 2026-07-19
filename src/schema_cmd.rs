//! Implementation of `ronten schema`: prints the JSON Schemas for the
//! concerns input contract and/or the result output contract to stdout.

use crate::model::{ConcernsInput, ResultOutput};
use schemars::schema_for;
use serde_json::json;

/// Print the requested JSON Schema(s) as pretty-printed JSON to stdout.
///
/// - Both flags false: prints `{"input": <schema>, "output": <schema>}`.
/// - `input_only`: prints the `ConcernsInput` schema alone.
/// - `output_only`: prints the `ResultOutput` schema alone.
///
/// stdout is machine-readable only; always returns 0.
pub fn run(input_only: bool, output_only: bool) -> u8 {
    let value = if input_only {
        serde_json::to_value(schema_for!(ConcernsInput)).expect("schema serializes to JSON")
    } else if output_only {
        serde_json::to_value(schema_for!(ResultOutput)).expect("schema serializes to JSON")
    } else {
        json!({
            "input": schema_for!(ConcernsInput),
            "output": schema_for!(ResultOutput),
        })
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("value serializes to JSON")
    );
    0
}
