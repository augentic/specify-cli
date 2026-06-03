//! `vectis verify` subcommand — declared-vs-present platform shell verification.
//!
//! Authority is `project.yaml.platforms` (the typed platform set, not
//! per-slice proposals). The engine inspects on-disk shell trees and
//! reports which declared platforms are present.
//!
//! Two modes:
//!
//! - **detect** (plan-time): returns the set of declared-but-absent
//!   platforms as a JSON array for bootstrap-slice insertion.
//! - **verify** (build/lint): emits `diagnostic.schema.json`-shaped
//!   findings and exits non-zero on any miss for a supported platform.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, ValueEnum};
use serde_json::Value;

use crate::VectisError;
use crate::render_json as render_value;
use crate::validate::find_project_root;

/// Arguments accepted by `vectis verify`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct VerifyArgs {
    /// Verification mode to run.
    #[arg(long, value_enum)]
    pub mode: VerifyMode,

    /// Project directory. Falls back to `PROJECT_DIR` env, then CWD walk-up.
    pub path: Option<PathBuf>,
}

/// Verification mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum VerifyMode {
    /// Plan-time: return the set of declared-but-absent platforms.
    Detect,
    /// Build/lint-time: emit diagnostic findings, exit non-zero on miss.
    Verify,
}

/// Per-platform status entry in the verify report.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformStatus {
    platform: String,
    declared: bool,
    present: bool,
}

/// Known platform strings that have on-disk shell interpretations today.
const SUPPORTED_PLATFORMS: &[&str] = &["core", "ios", "android"];

/// Run the verify engine.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when `project.yaml` is
/// missing or unparseable, or lacks a `platforms` field.
pub fn run(args: &VerifyArgs) -> Result<Value, VectisError> {
    let project_root = resolve_project_root(args.path.as_deref())?;
    let platforms = load_platforms(&project_root)?;

    let statuses: Vec<PlatformStatus> =
        platforms.iter().map(|p| check_platform(p, &project_root)).collect();

    match args.mode {
        VerifyMode::Detect => Ok(render_detect(&statuses)),
        VerifyMode::Verify => Ok(render_verify(&statuses, &project_root)),
    }
}

/// Render a `(success | error)` result as pretty-printed JSON with
/// exit code. Detect mode always exits 0; verify mode exits 1 when
/// any supported declared platform is missing.
#[must_use]
pub fn render_json(outcome: Result<Value, VectisError>) -> (String, u8) {
    match outcome {
        Ok(value) => {
            let code = verify_exit_code(&value);
            (render_value(&value), code)
        }
        Err(err) => {
            let exit_code = err.exit_code();
            let Value::Object(mut payload) = err.to_json() else {
                unreachable!("VectisError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(Value::from(exit_code));
            (render_value(&Value::Object(payload)), exit_code)
        }
    }
}

/// Compute the exit code for a verify payload.
///
/// Detect mode always returns 0 (the consumer reads the `missing`
/// array). Verify mode returns 1 when findings are present, 0
/// otherwise.
#[must_use]
fn verify_exit_code(value: &Value) -> u8 {
    if value.get("mode").and_then(Value::as_str) == Some("detect") {
        return 0;
    }
    let has_findings = value
        .get("findings")
        .and_then(Value::as_array)
        .is_some_and(|arr| arr.iter().any(|f| f.get("severity").and_then(Value::as_str) == Some("error")));
    u8::from(has_findings)
}

// ── project.yaml loading ───────────────────────────────────────────

fn resolve_project_root(path: Option<&Path>) -> Result<PathBuf, VectisError> {
    if let Some(p) = path {
        return Ok(p.to_path_buf());
    }
    if let Some(project_dir) = std::env::var_os("PROJECT_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(project_dir));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_project_root(&cwd).ok_or_else(|| VectisError::InvalidProject {
        message: "cannot locate project root (no .specify/ directory found)".into(),
    })
}

fn load_platforms(project_root: &Path) -> Result<Vec<String>, VectisError> {
    let config_path = project_root.join("project.yaml");
    let source = std::fs::read_to_string(&config_path).map_err(|_| VectisError::InvalidProject {
        message: format!(
            "project.yaml not readable at {}",
            config_path.display()
        ),
    })?;
    let doc: Value =
        serde_saphyr::from_str(&source).map_err(|err| VectisError::InvalidProject {
            message: format!("project.yaml is not valid YAML: {err}"),
        })?;
    let platforms = doc
        .get("platforms")
        .and_then(Value::as_array)
        .ok_or_else(|| VectisError::InvalidProject {
            message: "project.yaml does not declare a `platforms` array".into(),
        })?;
    platforms
        .iter()
        .map(|v| {
            v.as_str()
                .map(String::from)
                .ok_or_else(|| VectisError::InvalidProject {
                    message: "project.yaml `platforms` array contains a non-string entry".into(),
                })
        })
        .collect()
}

// ── per-platform shell detection ───────────────────────────────────

fn check_platform(platform: &str, project_root: &Path) -> PlatformStatus {
    let present = match platform {
        "core" => detect_core(project_root),
        "ios" => detect_ios(project_root),
        "android" => detect_android(project_root),
        _ => true, // web/desktop — no on-disk interpretation yet; treated as present
    };
    PlatformStatus {
        platform: platform.to_string(),
        declared: true,
        present,
    }
}

/// `core` -> `shared/src/app.rs` exists.
fn detect_core(project_root: &Path) -> bool {
    project_root.join("shared/src/app.rs").is_file()
}

/// `ios` -> `iOS/` has >= 1 `.swift` file.
fn detect_ios(project_root: &Path) -> bool {
    let ios_dir = project_root.join("iOS");
    if !ios_dir.is_dir() {
        return false;
    }
    has_files_with_extension(&ios_dir, "swift")
}

/// `android` -> `Android/` has >= 1 `.kt` file.
fn detect_android(project_root: &Path) -> bool {
    let android_dir = project_root.join("Android");
    if !android_dir.is_dir() {
        return false;
    }
    has_files_with_extension(&android_dir, "kt")
}

fn has_files_with_extension(dir: &Path, ext: &str) -> bool {
    walk_dir_recursive(dir).iter().any(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
}

fn walk_dir_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_dir_recursive(&path));
        } else {
            out.push(path);
        }
    }
    out
}

// ── output rendering ───────────────────────────────────────────────

fn is_supported(platform: &str) -> bool {
    SUPPORTED_PLATFORMS.contains(&platform)
}

fn render_detect(statuses: &[PlatformStatus]) -> Value {
    let missing: Vec<Value> = statuses
        .iter()
        .filter(|s| !s.present && is_supported(&s.platform))
        .map(|s| Value::String(s.platform.clone()))
        .collect();

    let info_findings: Vec<Value> = statuses
        .iter()
        .filter(|s| !is_supported(&s.platform))
        .map(|s| {
            serde_json::json!({
                "platform": s.platform,
                "id": "platform-not-yet-supported",
                "severity": "info",
                "message": format!(
                    "platform `{}` is accepted but has no on-disk interpretation yet",
                    s.platform,
                ),
            })
        })
        .collect();

    let entries: Vec<Value> = statuses
        .iter()
        .map(|s| {
            serde_json::json!({
                "platform": s.platform,
                "declared": s.declared,
                "present": s.present,
            })
        })
        .collect();

    serde_json::json!({
        "mode": "detect",
        "project-root": "",
        "platforms": entries,
        "missing": missing,
        "info": info_findings,
    })
}

fn render_verify(statuses: &[PlatformStatus], project_root: &Path) -> Value {
    let mut findings: Vec<Value> = Vec::new();

    for status in statuses {
        if !is_supported(&status.platform) {
            findings.push(serde_json::json!({
                "id": "platform-not-yet-supported",
                "severity": "info",
                "source": "deterministic",
                "message": format!(
                    "platform `{}` is accepted but has no on-disk interpretation yet",
                    status.platform,
                ),
            }));
            continue;
        }
        if !status.present {
            findings.push(serde_json::json!({
                "id": "platform-shell-missing",
                "severity": "error",
                "source": "deterministic",
                "message": format!(
                    "declared platform `{}` has no shell tree under `{}`",
                    status.platform,
                    project_root.display(),
                ),
            }));
        }
    }

    serde_json::json!({
        "mode": "verify",
        "project-root": project_root.display().to_string(),
        "findings": findings,
    })
}
