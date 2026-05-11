//! Cross-project contract compatibility classification.
//!
//! The classifier compares producer contracts in the current project
//! baseline with consumer views materialised under `.specify/workspace/`.
//! It is deliberately read-only and deterministic: callers provide a
//! project root, the module reads `registry.yaml`, `contracts/`, and
//! workspace clones, then returns a classified report.

use std::path::Path;

use serde::Serialize;
use specify_error::Error;

use crate::registry::{Registry, RegistryProject};
use crate::validate::contracts::validate_baseline;

mod asyncapi_diff;
mod openapi_diff;
mod pair;
mod schema_diff;
mod util;

use pair::{PairContext, diff_pair_contracts};

const KIND_REMOVED_FIELD: &str = "removed-field";
const KIND_REQUIRED_FIELD_ADDED: &str = "required-field-added";
const KIND_TYPE_NARROWED: &str = "type-narrowed";
const KIND_ENUM_VALUE_REMOVED: &str = "enum-value-removed";
const KIND_ADDITIONAL_PROPERTIES_TIGHTENED: &str = "additional-properties-tightened";
const KIND_REMOVED_ENDPOINT: &str = "removed-endpoint";
const KIND_STATUS_CODE_REMOVED: &str = "status-code-removed";
const KIND_REMOVED_CHANNEL: &str = "removed-channel";
const KIND_REMOVED_OPERATION: &str = "removed-operation";

crate::kebab_enum! {
    /// Compatibility buckets for a producer-to-consumer contract delta.
    #[derive(Debug)]
    pub enum CompatibilityClassification {
        /// The delta is backwards-compatible for the consumer.
        Additive => "additive",
        /// The delta is a recognized backwards-incompatible wire change.
        Breaking => "breaking",
        /// The delta changed behaviorally, but the classifier cannot prove
        /// whether it is safe.
        Ambiguous => "ambiguous",
        /// The classifier could not compare the producer and consumer views.
        Unverifiable => "unverifiable",
    }
}

/// One compatibility finding for a producer / consumer contract pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilityFinding {
    /// Classification bucket.
    pub classification: CompatibilityClassification,
    /// Shared `change-kind` value when the delta maps to the contract
    /// vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_kind: Option<String>,
    /// Registry project that produces the contract.
    pub producer_project: String,
    /// Registry project that consumes the contract.
    pub consumer_project: String,
    /// Contract path from the producer baseline, relative to repo root.
    pub producer_contract: String,
    /// Contract path from the consumer workspace, relative to repo root.
    pub consumer_contract: String,
    /// Human-readable locator inside the contract document.
    pub locator: String,
    /// Human-readable diagnostic detail.
    pub details: String,
}

/// Aggregate counts for a compatibility report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilitySummary {
    /// Number of findings in the report.
    pub total_findings: usize,
    /// Number of `additive` findings.
    pub additive: usize,
    /// Number of `breaking` findings.
    pub breaking: usize,
    /// Number of `ambiguous` findings.
    pub ambiguous: usize,
    /// Number of `unverifiable` findings.
    pub unverifiable: usize,
}

impl CompatibilitySummary {
    fn from_findings(findings: &[CompatibilityFinding]) -> Self {
        let mut summary = Self {
            total_findings: findings.len(),
            additive: 0,
            breaking: 0,
            ambiguous: 0,
            unverifiable: 0,
        };
        for finding in findings {
            match finding.classification {
                CompatibilityClassification::Additive => summary.additive += 1,
                CompatibilityClassification::Breaking => summary.breaking += 1,
                CompatibilityClassification::Ambiguous => summary.ambiguous += 1,
                CompatibilityClassification::Unverifiable => summary.unverifiable += 1,
            }
        }
        summary
    }
}

/// Full cross-project compatibility report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CompatibilityReport {
    /// Optional change name supplied by the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// Number of producer / consumer contract pairs inspected.
    pub checked_pairs: usize,
    /// `true` when the report has no blocking compatibility risk.
    pub ok: bool,
    /// Per-delta findings.
    pub findings: Vec<CompatibilityFinding>,
    /// Aggregate counts by classification.
    pub summary: CompatibilitySummary,
}

impl CompatibilityReport {
    fn new(
        change: Option<String>, checked_pairs: usize, mut findings: Vec<CompatibilityFinding>,
    ) -> Self {
        findings.sort_by(|a, b| {
            a.producer_project
                .cmp(&b.producer_project)
                .then_with(|| a.consumer_project.cmp(&b.consumer_project))
                .then_with(|| a.producer_contract.cmp(&b.producer_contract))
                .then_with(|| a.locator.cmp(&b.locator))
                .then_with(|| a.classification.as_str().cmp(b.classification.as_str()))
        });
        let summary = CompatibilitySummary::from_findings(&findings);
        let ok = summary.breaking == 0 && summary.ambiguous == 0 && summary.unverifiable == 0;
        Self {
            change,
            checked_pairs,
            ok,
            findings,
            summary,
        }
    }

    /// Returns `true` when the report contains only clean or additive
    /// compatibility results.
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.ok
    }
}

/// Classify cross-project compatibility for the current project.
///
/// Re-exported from the crate root as
/// [`crate::validate::classify_project_compatibility`] — that is the canonical
/// public name; this module-level alias avoids a stuttering function
/// suffix at the definition site.
///
/// # Errors
///
/// Returns an error when `registry.yaml` cannot be loaded.
pub fn classify_project(
    project_dir: &Path, change: Option<String>,
) -> Result<CompatibilityReport, Error> {
    let Some(registry) = Registry::load(project_dir)? else {
        return Ok(CompatibilityReport::new(change, 0, Vec::new()));
    };
    let baseline_findings = validate_baseline(&project_dir.join("contracts"));
    let mut findings = Vec::new();
    let mut checked_pairs = 0;

    for producer in &registry.projects {
        let Some(roles) = &producer.contracts else {
            continue;
        };
        for produced in &roles.produces {
            let rel = normalize_contract_path(produced);
            for consumer in consumers_for(&registry, producer, &rel) {
                checked_pairs += 1;
                let ctx = PairContext {
                    producer_project: &producer.name,
                    consumer_project: &consumer.name,
                    producer_contract: repo_contract_path(&rel),
                    consumer_contract: repo_contract_path(&rel),
                };
                diff_pair_contracts(project_dir, &rel, &baseline_findings, &ctx, &mut findings);
            }
        }
    }

    Ok(CompatibilityReport::new(change, checked_pairs, findings))
}

fn consumers_for<'a>(
    registry: &'a Registry, producer: &RegistryProject, rel: &str,
) -> Vec<&'a RegistryProject> {
    registry
        .projects
        .iter()
        .filter(|candidate| candidate.name != producer.name)
        .filter(|candidate| {
            candidate.contracts.as_ref().is_some_and(|roles| {
                roles.consumes.iter().any(|consumed| normalize_contract_path(consumed) == rel)
            })
        })
        .collect()
}

fn normalize_contract_path(path: &str) -> String {
    path.strip_prefix("contracts/").unwrap_or(path).trim_start_matches('/').to_string()
}

pub(super) fn repo_contract_path(rel: &str) -> String {
    format!("contracts/{rel}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn missing_consumer_baseline_is_unverifiable() {
        let tmp = TempDir::new().expect("tempdir");
        write_project(&tmp);
        write_file(
            &tmp.path().join("contracts/schemas/user.yaml"),
            "type: object\nproperties:\n  id:\n    type: string\n",
        );
        let report = classify_project(tmp.path(), Some("demo".to_string())).expect("report");
        assert_eq!(report.checked_pairs, 1);
        assert_eq!(report.summary.unverifiable, 1);
        assert!(!report.is_compatible());
    }

    fn write_project(tmp: &TempDir) {
        write_file(&tmp.path().join(".specify/project.yaml"), "name: hub\nhub: true\nrules: {}\n");
        write_file(
            &tmp.path().join("registry.yaml"),
            "version: 1\nprojects:\n  - name: backend\n    url: ../backend\n    capability: omnia@v1\n    description: Backend service.\n    contracts:\n      produces:\n        - schemas/user.yaml\n  - name: mobile\n    url: ../mobile\n    capability: vectis@v1\n    description: Mobile client.\n    contracts:\n      consumes:\n        - schemas/user.yaml\n",
        );
    }

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("test path has parent")).expect("mkdir");
        fs::write(path, content).expect("write file");
    }
}
