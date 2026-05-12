//! Canonical JSON envelope for the standalone `specify-contract`
//! binary. See `DECISIONS.md` §"Change D — Validate JSON envelope shape"
//! for provenance.

use std::path::Path;

use serde::Serialize;

use super::ContractFinding;

/// Serialise a baseline-contract validation result to the canonical
/// pretty-printed JSON envelope.
///
/// The shape is byte-compatible with the legacy `specify contract
/// validate --format json` envelope:
///
/// ```json
/// {
///   "envelope-version": 2,
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
/// deterministic.
///
/// Findings paths are emitted relative to `baseline_dir.parent()`
/// when that prefix is present on the absolute path produced by
/// [`crate::validate_baseline`]; otherwise the raw path is rendered.
/// This mirrors the legacy behaviour where the CLI stripped the
/// project root from finding paths so operators saw
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
        envelope_version: 2,
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
    #[serde(rename = "envelope-version")]
    envelope_version: u64,
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
    use std::path::PathBuf;

    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    fn write_contract(tmp: &TempDir, rel: &str, body: &str) -> PathBuf {
        let path = tmp.path().join("contracts").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn contracts_dir(tmp: &TempDir) -> PathBuf {
        tmp.path().join("contracts")
    }

    fn json_value(s: &str) -> Value {
        serde_json::from_str(s).expect("valid JSON")
    }

    #[test]
    fn empty_findings_render_ok_payload() {
        let tmp = TempDir::new().unwrap();
        let baseline = contracts_dir(&tmp);
        fs::create_dir_all(&baseline).unwrap();
        let s = serialize_contract_findings(&baseline, &[], 0);
        let value = json_value(&s);
        assert_eq!(value["envelope-version"], 2);
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
            rule_id: "contract.version-is-semver",
            detail: "demo".to_string(),
        }];
        let value = json_value(&serialize_contract_findings(&baseline, &findings, 1));
        assert_eq!(value["ok"], false);
        assert_eq!(value["exit-code"], 1);
        let rendered = value["findings"][0]["path"].as_str().unwrap();
        assert_eq!(rendered, "contracts/http/user-api.yaml");
        assert_eq!(value["findings"][0]["rule-id"], "contract.version-is-semver");
        assert_eq!(value["findings"][0]["detail"], "demo");
    }

    #[test]
    fn serialize_keeps_path_when_baseline_parent_does_not_match() {
        let baseline = PathBuf::from("/no/such/baseline");
        let foreign = PathBuf::from("/some/other/place/contracts/x.yaml");
        let findings = vec![ContractFinding {
            path: foreign.clone(),
            rule_id: "contract.id-format",
            detail: "demo".to_string(),
        }];
        let value = json_value(&serialize_contract_findings(&baseline, &findings, 1));
        assert_eq!(value["findings"][0]["path"], foreign.display().to_string());
    }

    /// Field order in the rendered JSON must match the legacy envelope
    /// (top-level keys: `envelope-version`, `contracts-dir`, `ok`,
    /// `findings`, `exit-code`; per-finding keys: `path`, `rule-id`,
    /// `detail`).
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
            rule_id: "contract.version-is-semver",
            detail: "demo".to_string(),
        }];
        let s = serialize_contract_findings(&baseline, &findings, 1);
        let p_schema = s.find("\"envelope-version\"").expect("envelope-version present");
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
