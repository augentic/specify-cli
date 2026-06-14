//! Library surface for the `vectis` WASI command tool.
//!
//! ## Carve-out from workspace standards
//!
//! This crate is a deliberate carve-out from the workspace's
//! `Render` / `emit` / `specify-error` discipline. It ships as a
//! self-contained `wasm32-wasip2` component distributed independently
//! of the `specify` runtime binary, so it owns its own error rendering and
//! exit-code shape. Future changes here MUST preserve that boundary
//! — do not pull in `specify-error`, `Render`, or the host
//! `output::emit` dispatcher; those couplings would re-attach the
//! tool to the host CLI's release cadence.
//!
//! `vectis` exposes six subcommands:
//!
//! - `validate` — schema + cross-artifact validation for tokens, assets,
//!   layout, composition, plus an `all` fan-out.
//! - `verify` — declared-vs-present platform shell verification
//!   (detect mode for plan-time, verify mode for build/lint).
//! - `infer` — deterministic component-identity clustering over the
//!   composition baseline (name-free cluster report; RFC-40 §B2).
//! - `materialize` — canonical-to-export asset conversion (`assets`
//!   target; RFC-46 Phase 2).
//! - `scaffold` — render-only Crux project scaffolds (core / iOS /
//!   Android shells).
//! - `schema` — print a tool-owned embedded schema to stdout (the tool-owned schema and catalog decisions D1).
//!
//! Each subcommand serialises its body directly; there is no shared
//! envelope wrapper.

mod embedded;
mod error;
pub mod infer;
pub mod materialize;
pub mod scaffold;
pub mod schema;
pub mod validate;
pub mod verify;

use clap::{Parser, Subcommand};
pub use error::{EXIT_FAILURE, VectisError};
use serde_json::Value;

/// Render a payload as pretty-printed JSON without a trailing newline.
///
/// # Panics
///
/// Panics only if `serde_json` cannot serialise the value, which is
/// impossible for any payload constructed by the subcommand renderers
/// (each carries fully-owned `String` / `bool` / `u64` / `Value`
/// fields).
#[must_use]
pub fn render_json(payload: &Value) -> String {
    serde_json::to_string_pretty(payload).expect("JSON serialise")
}

/// Top-level argument parser for the `vectis` binary.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "vectis",
    version,
    about = "Validate Vectis UI artifacts, verify platform shells, materialize design-system exports, infer shared components, render Crux project scaffolds, and retrieve tool-owned schemas.",
    long_about = "Vectis WASI command tool. Subcommands:\n  \
                  validate — validate Vectis UI artifacts (tokens, assets, layout, composition, all).\n  \
                  verify  — verify declared platform shells are present on disk (detect, verify).\n  \
                  infer   — cluster structurally-identical groups in the composition baseline (name-free report).\n  \
                  materialize — convert canonical assets into per-platform exports (assets).\n  \
                  scaffold — render Crux project scaffolds (core, ios, android).\n  \
                  schema  — print a tool-owned embedded schema to stdout."
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
    /// Verify declared platform shells are present on disk.
    Verify(verify::VerifyArgs),
    /// Cluster structurally-identical groups in the composition baseline.
    Infer(infer::InferArgs),
    /// Convert canonical assets into per-platform exports.
    #[command(subcommand)]
    Materialize(materialize::MaterializeCommand),
    /// Render Vectis Crux scaffolds.
    #[command(subcommand)]
    Scaffold(scaffold::ScaffoldCommand),
    /// Print a tool-owned embedded schema to stdout.
    Schema {
        /// Schema name (tokens, assets, composition).
        name: String,
    },
}

/// Dispatch a parsed `Args` to the matching subcommand and render the
/// JSON body plus exit code the binary should surface.
#[must_use]
pub fn run(args: &Args) -> (String, u8) {
    match &args.command {
        VectisCommand::Validate(v) => validate::render_json(validate::run(v)),
        VectisCommand::Verify(v) => verify::render_json(verify::run(v)),
        VectisCommand::Infer(v) => infer::render_json(infer::run(v)),
        VectisCommand::Materialize(m) => materialize::render_json(materialize::run(m)),
        VectisCommand::Scaffold(s) => scaffold::render_json(scaffold::run(s)),
        VectisCommand::Schema { name } => schema::run(name),
    }
}
