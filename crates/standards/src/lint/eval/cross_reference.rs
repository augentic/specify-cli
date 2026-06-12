//! `kind: cross-reference` evaluator.
//!
//! A generic relational join over a *source* and a *target*. Two source
//! shapes are supported, each pairing with a target fact family:
//!
//! - **fact-family source** (presence-only set difference) — `hint.value`
//!   names a source fact family; every element must have a corresponding
//!   element in the `config: { target }` fact family (joined on a
//!   per-family key). Each unmatched source item is flagged. v1 pair:
//!   source `adapter-dir` (the [`crate::lint::AdapterDir`] facts, one per
//!   immediate child directory under `adapters/{sources,targets}`, keyed
//!   by directory `path`) against target `adapter-manifest` (the
//!   [`crate::lint::AdapterManifest`] facts, keyed by the directory
//!   *containing* each manifest). For CORE-010 — an adapter directory with
//!   no resolvable `adapter.yaml`.
//!
//! - **expected-set source** (value-equality) — `hint.value` is the
//!   sentinel `expected-set`; the rule supplies a closed
//!   `config: { entries: [{ key, value }] }` table joined against the
//!   `config: { target }` fact family on the entry `key`. An entry is
//!   flagged when its key is absent from the target *and* the entry's
//!   scope (the parent of a `scope/leaf` key) exists in the target, or
//!   when the key is present but the target's value differs. Entries
//!   whose scope is absent from the target are skipped — the join never
//!   fabricates a finding for a group the target does not carry. v1 pair:
//!   `expected-set` against target `adapter-tool` (each
//!   [`crate::lint::AdapterManifest`] tool keyed `<adapter-dir>/<tool>`
//!   with the declared version as the value, scoped by adapter directory).
//!   For CORE-049 — a pinned first-party tool missing from, or
//!   version-mismatched in, its target adapter manifest.
//!
//! All policy (which family is source / target, the expected entries and
//! their values) rides the rule's `value` / `config:`; this arm names only
//! mechanism — the closed source / target selector tokens, the structural
//! join key each family contributes (a directory path, or an
//! `<adapter>/<tool>` pair, computed by the per-selector accessor like
//! every other fact-iterating kind), and the `scope/leaf` key convention
//! the value-equality join uses to gate absent groups. The evaluator
//! carries no rule id and no `adapters/...` path literal. Unknown
//! selectors are rejected as [`super::HintError::Unsupported`] so authoring
//! drift surfaces at hint-evaluation time rather than silently passing.
//!
//! The join is whole-tree: it reads the fact families directly and ignores
//! the candidate set (the source families — directories, config entries —
//! never appear in `model.files`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::{AdapterManifest, WorkspaceModel};
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_ADAPTER_DIR: &str = "adapter-dir";
const SOURCE_EXPECTED_SET: &str = "expected-set";
const TARGET_ADAPTER_MANIFEST: &str = "adapter-manifest";
const TARGET_ADAPTER_TOOL: &str = "adapter-tool";

/// Parsed `cross-reference` hint configuration. The target family
/// selector is supplied by the rule; the shape is schema-gated upstream
/// by `crossReferenceHintConfig` (the required `target` key disambiguates
/// the oneOf). The optional `entries` table carries the expected-set
/// source rows when `hint.value` is `expected-set`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CrossReferenceConfig {
    target: String,
    #[serde(default)]
    entries: Vec<ExpectedEntry>,
}

/// One expected-set row: the join `key` and the `value` the matched
/// target entry must equal. Both are opaque strings the rule author
/// composes against the target family's documented key / value mechanism.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ExpectedEntry {
    key: String,
    value: String,
}

impl CrossReferenceConfig {
    fn parse(rule: &ResolvedRule, hint: &RuleHint) -> Result<Self, HintError> {
        let raw = hint.config.as_ref().ok_or_else(|| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "`cross-reference` requires a `config: { target }`",
        })?;
        serde_json::from_value(raw.clone()).map_err(|_ignored| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "invalid cross-reference hint config JSON",
        })
    }
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, _candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg = CrossReferenceConfig::parse(rule, hint)?;
    let target = cfg.target.trim();
    match hint.value.trim() {
        SOURCE_EXPECTED_SET => evaluate_expected(rule, &cfg, target, model, next_id),
        source => evaluate_family(rule, source, target, &cfg, model, next_id),
    }
}

/// Fact-family presence join: flag every source-family key with no
/// corresponding target-family key.
fn evaluate_family(
    rule: &ResolvedRule, source: &str, target: &str, cfg: &CrossReferenceConfig,
    model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    if !cfg.entries.is_empty() {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "`config: { entries }` is only valid with the `expected-set` source",
        });
    }
    let source_keys = source_keys(rule, source, model)?;
    let target_keys = presence_target_keys(rule, target, model)?;

    let mut out: Vec<Diagnostic> = Vec::new();
    for (key, path) in source_keys {
        if target_keys.contains(&key) {
            continue;
        }
        let summary = format!("'{path}' has no corresponding '{target}' entry");
        out.push(mint(rule, &path, &summary, next_id));
    }
    Ok(out)
}

/// Expected-set value-equality join: for each rule-supplied entry whose
/// scope exists in the target, flag a missing key or a value mismatch.
fn evaluate_expected(
    rule: &ResolvedRule, cfg: &CrossReferenceConfig, target: &str, model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    if cfg.entries.is_empty() {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "the `expected-set` source requires a non-empty `config: { entries }`",
        });
    }
    let view = value_target_view(rule, target, model)?;

    let mut out: Vec<Diagnostic> = Vec::new();
    for entry in &cfg.entries {
        // Scope gating: an entry `scope/leaf` is only checked when its
        // scope group is carried by the target. This preserves the
        // "skip when the group is entirely absent" leniency without the
        // engine knowing what a scope means.
        if let Some((scope, _leaf)) = entry.key.rsplit_once('/') {
            let Some(scope_path) = view.scope_paths.get(scope) else {
                continue;
            };
            match view.values.get(&entry.key) {
                None => {
                    let summary =
                        format!("'{}' expected by '{target}' but not declared", entry.key);
                    out.push(mint(rule, scope_path, &summary, next_id));
                }
                Some(actual) if actual != &entry.value => {
                    let summary = format!(
                        "'{}' must be '{}' in '{target}' but is '{actual}'",
                        entry.key, entry.value
                    );
                    out.push(mint(rule, scope_path, &summary, next_id));
                }
                Some(_match) => {}
            }
        }
    }
    Ok(out)
}

/// `(join-key, finding-location-path)` pairs for a fact-family source. The
/// join key and the finding location coincide (the directory path).
fn source_keys(
    rule: &ResolvedRule, selector: &str, model: &WorkspaceModel,
) -> Result<Vec<(String, String)>, HintError> {
    match selector {
        SOURCE_ADAPTER_DIR => {
            Ok(model.adapter_dirs.iter().map(|dir| (dir.path.clone(), dir.path.clone())).collect())
        }
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "only the `adapter-dir` cross-reference source is supported in v1",
        }),
    }
}

/// The set of join keys present in a presence-join target family. For
/// `adapter-manifest` the key is the directory containing each manifest —
/// the structural counterpart of the `adapter-dir` directory key.
fn presence_target_keys(
    rule: &ResolvedRule, selector: &str, model: &WorkspaceModel,
) -> Result<BTreeSet<String>, HintError> {
    match selector {
        TARGET_ADAPTER_MANIFEST => Ok(model
            .adapter_manifests
            .iter()
            .filter_map(|manifest| containing_dir(&manifest.path))
            .collect()),
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "only the `adapter-manifest` presence target is supported in v1",
        }),
    }
}

/// A value-equality target family projection: the `key -> value` map the
/// expected entries are compared against plus the `scope -> location`
/// index used for scope gating and finding location.
struct ValueTargetView {
    values: BTreeMap<String, String>,
    scope_paths: BTreeMap<String, String>,
}

/// Build the value-equality projection for a target selector. For
/// `adapter-tool` each manifest contributes its directory name as a scope
/// (located at the manifest path) and one `<dir>/<tool>` -> version entry
/// per declared tool.
fn value_target_view(
    rule: &ResolvedRule, selector: &str, model: &WorkspaceModel,
) -> Result<ValueTargetView, HintError> {
    match selector {
        TARGET_ADAPTER_TOOL => {
            let mut values: BTreeMap<String, String> = BTreeMap::new();
            let mut scope_paths: BTreeMap<String, String> = BTreeMap::new();
            for manifest in &model.adapter_manifests {
                let Some(dir) = adapter_dir_name(manifest) else {
                    continue;
                };
                scope_paths.entry(dir.clone()).or_insert_with(|| manifest.path.clone());
                for tool in &manifest.tools {
                    values.insert(format!("{dir}/{}", tool.name), tool.version.clone());
                }
            }
            Ok(ValueTargetView { values, scope_paths })
        }
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::CrossReference,
            reason: "only the `adapter-tool` value-equality target is supported in v1",
        }),
    }
}

/// The adapter directory name for a manifest — the final segment of the
/// directory containing its `adapter.yaml` (e.g. `vectis` for
/// `adapters/targets/vectis/adapter.yaml`).
fn adapter_dir_name(manifest: &AdapterManifest) -> Option<String> {
    let dir = containing_dir(&manifest.path)?;
    dir.rsplit('/').next().map(str::to_owned)
}

/// The directory containing a file fact's path (everything before the
/// final `/`), or `None` when the path has no directory component.
fn containing_dir(path: &str) -> Option<String> {
    path.rfind('/').map(|idx| path[..idx].to_owned())
}

/// Mint one cross-reference finding located at `path`, with structured
/// evidence carrying the offending path, and bump the id counter.
fn mint(rule: &ResolvedRule, path: &str, summary: &str, next_id: &mut u64) -> Diagnostic {
    let location = FindingLocation {
        path: path.to_owned(),
        line: None,
        column: None,
        end_line: None,
        end_column: None,
    };
    let evidence = FindingEvidence::Structured {
        summary: summary.to_owned(),
        data: serde_json::json!({ "path": path }),
        locations: None,
    };
    let title = format!("{}: {summary}", rule.title);
    let finding = make_finding(rule, *next_id, title, Some(location), evidence);
    *next_id += 1;
    finding
}

#[cfg(test)]
mod unit {
    use serde_json::json;

    use super::*;
    use crate::lint::eval::testkit::{empty_model, hint_with_config, rule};
    use crate::lint::{AdapterAxis, AdapterDir, AdapterTool};

    fn dir(path: &str) -> AdapterDir {
        AdapterDir {
            path: path.to_string(),
            axis: AdapterAxis::Targets,
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
        }
    }

    fn manifest_with_tools(name: &str, tools: &[(&str, &str)]) -> AdapterManifest {
        AdapterManifest {
            axis: AdapterAxis::Targets,
            name: name.to_string(),
            path: format!("adapters/targets/{name}/adapter.yaml"),
            version: None,
            brief_keys: vec![],
            tools: tools
                .iter()
                .map(|(tool_name, version)| AdapterTool {
                    name: (*tool_name).to_string(),
                    version: (*version).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn orphan_adapter_dir_flagged() {
        let mut model = empty_model();
        model.adapter_dirs =
            vec![dir("adapters/targets/with-manifest"), dir("adapters/targets/orphan")];
        model.adapter_manifests = vec![manifest_with_tools("with-manifest", &[])];
        let cfg = json!({ "target": "adapter-manifest" });
        let hint = hint_with_config(HintKind::CrossReference, "adapter-dir", Some(cfg));
        let out = evaluate(&rule(), &hint, &[], &model, &mut 1).expect("evaluate");
        assert_eq!(out.len(), 1);
        assert!(out[0].title.contains("adapters/targets/orphan"), "{}", out[0].title);
    }

    #[test]
    fn expected_set_value_join() {
        let mut model = empty_model();
        model.adapter_manifests = vec![manifest_with_tools("vectis", &[("vectis", "0.4.0")])];
        let cfg = json!({
            "target": "adapter-tool",
            "entries": [
                { "key": "vectis/vectis", "value": "0.4.0" },   // match: silent
                { "key": "vectis/extra", "value": "1.0.0" },    // missing in present scope: flagged
                { "key": "omnia/tool", "value": "1.0.0" },      // absent scope: skipped
            ],
        });
        let hint = hint_with_config(HintKind::CrossReference, "expected-set", Some(cfg));
        let out = evaluate(&rule(), &hint, &[], &model, &mut 1).expect("evaluate");
        assert_eq!(out.len(), 1);
        assert!(out[0].title.contains("'vectis/extra'"), "{}", out[0].title);
    }

    #[test]
    fn expected_set_version_mismatch_flagged() {
        let mut model = empty_model();
        model.adapter_manifests = vec![manifest_with_tools("vectis", &[("vectis", "0.3.0")])];
        let cfg = json!({
            "target": "adapter-tool",
            "entries": [{ "key": "vectis/vectis", "value": "0.4.0" }],
        });
        let hint = hint_with_config(HintKind::CrossReference, "expected-set", Some(cfg));
        let out = evaluate(&rule(), &hint, &[], &model, &mut 1).expect("evaluate");
        assert_eq!(out.len(), 1);
        assert!(out[0].title.contains("must be '0.4.0'"), "{}", out[0].title);
    }

    #[test]
    fn entries_invalid_for_family_source() {
        let model = empty_model();
        let cfg = json!({
            "target": "adapter-manifest",
            "entries": [{ "key": "k", "value": "v" }],
        });
        let hint = hint_with_config(HintKind::CrossReference, "adapter-dir", Some(cfg));
        evaluate(&rule(), &hint, &[], &model, &mut 1).unwrap_err();
    }

    #[test]
    fn expected_set_requires_entries() {
        let model = empty_model();
        let cfg = json!({ "target": "adapter-tool" });
        let hint = hint_with_config(HintKind::CrossReference, "expected-set", Some(cfg));
        evaluate(&rule(), &hint, &[], &model, &mut 1).unwrap_err();
    }

    #[test]
    fn unknown_selectors_rejected() {
        let model = empty_model();
        let source = hint_with_config(
            HintKind::CrossReference,
            "no-such-source",
            Some(json!({ "target": "adapter-manifest" })),
        );
        evaluate(&rule(), &source, &[], &model, &mut 1).unwrap_err();
        let target = hint_with_config(
            HintKind::CrossReference,
            "adapter-dir",
            Some(json!({ "target": "no-such-target" })),
        );
        evaluate(&rule(), &target, &[], &model, &mut 1).unwrap_err();
    }
}
