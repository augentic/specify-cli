//! `specify-contract` — standalone validator binary for the contracts
//! capability.
//!
//! ## Carve-out from workspace standards
//!
//! This crate is a deliberate carve-out from the workspace's
//! `Render` / `emit` / `specify-error` discipline. It builds a
//! self-contained `wasm32-wasip2` artifact distributed independently
//! of the `specify` binary, so it owns its own JSON envelope, exit-code
//! mapping, and error rendering rather than routing through the shared
//! CLI plumbing. Future changes here MUST preserve that boundary —
//! do not introduce a dependency on `specify-error`, `Render`, or the
//! `output::emit` dispatcher; those couplings would re-attach this
//! tool to the host CLI's release cadence.
//!
//! Wraps [`specify_validate::validate_baseline`] to surface
//! the contract Validation checks (`SemVer` `info.version`,
//! `info.x-specify-id` format, cross-project id uniqueness) as a
//! standalone executable that the contracts capability can shell out
//! to from skill runtimes.
//!
//! The validator functions stay in `specify-validate`; this binary is
//! deliberately a thin argument-parsing + JSON-rendering shell over
//! them.
//!
//! # Exit codes
//!
//! - `0` — success, no findings.
//! - `1` — validation findings present (semver / x-specify-id /
//!   cross-project uniqueness violations).
//! - `2` — validator failed to run (path missing, not a directory, …).
//!
//! This binary uses the conventional shell-friendly exit-code mapping
//! (`0` clean / `1` findings / `2` invocation error) so capability
//! skills can branch on the exit code without needing the broader
//! `Exit` taxonomy. The JSON body's `"exit-code"` field reflects the
//! same value.
//!
//! # JSON shape
//!
//! ```json
//! {
//!   "contracts-dir": "<baseline-dir>",
//!   "ok": true,
//!   "findings": [
//!     { "path": "...", "rule-id": "...", "detail": "..." }
//!   ],
//!   "exit-code": 0
//! }
//! ```
//!
//! Findings paths are rendered relative to the parent of
//! `<baseline-dir>` when that prefix matches, so paths are reported
//! relative to the project root.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use serde::Serialize;
use specify_validate::{ContractFinding, validate_baseline};

const EXIT_OK: u8 = 0;
const EXIT_FINDINGS: u8 = 1;
const EXIT_INVOCATION_ERROR: u8 = 2;

/// Arguments accepted by `specify-contract`.
#[derive(Parser, Debug)]
#[command(
    name = "specify-contract",
    version,
    about = "Standalone validator for the contracts capability — SemVer + info.x-specify-id + cross-project uniqueness checks.",
    long_about = "Walks <BASELINE_DIR> for top-level OpenAPI 3.1 / AsyncAPI 3.0 documents \
                  (root key `openapi:` or `asyncapi:`) and runs the contract Validation rules:\n\
                  \n  \
                  * contract.version-is-semver\n  \
                  * contract.id-format\n  \
                  * contract.id-unique\n\
                  \nFindings are emitted as JSON (default) or text. Exit codes:\n\
                  \n  \
                  0   no findings\n  \
                  1   findings present\n  \
                  2   validator could not run (path missing / not a directory)"
)]
struct Args {
    /// Path to the baseline directory containing contract artefacts
    /// (typically the project's `contracts/` directory).
    baseline_dir: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    format: OutputFormat,
}

/// Output format selector for `--format`.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum OutputFormat {
    /// Human-readable summary on stdout, finding lines on stderr.
    Text,
    /// Pretty-printed JSON envelope on stdout (default).
    Json,
}

fn main() -> ExitCode {
    let args = Args::parse();
    run(&args.baseline_dir, args.format)
}

fn run(baseline_dir: &std::path::Path, format: OutputFormat) -> ExitCode {
    if let Err(message) = baseline_directory_error(baseline_dir) {
        eprintln!("{message}");
        return ExitCode::from(EXIT_INVOCATION_ERROR);
    }

    let findings = validate_baseline(baseline_dir);
    let exit_code = if findings.is_empty() { EXIT_OK } else { EXIT_FINDINGS };

    match format {
        OutputFormat::Json => {
            let body = serialize_findings(baseline_dir, &findings, exit_code);
            println!("{body}");
        }
        OutputFormat::Text => {
            if findings.is_empty() {
                println!(
                    "PASS — every top-level contract under {} is well-formed",
                    baseline_dir.display()
                );
            } else {
                println!("FAIL — {} finding(s):", findings.len());
                for f in &findings {
                    eprintln!("  [{}] {}: {}", f.rule_id, f.path.display(), f.detail);
                }
            }
        }
    }

    ExitCode::from(exit_code)
}

/// Render findings as the canonical pretty-printed JSON body. Field
/// order is preserved (typed `Serialize` structs piped through
/// `serde_json::to_string_pretty`) so the byte sequence is
/// deterministic. Findings paths are emitted relative to
/// `baseline_dir.parent()` when that prefix matches; otherwise the raw
/// path is rendered.
fn serialize_findings(
    baseline_dir: &Path, findings: &[ContractFinding], exit_code: u8,
) -> String {
    let strip_root = baseline_dir.parent();
    let payload: Vec<FindingPayload> = findings
        .iter()
        .map(|f| {
            let rendered = strip_root
                .and_then(|root| f.path.strip_prefix(root).ok())
                .map_or_else(|| f.path.display().to_string(), |p| p.display().to_string());
            FindingPayload {
                path: rendered,
                rule_id: f.rule_id,
                detail: f.detail.clone(),
            }
        })
        .collect();

    let body = ValidateBody {
        contracts_dir: baseline_dir.display().to_string(),
        ok: findings.is_empty(),
        findings: payload,
        exit_code,
    };
    serde_json::to_string_pretty(&body).expect("body is JSON-safe")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateBody {
    contracts_dir: String,
    ok: bool,
    findings: Vec<FindingPayload>,
    exit_code: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FindingPayload {
    path: String,
    rule_id: &'static str,
    detail: String,
}

fn baseline_directory_error(baseline_dir: &std::path::Path) -> Result<(), String> {
    std::fs::read_dir(baseline_dir).map(|_| ()).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => {
            format!("error: baseline directory does not exist: {}", baseline_dir.display())
        }
        std::io::ErrorKind::NotADirectory => {
            format!("error: baseline path is not a directory: {}", baseline_dir.display())
        }
        _ => {
            format!("error: baseline directory is not readable: {}: {err}", baseline_dir.display())
        }
    })
}
