//! Writes an assembled workflow document to stdout.

use serde_json::Value;

/// Serializes `workflow` to compact JSON and writes it as exactly one line
/// on stdout -- the shape `loopt gen`'s YAML backend reads back from a
/// generated authoring program's output.
pub fn print(workflow: &Value) {
    let rendered = serde_json::to_string(workflow).expect("serialize workflow to JSON");
    println!("{rendered}");
}
