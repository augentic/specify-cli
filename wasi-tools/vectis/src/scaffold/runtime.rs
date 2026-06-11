//! Per-target plan construction and the on-disk write step.
//!
//! Pure relocation from `scaffold.rs`: deterministic plan derivation
//! per scaffold target (core / iOS / Android), all-or-nothing collision
//! check, and the final `fs::write` pass under `PROJECT_DIR`. Public
//! surface is unchanged; the parent re-exports `plan_core`, `plan_ios`,
//! `plan_android`, `write_plan`, and `validate_app_name`.

use std::fs;
use std::path::Path;

use super::templates::{Params, plan_assembly, registry};
use super::{Capability, PlannedFile, ScaffoldError, ScaffoldPlan, Versions};

/// Render and plan the core scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_core(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let files = plan_assembly(registry::core::ENTRIES, &params, caps);
    Ok(scaffold_plan("core", app_name, android_package, caps, files))
}

/// Render and plan the iOS shell scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_ios(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let files = plan_assembly(registry::ios::ENTRIES, &params, caps);
    Ok(scaffold_plan("ios", app_name, android_package, caps, files))
}

/// Render and plan the Android shell scaffold.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name is invalid.
pub fn plan_android(
    app_name: &str, android_package: &str, caps: &[Capability], versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    validate_app_name(app_name)?;
    let params = build_params(app_name, android_package, versions);
    let files = plan_assembly(registry::android::ENTRIES, &params, caps);
    Ok(scaffold_plan("android", app_name, android_package, caps, files))
}

/// Write a complete plan under `project_dir` after checking every collision.
///
/// An existing `.gitignore` is the one tolerated collision: `specify
/// init` writes a root `.gitignore` in every project, so the bootstrap
/// path (`scaffold core` against an initialised repo) merges the
/// template's missing lines into it instead of refusing.
///
/// # Errors
///
/// Returns [`ScaffoldError`] if any other target already exists or a write fails.
pub fn write_plan(project_dir: &Path, plan: &ScaffoldPlan) -> Result<(), ScaffoldError> {
    match plan.target {
        "ios" => refuse_existing_root(project_dir, "iOS", "iOS")?,
        "android" => refuse_existing_root(project_dir, "Android", "Android")?,
        _ => {}
    }

    for file in &plan.files {
        let target = project_dir.join(&file.relative_path);
        if target.exists() && !is_gitignore(&file.relative_path) {
            return Err(ScaffoldError::InvalidProject {
                message: format!(
                    "refusing to overwrite existing file at {} (run `vectis scaffold` against an empty target)",
                    target.display()
                ),
            });
        }
    }

    if !project_dir.exists() {
        fs::create_dir_all(project_dir)?;
    }
    for file in &plan.files {
        let target = project_dir.join(&file.relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if target.exists() && is_gitignore(&file.relative_path) {
            merge_gitignore(&target, &file.contents)?;
        } else {
            fs::write(target, &file.contents)?;
        }
    }
    Ok(())
}

fn is_gitignore(relative_path: &str) -> bool {
    Path::new(relative_path).file_name().is_some_and(|name| name == ".gitignore")
}

/// Append the template's missing lines to an existing `.gitignore`.
/// Line-set comparison keeps the merge idempotent across scaffold
/// re-runs; existing operator content is never rewritten.
pub(super) fn merge_gitignore(target: &Path, template: &str) -> Result<(), ScaffoldError> {
    let existing = fs::read_to_string(target)?;
    let present: std::collections::BTreeSet<&str> =
        existing.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let missing: Vec<&str> = template
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !present.contains(trimmed)
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut merged = existing;
    if !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged.push_str("\n# Vectis scaffold\n");
    for line in missing {
        merged.push_str(line);
        merged.push('\n');
    }
    fs::write(target, merged)?;
    Ok(())
}

/// Validate `app_name` as `PascalCase` ASCII.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when the app name cannot be used as a generated
/// Rust/Swift/Kotlin identifier segment.
pub fn validate_app_name(app_name: &str) -> Result<(), ScaffoldError> {
    let mut chars = app_name.chars();
    let first = chars.next().ok_or_else(|| ScaffoldError::InvalidProject {
        message: "app name must not be empty".into(),
    })?;
    if !first.is_ascii_uppercase() {
        return Err(ScaffoldError::InvalidProject {
            message: format!(
                "app name {app_name:?} must start with an ASCII uppercase letter (PascalCase, e.g. \"Counter\")"
            ),
        });
    }
    for c in chars {
        if !c.is_ascii_alphanumeric() {
            return Err(ScaffoldError::InvalidProject {
                message: format!(
                    "app name {app_name:?} must contain only ASCII alphanumeric characters (PascalCase)"
                ),
            });
        }
    }
    Ok(())
}

fn scaffold_plan(
    target: &'static str, app_name: &str, android_package: &str, caps: &[Capability],
    files: Vec<PlannedFile>,
) -> ScaffoldPlan {
    ScaffoldPlan {
        target,
        app_name: app_name.to_string(),
        android_package: android_package.to_string(),
        capabilities: caps.iter().map(|c| c.marker_tag().to_string()).collect(),
        files,
    }
}

fn build_params(app_name: &str, android_package: &str, versions: &Versions) -> Params {
    Params {
        app_name: app_name.to_string(),
        app_struct: app_name.to_string(),
        app_name_lower: app_name.to_lowercase(),
        android_package: android_package.to_string(),
        crux_core_version: versions.crux.core.clone(),
        crux_http_version: versions.crux.http.clone(),
        crux_kv_version: versions.crux.kv.clone(),
        crux_time_version: versions.crux.time.clone(),
        crux_platform_version: versions.crux.platform.clone(),
        facet_version: versions.crux.facet.clone(),
        serde_version: versions.crux.serde.clone(),
        uniffi_version: versions.crux.uniffi.clone(),
        agp_version: versions.android.agp.clone(),
        kotlin_version: versions.android.kotlin.clone(),
        compose_bom_version: versions.android.compose_bom.clone(),
        ktor_version: versions.android.ktor.clone(),
        koin_version: versions.android.koin.clone(),
        android_ndk_version: versions
            .android
            .ndk
            .clone()
            .unwrap_or_else(|| "__ANDROID_NDK_VERSION__".to_string()),
    }
}

fn refuse_existing_root(
    project_dir: &Path, root: &str, display_name: &str,
) -> Result<(), ScaffoldError> {
    let shell_root = project_dir.join(root);
    if shell_root.exists() {
        return Err(ScaffoldError::InvalidProject {
            message: format!(
                "refusing to overwrite existing {display_name} shell at {} (delete it first or use the host add-shell workflow)",
                shell_root.display()
            ),
        });
    }
    Ok(())
}
