//! `marketplace` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the marketplace drift check is whole-tree) and reads
//! `PROJECT_DIR` from the environment. The positional path argument names
//! the rule's own sentinel file (`…/CORE-022-…md`); the tool walks the
//! tree itself and carries no policy.
//!
//! Findings are emitted on stdout as a `DiagnosticReport` envelope the
//! host folds into its scan output; each carries its own
//! `rule-id: CORE-022` and `severity: important`. The host restamps `id`
//! and `fingerprint`. Exit is always `0` on a successful run: the host
//! treats a non-zero exit with no parsed findings as an invocation
//! failure, so a clean tree must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use specify_marketplace::{MarketplaceFinding, check_marketplace_drift};

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let findings = check_marketplace_drift(&project_dir);
    print_report(&findings);
    ExitCode::SUCCESS
}

fn print_report(findings: &[MarketplaceFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("marketplace: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[MarketplaceFinding]) -> Self {
        let wire: Vec<Finding> = findings.iter().enumerate().map(Finding::from_indexed).collect();
        Self {
            version: 1,
            summary: Summary {
                critical: 0,
                important: u32::try_from(wire.len()).unwrap_or(u32::MAX),
                suggestion: 0,
                optional: 0,
            },
            findings: wire,
        }
    }
}

#[derive(Serialize)]
struct Summary {
    critical: u32,
    important: u32,
    suggestion: u32,
    optional: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Finding {
    id: String,
    rule_id: String,
    title: String,
    severity: String,
    source: String,
    artifact: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
    evidence: Evidence,
    impact: String,
    remediation: String,
    fingerprint: String,
}

impl Finding {
    fn from_indexed((index, finding): (usize, &MarketplaceFinding)) -> Self {
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: finding.rule_id.to_string(),
            title: finding.message.clone(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location: finding.path.clone().map(|path| Location { path }),
            evidence: Evidence::Snippet { value: finding.message.clone() },
            impact: "The marketplace manifest disagrees with the on-disk plugin layout, so the plugin set advertised to Cursor is wrong.".to_string(),
            remediation: "Reconcile .cursor-plugin/marketplace.json with the plugins/ tree: declare every on-disk plugin and ensure each declared plugin has skills/ and a plugin.json.".to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
    }
}

#[derive(Serialize)]
struct Location {
    path: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Evidence {
    Snippet { value: String },
}
