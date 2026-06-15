//! `vectis materialize` subcommand — canonical-to-export asset conversion.
//!
//! Phase 2 (RFC-46) converts designer-owned `source:` files into per-platform
//! exports under `design-system/assets/exports/<platform>/`.

pub mod icons;
pub mod paths;
mod svg;

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::validate::engine::resolve_default_path_with_root;
use crate::validate::{ValidateMode, find_project_root};
use crate::{VectisError, render_json as render_value};

use icons::materialize_icon_vectors;

/// Nested targets under `vectis materialize`.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum MaterializeCommand {
    /// Convert canonical asset masters into per-platform exports.
    Assets(AssetsArgs),
}

/// Arguments for `vectis materialize assets`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct AssetsArgs {
    /// Path to `assets.yaml`. Defaults to the design-system cascade.
    pub path: Option<PathBuf>,

    /// Comma-separated platform filter (`ios`, `android`). Defaults to both.
    #[arg(long, value_delimiter = ',')]
    pub platform: Option<Vec<String>>,

    /// Report planned writes without creating files or auto-writing pins.
    #[arg(long)]
    pub dry_run: bool,
}

/// Dispatch a parsed [`MaterializeCommand`].
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved `assets.yaml`
/// is missing or unreadable, or when `--platform` carries an unknown token.
pub fn run(command: &MaterializeCommand) -> Result<Value, VectisError> {
    match command {
        MaterializeCommand::Assets(args) => run_assets(args),
    }
}

/// Render a materialize outcome as pretty-printed JSON and exit code.
#[must_use]
pub fn render_json(outcome: Result<Value, VectisError>) -> (String, u8) {
    match outcome {
        Ok(value) => {
            let code = materialize_exit_code(&value);
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

/// Exit non-zero when the summary carries conversion errors.
#[must_use]
pub fn materialize_exit_code(value: &Value) -> u8 {
    u8::from(value.get("errors").and_then(Value::as_array).is_some_and(|arr| !arr.is_empty()))
}

fn run_assets(args: &AssetsArgs) -> Result<Value, VectisError> {
    let path = resolve_assets_path(args.path.as_deref());
    if !path.is_file() {
        return Err(VectisError::InvalidProject {
            message: format!("assets.yaml not readable at {}", path.display()),
        });
    }

    let platforms = resolve_platform_filter(args.platform.as_deref())?;
    let source = std::fs::read_to_string(&path).map_err(VectisError::from)?;

    let mut materialized = Vec::new();
    let mut skipped_pins = Vec::new();
    let mut errors = Vec::new();

    let instance = match serde_saphyr::from_str::<Value>(&source) {
        Ok(value) => value,
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
            return Ok(build_summary(
                &path,
                args.dry_run,
                &platforms,
                &materialized,
                &skipped_pins,
                &errors,
            ));
        }
    };

    if let Some(assets) = instance.get("assets").and_then(Value::as_object) {
        let assets_dir = path.parent().unwrap_or_else(|| Path::new("."));
        materialize_icon_vectors(
            assets_dir,
            assets,
            &platforms,
            args.dry_run,
            &mut materialized,
            &mut skipped_pins,
            &mut errors,
        );
    }

    Ok(build_summary(
        &path,
        args.dry_run,
        &platforms,
        &materialized,
        &skipped_pins,
        &errors,
    ))
}

fn resolve_assets_path(path: Option<&Path>) -> PathBuf {
    if let Some(p) = path {
        return p.to_path_buf();
    }
    let root = materialize_project_root();
    resolve_default_path_with_root(ValidateMode::Assets, &root)
}

fn materialize_project_root() -> PathBuf {
    if let Some(project_dir) = std::env::var_os("PROJECT_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(project_dir);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_project_root(&cwd).unwrap_or(cwd)
}

fn resolve_platform_filter(platforms: Option<&[String]>) -> Result<Vec<String>, VectisError> {
    let Some(tokens) = platforms else {
        return Ok(vec!["ios".into(), "android".into()]);
    };

    if tokens.is_empty() {
        return Ok(vec!["ios".into(), "android".into()]);
    }

    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let normalized = token.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if normalized != "ios" && normalized != "android" {
            return Err(VectisError::InvalidProject {
                message: format!("unknown platform filter {token:?} (expected ios and/or android)"),
            });
        }
        if !out.iter().any(|existing| existing == &normalized) {
            out.push(normalized);
        }
    }

    if out.is_empty() { Ok(vec!["ios".into(), "android".into()]) } else { Ok(out) }
}

fn build_summary(
    path: &Path, dry_run: bool, platforms: &[String], materialized: &[Value],
    skipped_pins: &[Value], errors: &[Value],
) -> Value {
    json!({
        "command": "materialize assets",
        "path": path.display().to_string(),
        "dry_run": dry_run,
        "platforms": platforms,
        "materialized": materialized,
        "skipped_pins": skipped_pins,
        "errors": errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_follows_errors_array() {
        let clean = build_summary(Path::new("assets.yaml"), false, &["ios".into()], &[], &[], &[]);
        assert_eq!(materialize_exit_code(&clean), 0);

        let failed = build_summary(
            Path::new("assets.yaml"),
            false,
            &["ios".into()],
            &[],
            &[],
            &[json!({ "path": "/assets/foo", "message": "decode failed" })],
        );
        assert_eq!(materialize_exit_code(&failed), 1);
    }

    #[test]
    fn platform_filter_defaults_and_dedupes() {
        let both = resolve_platform_filter(None).expect("default");
        assert_eq!(both, vec!["ios", "android"]);

        let ios_only =
            resolve_platform_filter(Some(&["ios".into(), "ios".into()])).expect("dedupe");
        assert_eq!(ios_only, vec!["ios"]);
    }

    #[test]
    fn platform_filter_rejects_unknown() {
        let err = resolve_platform_filter(Some(&["web".into()])).unwrap_err();
        assert!(matches!(err, VectisError::InvalidProject { .. }));
    }
}
