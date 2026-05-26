//! `vectis schema` subcommand — print a tool-owned schema to stdout.

use crate::embedded::{ASSETS_SCHEMA_SOURCE, COMPOSITION_SCHEMA_SOURCE, TOKENS_SCHEMA_SOURCE};
use crate::render_json;

/// Known schema names and their embedded sources.
const SCHEMAS: &[(&str, &str)] = &[
    ("tokens", TOKENS_SCHEMA_SOURCE),
    ("assets", ASSETS_SCHEMA_SOURCE),
    ("composition", COMPOSITION_SCHEMA_SOURCE),
];

/// Emit the named schema as pretty-printed JSON, or return an error
/// envelope for unknown names.
///
/// Exit codes: `0` schema emitted, `2` unknown schema name.
#[must_use]
pub fn run(name: &str) -> (String, u8) {
    if let Some((_, source)) = SCHEMAS.iter().find(|(n, _)| *n == name) {
        (source.trim_end().to_string(), 0)
    } else {
        let known: Vec<&str> = SCHEMAS.iter().map(|(n, _)| *n).collect();
        let body = serde_json::json!({
            "error": "unknown-schema",
            "message": format!("unknown schema: {name:?} (known: {})", known.join(", ")),
            "exit-code": 2,
        });
        (render_json(&body), 2)
    }
}
