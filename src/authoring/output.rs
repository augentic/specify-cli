use std::path::Path;

use serde::Serialize;
use specify_authoring::error::ToolingError;
use specify_authoring::finding::{Finding, Location};
use specify_error::ValidationSummary;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckBody {
    pub status: CheckStatus,
    pub results: Vec<ValidationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
pub enum CheckStatus {
    Pass,
    Fail,
    Error,
}

pub fn render_text(result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
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

pub fn render_json(result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) {
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

fn format_location(framework_root: &Path, location: &Location) -> String {
    let path = location
        .path
        .strip_prefix(framework_root)
        .unwrap_or(&location.path)
        .display()
        .to_string()
        .replace('\\', "/");

    location.column.map_or_else(
        || format!("{path}:{}", location.line),
        |column| format!("{path}:{}:{column}", location.line),
    )
}
