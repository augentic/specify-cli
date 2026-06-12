//! `validate composition` — schema validation, structural-identity,
//! sibling auto-invoke (tokens / assets), and cross-artifact reference
//! resolution. The structural-identity engine lives in
//! [`structural_identity`] (shared with `validate layout` and the
//! `infer` verb); reference resolution in [`refs`]; the component
//! catalog contract in [`catalog`]; the typed finding they all emit in
//! [`finding`].

mod catalog;
mod finding;
mod refs;
pub(crate) mod structural_identity;

use std::path::Path;

use serde_json::{Value, json};

pub(crate) use self::finding::Finding;
pub(crate) use self::structural_identity::{
    Skeleton, build_group_skeleton, check_structural_identity, fingerprint, skeleton_to_json,
};
use super::paths::{discover_artifact, discover_catalog, resolve_default_path};
use super::run_inner;
use super::shared::{composition_validator, parse_yaml_file};
use crate::validate::ValidateMode;
use crate::validate::error::VectisError;

/// Validate `composition.yaml` as the lifecycle artifact.
///
/// The mode performs five checks:
///
/// 1. **Schema validation** against the embedded composition schema
///    (shared with `layout` mode — one schema, two runtime layers).
/// 2. **Structural-identity** for `component:` directives, reusing the
///    [`check_structural_identity`] engine. The walk covers both
///    `screens` (baseline shape) and `delta.added` / `delta.modified`
///    (change-local shape) so instances introduced or modified in a
///    delta participate in identity checks together.
/// 3. **Auto-invoke** sibling `tokens.yaml` / `assets.yaml` modes when
///    the files exist; their envelopes are folded into
///    `results: [{ mode, report }]` (the same shape `validate all`
///    emits).
/// 4. **Cross-artifact reference resolution** — token references
///    (`color`, `background`, `border.color`, `elevation`, plus
///    string-valued `gap` / `padding` / `padding.<side>` /
///    `corner_radius`) and asset references (`image.name`,
///    `icon.name`, `icon-button.icon`, `fab.icon`) are resolved
///    against the discovered manifests' id sets. Unresolved
///    references become composition-mode errors with
///    JSON-Pointer-shaped paths.
/// 5. **Catalog cross-reference** (component catalog contract) — when
///    `.specify/design-system/components.yaml` is discoverable,
///    every `component: <slug>` in the composition must resolve to
///    a `confirmed` catalog entry (rejected or missing entries are
///    errors), and every confirmed catalog entry should have ≥1
///    `component:` reference (warning, not error). Absent catalogs
///    are silently skipped.
///
/// `maps_to` / `bind` / `event` / overlay `trigger` / navigation
/// target full resolution is deferred. The schema's regex patterns
/// shape-check these fields at parse time, but resolution against
/// `design.md` / `specs/` belongs to a follow-on rule.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when an explicitly supplied
/// file is unreadable, and [`VectisError::Internal`] if the embedded
/// schema fails to compile. With no `[path]` and no discoverable
/// `composition.yaml` (a core-only project has none by design), the
/// mode exits cleanly with a `skipped` envelope instead of erroring.
pub(super) fn validate(path: Option<&Path>) -> Result<Value, VectisError> {
    let target = path.map_or_else(
        || resolve_default_path(ValidateMode::Composition),
        std::path::Path::to_path_buf,
    );

    if path.is_none() && !target.exists() {
        return Ok(json!({
            "mode": "composition",
            "status": "skipped",
            "reason": format!("no composition.yaml discoverable (looked at {}); core-only projects carry none", target.display()),
            "errors": [],
            "warnings": [],
        }));
    }

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("composition.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = composition_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(Finding::new(err.instance_path().to_string(), err.to_string()));
            }

            // Structural identity walks both shapes. The schema's
            // `oneOf` ensures only one of `screens` / `delta` is
            // present at a time; the `if let` guards keep the call
            // site shape-agnostic.
            if let Some(screens) = instance.get("screens") {
                check_structural_identity(screens, "/screens", &mut errors);
            }
            if let Some(delta) = instance.get("delta") {
                check_structural_identity(delta, "/delta", &mut errors);
            }

            // Sibling discovery + auto-invoke. `tokens` runs before
            // `assets` so the envelope's `results` array matches the
            // dispatch order operators see in `validate all`.
            let tokens_sibling = discover_artifact(&target, ValidateMode::Tokens);
            let assets_sibling = discover_artifact(&target, ValidateMode::Assets);

            if let Some(ref tokens_path) = tokens_sibling {
                let report = run_inner(ValidateMode::Tokens, tokens_path)?;
                results.push(json!({
                    "mode": ValidateMode::Tokens.as_str(),
                    "report": report,
                }));
            }
            if let Some(ref assets_path) = assets_sibling {
                let report = run_inner(ValidateMode::Assets, assets_path)?;
                results.push(json!({
                    "mode": ValidateMode::Assets.as_str(),
                    "report": report,
                }));
            }

            // Cross-artifact reference resolution. Token / asset
            // walks run against the *content* of the sibling
            // manifests, separately from the auto-invoked structural
            // validation above. This is the layer that catches
            // "composition references a name that does not exist in
            // tokens.yaml / assets.yaml"; the auto-invoke catches
            // "tokens.yaml / assets.yaml is itself structurally
            // broken".
            if let Some(ref tokens_path) = tokens_sibling
                && let Some(tokens_value) = parse_yaml_file(tokens_path)
            {
                refs::resolve_token_references(&instance, &tokens_value, &mut errors);
            }
            if let Some(ref assets_path) = assets_sibling
                && let Some(assets_value) = parse_yaml_file(assets_path)
            {
                refs::resolve_asset_references(&instance, &assets_value, &mut errors);
            }

            // Catalog cross-reference (component catalog contract). When the
            // project-level component catalog exists, every
            // `component: <slug>` in the composition must resolve to
            // a confirmed entry and every confirmed entry should be
            // referenced at least once.
            //
            // Unlike tokens/assets (which are auto-invoked and report
            // their own parse errors), the catalog has no sibling
            // validator — report read/parse failures explicitly.
            if let Some(ref catalog_path) = discover_catalog(&target) {
                match catalog::parse_catalog_file(catalog_path) {
                    Ok(catalog_value) => {
                        catalog::check_catalog_cross_references(
                            &instance,
                            &catalog_value,
                            &mut errors,
                            &mut warnings,
                        );
                    }
                    Err(message) => {
                        errors.push(Finding::new("", message));
                    }
                }
            }
        }
        Err(err) => {
            errors.push(Finding::new("", format!("invalid YAML: {err}")));
        }
    }

    let mut envelope = json!({
        "mode": ValidateMode::Composition.as_str(),
        "path": target.display().to_string(),
        "errors": finding::to_values(errors),
        "warnings": finding::to_values(warnings),
    });
    // Only emit `results` when we actually folded something in.
    if !results.is_empty()
        && let Value::Object(ref mut map) = envelope
    {
        map.insert("results".to_string(), Value::Array(results));
    }

    Ok(envelope)
}
