//! Library surface for the `vectis` WASI command tool.
//!
//! `vectis` exposes two subcommands:
//!
//! - `validate` — schema + cross-artifact validation for tokens, assets,
//!   layout, composition, plus an `all` fan-out.
//! - `scaffold` — render-only Crux project scaffolds (core / iOS /
//!   Android shells).
//!
//! Each subcommand owns its own JSON envelope shape; the shared
//! `schema-version: 2` framing lives in this crate so both halves stay
//! byte-compatible with their pre-merge dispatchers.

pub mod scaffold;
pub mod validate;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

/// JSON contract version emitted on every structured response.
pub const JSON_SCHEMA_VERSION: u64 = 2;

/// Wire shape for every structured response: the schema-version envelope
/// plus a flattened payload supplied by the dispatching subcommand.
#[derive(Serialize)]
struct Envelope {
    #[serde(rename = "schema-version")]
    schema_version: u64,
    #[serde(flatten)]
    payload: Value,
}

/// Top-level argument parser for the `vectis` binary.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "vectis",
    version,
    about = "Validate Vectis UI artifacts and render Crux project scaffolds.",
    long_about = "Vectis WASI command tool. Subcommands:\n  \
                  validate — validate Vectis UI artifacts (tokens, assets, layout, composition, all).\n  \
                  scaffold — render Crux project scaffolds (core, ios, android)."
)]
pub struct Args {
    /// Subcommand to dispatch.
    #[command(subcommand)]
    pub command: VectisCommand,
}

/// Top-level subcommand variants.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum VectisCommand {
    /// Validate Vectis UI artifacts.
    Validate(validate::ValidateArgs),
    /// Render Vectis Crux scaffolds.
    #[command(subcommand)]
    Scaffold(scaffold::ScaffoldCommand),
}

/// Render the v2 JSON envelope for a fully-formed payload.
///
/// # Panics
///
/// Panics only if `serde_json` cannot serialise the envelope, which is
/// impossible for the `Envelope` shape (a `u64` plus an already-parsed
/// `serde_json::Value`).
#[must_use]
pub fn envelope_json(payload: Value) -> String {
    serde_json::to_string_pretty(&Envelope {
        schema_version: JSON_SCHEMA_VERSION,
        payload,
    })
    .expect("JSON serialise")
}

/// Dispatch a parsed `Args` to the matching subcommand and render the
/// JSON envelope plus exit code the binary should surface.
#[must_use]
pub fn run(args: &Args) -> (String, u8) {
    match &args.command {
        VectisCommand::Validate(v) => validate::render_envelope_json(validate::run(v)),
        VectisCommand::Scaffold(s) => scaffold::render_envelope_json(scaffold::run(s)),
    }
}
