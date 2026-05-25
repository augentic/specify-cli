//! `specdev` binary entry point for framework authoring checks.

use std::path::Path;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use specify_authoring::check;
use specify_authoring::context::Context;
use specify_authoring::error::ToolingError;
use specify_authoring::exit::{Exit, exit_from_result};
use specify_authoring::finding::{Finding, Location};
use specify_error::ValidationSummary;

#[derive(Debug, Parser)]
#[command(
    name = "specdev",
    about = "Framework authoring checks for augentic/specify",
    version,
    after_help = "Common entry points:\n  specdev check --framework-root .\n  make check"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Output format. `text` by default; pass `--format json` (or set
    /// `SPECDEV_FORMAT=json`) for structured validation summaries.
    #[arg(long, env = "SPECDEV_FORMAT", default_value = "text", global = true)]
    format: Format,
}

#[derive(Debug, Copy, Clone, ValueEnum, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run framework consistency checks over a framework repo root.
    Check {
        /// Path to the augentic/specify framework repository.
        #[arg(long, env = "SPECDEV_FRAMEWORK_ROOT")]
        framework_root: std::path::PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Check { framework_root } => run_check(cli.format, framework_root),
    };
    process::exit(i32::from(code.code()));
}

fn run_check(format: Format, framework_root: std::path::PathBuf) -> Exit {
    let result = (|| -> Result<(std::path::PathBuf, Vec<Finding>), ToolingError> {
        let ctx = Context::from_framework_root(framework_root)?;
        let framework_root = ctx.framework_root().to_path_buf();
        Ok((framework_root, check::run(&ctx)))
    })();

    match format {
        Format::Text => render_text(&result),
        Format::Json => render_json(&result),
    }

    match result {
        Ok((_, findings)) => exit_from_result(Ok(()), findings.len()),
        Err(error) => exit_from_result(Err(error), 0),
    }
}

fn render_text(result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
    match result {
        Ok((_, findings)) if findings.is_empty() => {
            println!("All checks passed.");
        }
        Ok((framework_root, findings)) => {
            for finding in findings {
                eprintln!("FAIL: {}: {}", finding.rule_id, finding.message);
                if let Some(location) = &finding.location {
                    eprintln!("  at {}", format_location(framework_root, location));
                }
            }
            eprintln!("{} check failure(s).", findings.len());
        }
        Err(error) => eprintln!("error: {error}"),
    }
}

fn render_json(result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
    let body = match result {
        Ok((_, findings)) => CheckBody::from(findings.as_slice()),
        Err(error) => CheckBody {
            status: CheckStatus::Error,
            results: Vec::new(),
            error: Some(error.to_string()),
        },
    };
    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout().lock(), &body) {
        eprintln!("error: {error}");
    } else {
        println!();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckBody {
    status: CheckStatus,
    results: Vec<ValidationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl From<&[Finding]> for CheckBody {
    fn from(findings: &[Finding]) -> Self {
        Self {
            status: if findings.is_empty() { CheckStatus::Pass } else { CheckStatus::Fail },
            results: findings.iter().map(Finding::to_summary).collect(),
            error: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckStatus {
    Pass,
    Fail,
    Error,
}

fn format_location(framework_root: &Path, location: &Location) -> String {
    let path = location
        .path
        .strip_prefix(framework_root)
        .unwrap_or(&location.path)
        .display()
        .to_string()
        .replace('\\', "/");

    match location.column {
        Some(column) => format!("{path}:{}:{column}", location.line),
        None => format!("{path}:{}", location.line),
    }
}
