//! `specify-contract-validate` — standalone validator binary for the
//! contracts capability (RFC-13 §4.2a).
//!
//! Wraps [`specify_validate::validate_baseline_contracts`] and
//! [`specify_validate::serialize_contract_findings`] to surface the
//! RFC-12 §Validation checks (`SemVer` `info.version`,
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
//! Note that the legacy pre-Phase-2.7 `specify contract validate`
//! command surfaced validation failures as exit code `2` (Specify's
//! `CliResult::ValidationFailed`). This standalone binary uses the
//! conventional shell-friendly mapping (`0` clean / `1` findings /
//! `2` invocation error) so capability skills can branch on the exit
//! code without needing the broader `CliResult` taxonomy. The JSON
//! envelope's `"exit-code"` field reflects the same value.
//!
//! # JSON shape
//!
//! Identical to the legacy `specify contract validate --format json`
//! envelope:
//!
//! ```json
//! {
//!   "schema-version": 2,
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
//! `<baseline-dir>` when that prefix matches, mirroring the legacy
//! behaviour where paths were reported relative to the project root.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use specify_validate::{serialize_contract_findings, validate_baseline_contracts};

const EXIT_OK: u8 = 0;
const EXIT_FINDINGS: u8 = 1;
const EXIT_INVOCATION_ERROR: u8 = 2;

/// Arguments accepted by `specify-contract-validate`.
#[derive(Parser, Debug)]
#[command(
    name = "specify-contract-validate",
    version,
    about = "Standalone validator for the contracts capability — SemVer + info.x-specify-id + cross-project uniqueness checks (RFC-12 / RFC-13).",
    long_about = "Walks <BASELINE_DIR> for top-level OpenAPI 3.1 / AsyncAPI 3.0 documents \
                  (root key `openapi:` or `asyncapi:`) and runs the RFC-12 §Validation rules:\n\
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

    let findings = validate_baseline_contracts(baseline_dir);
    let exit_code = if findings.is_empty() { EXIT_OK } else { EXIT_FINDINGS };

    match format {
        OutputFormat::Json => {
            let envelope = serialize_contract_findings(baseline_dir, &findings, exit_code);
            println!("{envelope}");
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

fn baseline_directory_error(baseline_dir: &std::path::Path) -> Result<(), String> {
    std::fs::read_dir(baseline_dir)
        .map(|_| ())
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                format!("error: baseline directory does not exist: {}", baseline_dir.display())
            }
            std::io::ErrorKind::NotADirectory => {
                format!("error: baseline path is not a directory: {}", baseline_dir.display())
            }
            _ => format!("error: baseline directory is not readable: {}: {err}", baseline_dir.display()),
        })
}
