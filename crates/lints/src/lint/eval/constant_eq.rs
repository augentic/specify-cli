//! `kind: constant-eq` evaluator.
//!
//! Asserts that some extracted field on a candidate fact equals a
//! configured constant. v1 supports one source discriminator —
//! `adapter-manifest-version-equals-v1` — which consumes the
//! [`crate::lint::AdapterManifest`] facts the framework-profile
//! indexer already produced
//! (see [`crate::lint::index::adapter::extract`], whose `version`
//! field stringifies both integer and string YAML forms) and flags
//! each `adapters/{sources,targets}/<name>/adapter.yaml` whose
//! `version:` does not equal the literal string `"1"`. The
//! interpreter emits one [`crate::rules::Diagnostic`] per
//! non-conforming manifest with the manifest path as the finding's
//! location and the `(actual, expected)` pair surfaced via
//! [`crate::rules::FindingEvidence::Structured`] for downstream
//! tooling. Manifests whose `version:` is absent count as actual
//! `"(absent)"`; that string can never collide with a real version
//! because the extractor rejects empty / non-string-or-number
//! values up front.
//!
//! Adapter manifests whose `path` is not in the caller-supplied
//! candidate set are ignored, so the closed `path-pattern` filter
//! the umbrella evaluator builds still drives candidate selection.
//! Manifests the indexer drops upstream (binary `adapter.yaml`,
//! YAML body without a non-empty `name:` value, etc.) never reach
//! this layer.
//!
//! The single source discriminator hardcodes both the field
//! (`AdapterManifest.version`) and the expected constant (`"1"`) —
//! this is the smoke-test landing path for the
//! reserved kind, so a richer config shape
//! (`{field: …, expected: …}`) is deferred until a second consumer
//! arrives. Future hint values may extend the closed source set;
//! unknown discriminators are rejected as
//! [`super::HintError::Unsupported`] so authoring drift surfaces at
//! hint-evaluation time rather than silently passing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{DeterministicHint, HintKind, ResolvedRule};

const SOURCE_ADAPTER_MANIFEST_VERSION_EQUALS_V1: &str = "adapter-manifest-version-equals-v1";
const ADAPTER_MANIFEST_EXPECTED_VERSION: &str = "1";
const ABSENT_VERSION_TOKEN: &str = "(absent)";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_ADAPTER_MANIFEST_VERSION_EQUALS_V1 {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::ConstantEq,
            reason: "only `adapter-manifest-version-equals-v1` is supported in v1",
        });
    }

    let candidate_set: BTreeSet<String> =
        candidates.iter().map(|p| p.to_string_lossy().into_owned()).collect();

    let mut out: Vec<Diagnostic> = Vec::new();
    for manifest in &model.adapter_manifests {
        if !candidate_set.contains(&manifest.path) {
            continue;
        }
        let actual = manifest.version.as_deref().unwrap_or(ABSENT_VERSION_TOKEN);
        if actual == ADAPTER_MANIFEST_EXPECTED_VERSION {
            continue;
        }
        let location = FindingLocation {
            path: manifest.path.clone(),
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        };
        let evidence = FindingEvidence::Structured {
            summary: format!(
                "adapter '{}' declares version '{}' (expected '{}')",
                manifest.name, actual, ADAPTER_MANIFEST_EXPECTED_VERSION,
            ),
            data: serde_json::json!({
                "adapter": manifest.name,
                "path": manifest.path,
                "field": "version",
                "actual": actual,
                "expected": ADAPTER_MANIFEST_EXPECTED_VERSION,
            }),
            locations: None,
        };
        let title = format!(
            "{}: adapter '{}' version '{}' does not equal '{}'",
            rule.title, manifest.name, actual, ADAPTER_MANIFEST_EXPECTED_VERSION,
        );
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}
