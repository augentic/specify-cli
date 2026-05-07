//! Baseline-contract checks (RFC-12 §Validation).
//!
//! Walks the supplied `contracts/` directory (typically the project
//! baseline at `<project>/contracts/`), projects each top-level
//! `OpenAPI` 3.1 / `AsyncAPI` 3.0 document (root `openapi:` or
//! `asyncapi:` key — format detection per RFC-12 §"Top-level
//! contracts"), and enforces three rules:
//!
//! 1. `contract.version-is-semver` — `info.version` must parse as
//!    `SemVer` (prerelease labels included; the `semver` crate decides).
//! 2. `contract.id-format` — when `info.x-specify-id` is present,
//!    matches `^[a-z][a-z0-9-]*$` and is ≤ 64 characters.
//! 3. `contract.id-unique` — every `info.x-specify-id` value is
//!    unique across the walked set; on duplicates, both offending
//!    paths are reported.
//!
//! Files that fail to parse as YAML are skipped silently — the
//! contracts-brief verifier owns that diagnostic. The standalone JSON
//! Schema files under `contracts/schemas/` are payload vocabulary, not
//! top-level contracts, and are skipped by the same `openapi:` /
//! `asyncapi:` filter (RFC-12 §"Top-level contracts" + §Non-goals).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// One validation finding produced by [`validate_baseline_contracts`].
///
/// `rule_id` is one of `contract.version-is-semver`,
/// `contract.id-format`, or `contract.id-unique`. `path` is the
/// absolute path to the offending YAML file, suitable to render
/// verbatim in the operator's terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractFinding {
    /// Absolute path to the contract file the finding refers to.
    pub path: PathBuf,
    /// Stable rule identifier (`contract.<rule>`).
    pub rule_id: &'static str,
    /// Human-readable failure detail (file-name-aware).
    pub detail: String,
}

const RULE_VERSION_IS_SEMVER: &str = "contract.version-is-semver";
const RULE_ID_FORMAT: &str = "contract.id-format";
const RULE_ID_UNIQUE: &str = "contract.id-unique";

/// Run the RFC-12 §Validation checks across `contracts_dir`.
///
/// Returns an empty vector when the directory does not exist, when it
/// is empty, or when every walked file is well-formed. The order of
/// findings is deterministic: rules within a file appear in the order
/// listed in the module docs, and files appear in lexicographic path
/// order.
#[must_use]
pub fn validate_baseline_contracts(contracts_dir: &Path) -> Vec<ContractFinding> {
    if std::fs::read_dir(contracts_dir).is_err() {
        return Vec::new();
    }

    let docs = collect_top_level_docs(contracts_dir);

    let mut findings: Vec<ContractFinding> = Vec::new();
    let mut id_to_paths: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for doc in &docs {
        let info = doc.value.get("info");

        match version_str(info) {
            Some(v) if semver::Version::parse(v).is_ok() => {}
            Some(v) => findings.push(ContractFinding {
                path: doc.path.clone(),
                rule_id: RULE_VERSION_IS_SEMVER,
                detail: format!(
                    "info.version `{v}` is not valid SemVer (must parse per semver.org, \
                     including optional prerelease labels)"
                ),
            }),
            None => findings.push(ContractFinding {
                path: doc.path.clone(),
                rule_id: RULE_VERSION_IS_SEMVER,
                detail: "info.version is missing or not a string; \
                         every top-level OpenAPI / AsyncAPI document must \
                         set a SemVer info.version"
                    .to_string(),
            }),
        }

        if let Some(id) = id_str(info) {
            if is_valid_specify_id(id) {
                id_to_paths.entry(id.to_string()).or_default().push(doc.path.clone());
            } else {
                findings.push(ContractFinding {
                    path: doc.path.clone(),
                    rule_id: RULE_ID_FORMAT,
                    detail: format!(
                        "info.x-specify-id `{id}` must match `^[a-z][a-z0-9-]*$` \
                         and be ≤ 64 characters"
                    ),
                });
            }
        }
    }

    for (id, paths) in &id_to_paths {
        if paths.len() < 2 {
            continue;
        }
        let listed: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        for path in paths {
            findings.push(ContractFinding {
                path: path.clone(),
                rule_id: RULE_ID_UNIQUE,
                detail: format!(
                    "info.x-specify-id `{id}` is declared by multiple top-level contracts: {}",
                    listed.join(", ")
                ),
            });
        }
    }

    findings.sort_by(|a, b| {
        a.path
            .as_os_str()
            .cmp(b.path.as_os_str())
            .then_with(|| a.rule_id.cmp(b.rule_id))
            .then_with(|| a.detail.cmp(&b.detail))
    });

    findings
}

/// Parsed top-level contract document — the YAML root plus the
/// absolute path it came from.
struct TopLevelDoc {
    path: PathBuf,
    value: Value,
}

/// Walk `contracts_dir` for `*.yaml` files, parse each, and keep only
/// those whose root carries `openapi:` or `asyncapi:` (RFC-12
/// §"Top-level contracts"). YAML parse errors are swallowed silently
/// — the contracts-brief verifier owns that diagnostic; this module is
/// identity / version only.
fn collect_top_level_docs(contracts_dir: &Path) -> Vec<TopLevelDoc> {
    let mut paths = Vec::new();
    collect_yaml_paths(contracts_dir, &mut paths);
    paths.sort();
    let mut out: Vec<TopLevelDoc> = Vec::new();
    for entry in paths {
        let Ok(content) = std::fs::read_to_string(&entry) else {
            continue;
        };
        let Ok(value) = serde_saphyr::from_str::<Value>(&content) else {
            continue;
        };
        if !is_top_level(&value) {
            continue;
        }
        out.push(TopLevelDoc { path: entry, value });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn collect_yaml_paths(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_yaml_paths(&path, out);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "yaml") {
            out.push(path);
        }
    }
}

/// `true` when `value`'s root object declares `openapi:` or
/// `asyncapi:`. Matches the format-detection rule from RFC-12
/// §"Top-level contracts" — directory layout and filename are
/// explicitly *not* signals.
fn is_top_level(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("openapi") || obj.contains_key("asyncapi")
}

fn version_str(info: Option<&Value>) -> Option<&str> {
    info?.get("version")?.as_str()
}

fn id_str(info: Option<&Value>) -> Option<&str> {
    info?.get("x-specify-id")?.as_str()
}

/// Mirror of the kebab-case rule used by `composition.screen-slugs-kebab`
/// (`crate::registry`) and `RegistryProject::name`. Inlined here so the
/// id check stays self-contained and so the cap is enforced (≤ 64
/// characters per RFC-12 §Validation rule 2).
fn is_valid_specify_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut prev_dash = false;
    for &b in bytes {
        let lower = b.is_ascii_lowercase();
        let digit = b.is_ascii_digit();
        let dash = b == b'-';
        if !(lower || digit || dash) {
            return false;
        }
        if dash && prev_dash {
            return false;
        }
        prev_dash = dash;
    }
    if prev_dash {
        return false;
    }
    true
}

/// Serialise a baseline-contract validation result to the canonical
/// pretty-printed JSON envelope consumed by the standalone
/// `specify-contract-validate` binary (RFC-13 §4.2a).
///
/// The shape is byte-compatible with the pre-Phase-2.7 `specify
/// contract validate --format json` envelope:
///
/// ```json
/// {
///   "schema-version": 2,
///   "contracts-dir": "<baseline-dir>",
///   "ok": true,
///   "findings": [
///     { "path": "...", "rule-id": "...", "detail": "..." }
///   ],
///   "exit-code": 0
/// }
/// ```
///
/// Field order is preserved (typed `Serialize` structs piped through
/// `serde_json::to_string_pretty`) so the byte sequence is
/// deterministic and matches the legacy envelope key-for-key.
///
/// Findings paths are emitted relative to `baseline_dir.parent()`
/// when that prefix is present on the absolute path produced by
/// [`validate_baseline_contracts`]; otherwise the raw path is
/// rendered. This mirrors the pre-Phase-2.7 behaviour where the CLI
/// stripped the project root from finding paths so operators saw
/// `contracts/<file>` rather than absolute paths.
///
/// `exit_code` is the value the caller intends to surface as the
/// process exit code (the standalone binary uses `0` for success and
/// `1` for findings; see the binary's `--help`).
///
/// # Panics
///
/// Panics if `serde_json` fails to serialise the envelope. The
/// envelope is composed of fully-owned `String` / `bool` / `u64` /
/// `&'static str` fields with no foreign `Serialize` impls, so this
/// is structurally unreachable; the panic exists only as a
/// last-resort tripwire.
#[must_use]
pub fn serialize_contract_findings(
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

    let envelope = ValidateEnvelope {
        schema_version: 2,
        contracts_dir: baseline_dir.display().to_string(),
        ok: findings.is_empty(),
        findings: payload,
        exit_code,
    };
    serde_json::to_string_pretty(&envelope).expect("envelope is JSON-safe")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateEnvelope {
    #[serde(rename = "schema-version")]
    schema_version: u64,
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// Materialise `contracts/<rel-path>` with `body` and return the
    /// project-root tempdir handle.
    fn write_contract(tmp: &TempDir, rel: &str, body: &str) -> PathBuf {
        let path = tmp.path().join("contracts").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn contracts_dir(tmp: &TempDir) -> PathBuf {
        tmp.path().join("contracts")
    }

    fn finding_kinds(findings: &[ContractFinding]) -> Vec<&'static str> {
        findings.iter().map(|f| f.rule_id).collect()
    }

    // ---------- happy paths ----------

    #[test]
    fn absent_directory_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        let findings = validate_baseline_contracts(&tmp.path().join("contracts"));
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_directory_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("contracts")).unwrap();
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert!(findings.is_empty());
    }

    #[test]
    fn semver_version_passes() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n",
        );
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn semver_prerelease_label_passes() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0-draft.1\n",
        );
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn asyncapi_top_level_is_validated() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "messages/orders.yaml",
            "asyncapi: '3.0.0'\ninfo:\n  title: Orders\n  version: 2024-01-15\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
    }

    #[test]
    fn json_schema_file_is_skipped() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "schemas/user.yaml",
            "$id: urn:specify:schemas/user\ntitle: User\ndescription: A user.\ntype: object\n",
        );
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn unparseable_yaml_is_skipped() {
        let tmp = TempDir::new().unwrap();
        write_contract(&tmp, "http/broken.yaml", ":this is not yaml: [\n");
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    // ---------- semver rule ----------

    #[test]
    fn date_string_version_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
        assert!(findings[0].detail.contains("2024-01-15"));
    }

    #[test]
    fn major_only_version_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: '1'\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
    }

    #[test]
    fn missing_version_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(&tmp, "http/user-api.yaml", "openapi: '3.1.0'\ninfo:\n  title: User API\n");
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
        assert!(findings[0].detail.contains("missing"));
    }

    // ---------- id-format rule ----------

    #[test]
    fn id_format_uppercase_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: User-API\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_ID_FORMAT]);
    }

    #[test]
    fn id_format_leading_hyphen_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: -leading\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_ID_FORMAT]);
    }

    #[test]
    fn id_format_too_long_fails() {
        let tmp = TempDir::new().unwrap();
        let too_long: String = std::iter::repeat_n('a', 65).collect();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            &format!(
                "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: {too_long}\n"
            ),
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_ID_FORMAT]);
    }

    #[test]
    fn id_format_kebab_case_passes() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
        );
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    // ---------- id-unique rule ----------

    #[test]
    fn id_duplicates_across_two_files_fail_both() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: shared\n",
        );
        write_contract(
            &tmp,
            "http/billing-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: Billing API\n  version: 1.0.0\n  x-specify-id: shared\n",
        );
        let findings = validate_baseline_contracts(&contracts_dir(&tmp));
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.rule_id == RULE_ID_UNIQUE));
        assert!(
            findings.iter().any(|f| f.path.ends_with("http/user-api.yaml")),
            "user-api.yaml flagged"
        );
        assert!(
            findings.iter().any(|f| f.path.ends_with("http/billing-api.yaml")),
            "billing-api.yaml flagged"
        );
    }

    #[test]
    fn missing_id_does_not_count_as_duplicate() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n",
        );
        write_contract(
            &tmp,
            "http/billing-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: Billing API\n  version: 1.0.0\n",
        );
        assert!(validate_baseline_contracts(&contracts_dir(&tmp)).is_empty());
    }

    // ---------- serialize_contract_findings ----------

    fn json_value(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    #[test]
    fn serialize_empty_findings_renders_ok_envelope() {
        let tmp = TempDir::new().unwrap();
        let baseline = contracts_dir(&tmp);
        fs::create_dir_all(&baseline).unwrap();
        let s = serialize_contract_findings(&baseline, &[], 0);
        let value = json_value(&s);
        assert_eq!(value["schema-version"], 2);
        assert_eq!(value["contracts-dir"], baseline.display().to_string());
        assert_eq!(value["ok"], true);
        assert_eq!(value["findings"], serde_json::json!([]));
        assert_eq!(value["exit-code"], 0);
    }

    #[test]
    fn serialize_strips_baseline_parent_from_finding_paths() {
        let tmp = TempDir::new().unwrap();
        let path = write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: not-semver\n",
        );
        let baseline = contracts_dir(&tmp);
        let findings = vec![ContractFinding {
            path,
            rule_id: RULE_VERSION_IS_SEMVER,
            detail: "demo".to_string(),
        }];
        let value = json_value(&serialize_contract_findings(&baseline, &findings, 1));
        assert_eq!(value["ok"], false);
        assert_eq!(value["exit-code"], 1);
        let rendered = value["findings"][0]["path"].as_str().unwrap();
        assert_eq!(rendered, "contracts/http/user-api.yaml");
        assert_eq!(value["findings"][0]["rule-id"], RULE_VERSION_IS_SEMVER);
        assert_eq!(value["findings"][0]["detail"], "demo");
    }

    #[test]
    fn serialize_keeps_path_when_baseline_parent_does_not_match() {
        let baseline = PathBuf::from("/no/such/baseline");
        let foreign = PathBuf::from("/some/other/place/contracts/x.yaml");
        let findings = vec![ContractFinding {
            path: foreign.clone(),
            rule_id: RULE_ID_FORMAT,
            detail: "demo".to_string(),
        }];
        let value = json_value(&serialize_contract_findings(&baseline, &findings, 1));
        assert_eq!(value["findings"][0]["path"], foreign.display().to_string());
    }

    /// Field order in the rendered JSON must match the legacy
    /// envelope (top-level keys: `schema-version`, `contracts-dir`,
    /// `ok`, `findings`, `exit-code`; per-finding keys: `path`,
    /// `rule-id`, `detail`).
    #[test]
    fn serialize_preserves_legacy_field_order() {
        let tmp = TempDir::new().unwrap();
        let path = write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: not-semver\n",
        );
        let baseline = contracts_dir(&tmp);
        let findings = vec![ContractFinding {
            path,
            rule_id: RULE_VERSION_IS_SEMVER,
            detail: "demo".to_string(),
        }];
        let s = serialize_contract_findings(&baseline, &findings, 1);
        let p_schema = s.find("\"schema-version\"").expect("schema-version present");
        let p_contracts = s.find("\"contracts-dir\"").expect("contracts-dir present");
        let p_ok = s.find("\"ok\"").expect("ok present");
        let p_findings = s.find("\"findings\"").expect("findings present");
        let p_exit = s.find("\"exit-code\"").expect("exit-code present");
        assert!(p_schema < p_contracts);
        assert!(p_contracts < p_ok);
        assert!(p_ok < p_findings);
        assert!(p_findings < p_exit);
        let p_path = s.find("\"path\"").expect("path present");
        let p_rule = s.find("\"rule-id\"").expect("rule-id present");
        let p_detail = s.find("\"detail\"").expect("detail present");
        assert!(p_path < p_rule);
        assert!(p_rule < p_detail);
    }
}
