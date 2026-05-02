#![allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]

//! `specify contract { list, validate }` (RFC-12 §"CLI surface").
//!
//! Both verbs operate over the platform baseline at the path returned
//! by `ProjectConfig::contracts_dir` (today: `<project>/contracts/`).
//! They are deliberately read-only: `list` is a deterministic
//! projection, and `validate` surfaces the
//! [`specify::ContractFinding`] vector produced by the
//! `specify-validate` crate.
//!
//! Absent baseline (no `contracts/` directory) is **not** an error —
//! both verbs print an empty result with exit 0, matching the
//! `specify registry validate` posture for absent registries.
//!
//! Both handlers return `Result<CliResult, Error>` (rather than a bare
//! `CliResult`) so the top-level dispatcher in `commands/mod.rs` can
//! treat them uniformly with the other `run_with_project` handlers
//! that *do* propagate `Error` via `?`. The wrappers are flagged as
//! "unnecessary" by clippy when no inner step actually returns `Err`,
//! hence the file-level allow.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, validate_baseline_contracts};

use crate::cli::{ContractAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_contract(ctx: &CommandContext, action: ContractAction) -> Result<CliResult, Error> {
    match action {
        ContractAction::List => list_contracts(ctx),
        ContractAction::Validate => validate_contracts(ctx),
    }
}

/// `info.x-specify-id` is rendered as `null` in JSON when absent
/// (RFC-12 §"CLI surface").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ContractListEntry {
    /// Path to the contract file, relative to the project root.
    path: String,
    /// `openapi` or `asyncapi`.
    format: &'static str,
    /// `info.title`, or `null` when absent / non-string.
    title: Option<String>,
    /// `info.version`, or `null` when absent / non-string. Renders the
    /// raw string verbatim — `contract validate` enforces `SemVer`.
    version: Option<String>,
    /// `info.x-specify-id`, or `null` when absent (RFC-12 §"Optional
    /// rename-stable identity").
    #[serde(rename = "x-specify-id")]
    x_specify_id: Option<String>,
}

fn list_contracts(ctx: &CommandContext) -> Result<CliResult, Error> {
    let contracts_dir = ctx.contracts_dir();
    let entries = collect_entries(&contracts_dir, &ctx.project_dir);

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ListBody {
                contracts_dir: String,
                contracts: Vec<ContractListEntry>,
            }
            emit_response(ListBody {
                contracts_dir: contracts_dir.display().to_string(),
                contracts: entries,
            });
        }
        OutputFormat::Text => {
            if entries.is_empty() {
                println!("no top-level contracts under {}", contracts_dir.display());
            } else {
                print_list_table(&entries);
            }
        }
    }
    Ok(CliResult::Success)
}

fn validate_contracts(ctx: &CommandContext) -> Result<CliResult, Error> {
    let contracts_dir = ctx.contracts_dir();
    let findings = validate_baseline_contracts(&contracts_dir);
    let ok = findings.is_empty();
    let exit_code = if ok { CliResult::Success } else { CliResult::ValidationFailed };

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct FindingPayload {
                path: String,
                rule_id: String,
                detail: String,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ValidateBody {
                contracts_dir: String,
                ok: bool,
                findings: Vec<FindingPayload>,
                exit_code: u8,
            }
            let payload: Vec<FindingPayload> = findings
                .iter()
                .map(|f| FindingPayload {
                    path: relative_path_string(&f.path, &ctx.project_dir),
                    rule_id: f.rule_id.to_string(),
                    detail: f.detail.clone(),
                })
                .collect();
            emit_response(ValidateBody {
                contracts_dir: contracts_dir.display().to_string(),
                ok,
                findings: payload,
                exit_code: exit_code.code(),
            });
        }
        OutputFormat::Text => {
            if ok {
                if contracts_dir.is_dir() {
                    println!(
                        "PASS — every top-level contract under {} is well-formed",
                        contracts_dir.display()
                    );
                } else {
                    println!("no contracts directory at {}", contracts_dir.display());
                }
            } else {
                println!("FAIL — {} finding(s):", findings.len());
                for f in &findings {
                    eprintln!(
                        "  [{}] {}: {}",
                        f.rule_id,
                        relative_path_string(&f.path, &ctx.project_dir),
                        f.detail
                    );
                }
            }
        }
    }
    Ok(exit_code)
}

fn collect_entries(contracts_dir: &Path, project_dir: &Path) -> Vec<ContractListEntry> {
    if !contracts_dir.is_dir() {
        return Vec::new();
    }
    let pattern = match contracts_dir.join("**").join("*.yaml").to_str() {
        Some(p) => p.to_string(),
        None => return Vec::new(),
    };
    let Ok(walker) = glob::glob(&pattern) else {
        return Vec::new();
    };

    let mut out: Vec<ContractListEntry> = Vec::new();
    for entry in walker.flatten() {
        if !entry.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&entry) else {
            continue;
        };
        let Ok(value) = serde_saphyr::from_str::<Value>(&content) else {
            continue;
        };
        let Some(format) = top_level_format(&value) else {
            continue;
        };
        let info = value.get("info");
        out.push(ContractListEntry {
            path: relative_path_string(&entry, project_dir),
            format,
            title: info.and_then(|i| i.get("title")).and_then(|v| v.as_str()).map(str::to_string),
            version: info
                .and_then(|i| i.get("version"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            x_specify_id: info
                .and_then(|i| i.get("x-specify-id"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn top_level_format(value: &Value) -> Option<&'static str> {
    let obj = value.as_object()?;
    if obj.contains_key("openapi") {
        Some("openapi")
    } else if obj.contains_key("asyncapi") {
        Some("asyncapi")
    } else {
        None
    }
}

fn relative_path_string(path: &Path, project_dir: &Path) -> String {
    path.strip_prefix(project_dir).unwrap_or(path).to_string_lossy().into_owned()
}

fn print_list_table(entries: &[ContractListEntry]) {
    let mut path_w = "PATH".len();
    let mut format_w = "FORMAT".len();
    let mut title_w = "TITLE".len();
    let mut version_w = "VERSION".len();
    for e in entries {
        path_w = path_w.max(e.path.len());
        format_w = format_w.max(e.format.len());
        title_w = title_w.max(e.title.as_deref().unwrap_or("-").len());
        version_w = version_w.max(e.version.as_deref().unwrap_or("-").len());
    }
    println!(
        "{:<path_w$}  {:<format_w$}  {:<title_w$}  {:<version_w$}  X-SPECIFY-ID",
        "PATH", "FORMAT", "TITLE", "VERSION"
    );
    for e in entries {
        println!(
            "{:<path_w$}  {:<format_w$}  {:<title_w$}  {:<version_w$}  {}",
            e.path,
            e.format,
            e.title.as_deref().unwrap_or("-"),
            e.version.as_deref().unwrap_or("-"),
            e.x_specify_id.as_deref().unwrap_or("-"),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use specify::ProjectConfig;
    use tempfile::TempDir;

    use super::*;

    fn ctx_for(tmp: &TempDir) -> CommandContext {
        let specify_dir = tmp.path().join(".specify");
        fs::create_dir_all(&specify_dir).expect("create .specify");
        let cfg = ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            schema: "omnia".to_string(),
            specify_version: None,
            rules: BTreeMap::new(),
            hub: false,
        };
        let cfg_path = ProjectConfig::config_path(tmp.path());
        fs::write(&cfg_path, serde_saphyr::to_string(&cfg).expect("serialise")).expect("write");
        CommandContext {
            format: OutputFormat::Json,
            project_dir: tmp.path().to_path_buf(),
            config: cfg,
        }
    }

    fn write_contract(tmp: &TempDir, rel: &str, body: &str) -> PathBuf {
        let path = tmp.path().join("contracts").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn list_no_contracts_dir_is_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        let result = list_contracts(&ctx).expect("list ok");
        assert_eq!(result, CliResult::Success);
    }

    #[test]
    fn list_collects_top_level_entries_only() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
        );
        write_contract(
            &tmp,
            "messages/orders.yaml",
            "asyncapi: '3.0.0'\ninfo:\n  title: Orders\n  version: 1.2.3\n",
        );
        write_contract(&tmp, "schemas/user.yaml", "$id: urn:test\ntitle: User\ndescription: x\n");

        let entries = collect_entries(&ctx.contracts_dir(), &ctx.project_dir);
        assert_eq!(entries.len(), 2, "schemas/ excluded");
        assert!(entries.iter().any(|e| e.format == "openapi"));
        assert!(entries.iter().any(|e| e.format == "asyncapi"));
        let openapi = entries.iter().find(|e| e.format == "openapi").unwrap();
        assert_eq!(openapi.title.as_deref(), Some("User API"));
        assert_eq!(openapi.version.as_deref(), Some("1.0.0"));
        assert_eq!(openapi.x_specify_id.as_deref(), Some("user-api"));
        let asyncapi = entries.iter().find(|e| e.format == "asyncapi").unwrap();
        assert!(asyncapi.x_specify_id.is_none());
    }

    #[test]
    fn validate_no_contracts_dir_is_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        let result = validate_contracts(&ctx).expect("validate ok");
        assert_eq!(result, CliResult::Success);
    }

    #[test]
    fn validate_clean_baseline_is_success() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\n",
        );
        let result = validate_contracts(&ctx).expect("validate ok");
        assert_eq!(result, CliResult::Success);
    }

    #[test]
    fn validate_bad_semver_returns_validation_failed() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        write_contract(
            &tmp,
            "http/user-api.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 2024-01-15\n",
        );
        let result = validate_contracts(&ctx).expect("validate ran");
        assert_eq!(result, CliResult::ValidationFailed);
    }

    #[test]
    fn validate_duplicate_id_returns_validation_failed() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_for(&tmp);
        write_contract(
            &tmp,
            "http/a.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: A\n  version: 1.0.0\n  x-specify-id: shared\n",
        );
        write_contract(
            &tmp,
            "http/b.yaml",
            "openapi: '3.1.0'\ninfo:\n  title: B\n  version: 1.0.0\n  x-specify-id: shared\n",
        );
        let result = validate_contracts(&ctx).expect("validate ran");
        assert_eq!(result, CliResult::ValidationFailed);
    }
}
