//! Baseline-contract validation primitives shared by the host CLI and
//! the standalone WASI carve-out. Walks `contracts/` and enforces
//! `version-is-semver`, `id-format`, and `id-unique` against each doc.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod parse;

/// One validation finding produced by [`validate_baseline`].
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

/// Run the baseline-contract validation checks across `contracts_dir`.
///
/// Returns an empty vector when the directory does not exist, when it
/// is empty, or when every walked file is well-formed. The order of
/// findings is deterministic: rules within a file appear in the order
/// listed in the module docs, and files appear in lexicographic path
/// order.
#[must_use]
pub fn validate_baseline(contracts_dir: &Path) -> Vec<ContractFinding> {
    if std::fs::read_dir(contracts_dir).is_err() {
        return Vec::new();
    }

    let docs = parse::collect_top_level_docs(contracts_dir);

    let mut findings: Vec<ContractFinding> = Vec::new();
    let mut id_to_paths: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for doc in &docs {
        let info = doc.value.get("info");

        match parse::version_str(info) {
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

        if let Some(id) = parse::id_str(info) {
            if parse::is_valid_specify_id(id) {
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

/// Convenience alias matching the historic re-export name. The host
/// CLI's `specify_domain::validate::validate_baseline_contracts` and
/// `wasi-tools/contract` both use this spelling.
pub use validate_baseline as validate_baseline_contracts;

#[cfg(test)]
mod tests {
    use std::fs;

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

    fn finding_kinds(findings: &[ContractFinding]) -> Vec<&'static str> {
        findings.iter().map(|f| f.rule_id).collect()
    }

    #[test]
    fn absent_directory_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        let findings = validate_baseline(&tmp.path().join("contracts"));
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_directory_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("contracts")).unwrap();
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn semver_prerelease_label_passes() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0-draft.1\n",
        );
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn asyncapi_top_level_is_validated() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "messages/orders.yaml",
            "asyncapi: '3.0.0'\ninfo:\n  title: Orders\n  version: 2024-01-15\n",
        );
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn unparseable_yaml_is_skipped() {
        let tmp = TempDir::new().unwrap();
        write_contract(&tmp, "http/broken.yaml", ":this is not yaml: [\n");
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }

    #[test]
    fn date_string_version_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
        );
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        let findings = validate_baseline(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
    }

    #[test]
    fn missing_version_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(&tmp, "http/user-api.yaml", "openapi: '3.1.0'\ninfo:\n  title: User API\n");
        let findings = validate_baseline(&contracts_dir(&tmp));
        assert_eq!(finding_kinds(&findings), vec![RULE_VERSION_IS_SEMVER]);
        assert!(findings[0].detail.contains("missing"));
    }

    #[test]
    fn id_format_uppercase_fails() {
        let tmp = TempDir::new().unwrap();
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: User-API\n",
        );
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }

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
        let findings = validate_baseline(&contracts_dir(&tmp));
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
        assert!(validate_baseline(&contracts_dir(&tmp)).is_empty());
    }
}
