//! `validate layout` — schema validation plus unwired-subset
//! enforcement (forbidden wiring keys + `delta`-shape rejection) plus
//! shared structural-identity checks.

use std::path::Path;

use serde_json::{Value, json};

use super::composition::check_structural_identity;
use super::paths::resolve_default_path;
use super::shared::{composition_validator, escape_pointer_token};
use crate::error::VectisError;
use crate::{CommandOutcome, ValidateMode};

/// Validate `layout.yaml` as the unwired subset of the patched
/// composition schema.
///
/// The mode performs three checks:
///
/// 1. **Schema validation** against the embedded composition schema.
///    The schema permits both `screens` and `delta` shapes; the
///    unwired-subset check below rejects `delta`-shaped layout
///    documents.
/// 2. **Unwired-subset enforcement** — reject `delta:` and any
///    occurrence of define-owned wiring keys (`maps_to`, `bind`,
///    `event`, `error`, overlay `trigger`, conditional visual
///    `*-when` keys). The walker descends only the `screens` sub-tree
///    (the only place where wiring keys can appear in a valid
///    composition document); other top-level keys (`provenance`,
///    `version`, `custom_items`) carry no wiring. Bare `when:` (the
///    required `stateEntry.when` from the schema) is *not* a `*-when`
///    key and is preserved.
/// 3. **Structural-identity** for `component:` directives — every
///    group carrying the same `component: <slug>` MUST share the same
///    skeleton. The engine ignores leaf wiring values (`bind`,
///    `event`, `error`, free text content, token / asset references)
///    and `*-when` *condition values*, but is sensitive to `*-when`
///    key *presence*. Per-instance `platforms.*` overrides are exempt
///    from base-skeleton match.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved file is
/// unreadable, and [`VectisError::Internal`] if the embedded schema
/// fails to compile.
pub(super) fn validate(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path
        .map_or_else(|| resolve_default_path(ValidateMode::Layout), std::path::Path::to_path_buf);

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("layout.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    let warnings: Vec<Value> = Vec::new();

    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = composition_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
            }

            if instance.get("delta").is_some() {
                errors.push(json!({
                    "path": "/delta",
                    "message": "layout.yaml MUST NOT use the `delta` shape (unwired-subset rule); only `screens` documents are permitted. Use composition.yaml for change-local delta artifacts.",
                }));
            }

            // Walk the `screens` sub-tree for forbidden wiring keys
            // and `component:` directive instances. Both walks are
            // scoped to `screens` because (a) other top-level keys
            // never carry wiring per the schema, and (b) keeping the
            // scope tight avoids descending into a `delta:` sub-tree
            // (which would surface noisy redundant wiring-key errors
            // after we've already rejected the shape itself).
            if let Some(screens) = instance.get("screens") {
                walk_unwired(screens, "/screens", &mut errors);
                check_structural_identity(screens, "/screens", &mut errors);
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::Layout.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    })))
}

/// Walk a YAML sub-tree (typically the `screens` value) and append an
/// error for every define-owned wiring key the unwired subset forbids:
///
/// - `maps_to` (screen route binding).
/// - `bind` (field binding on items).
/// - `event` (event handler on items).
/// - `error` (validation-error string on items).
/// - `trigger` (overlay trigger).
/// - any key matching the pattern `*-when` (e.g. `strikethrough-when`,
///   `visible-when`). The bare `when:` key (`stateEntry.when`) is part
///   of the unwired subset and explicitly preserved.
///
/// The walker recurses through every nested object and array so a
/// `bind:` buried in `screens.<name>.body.list.item[0].group.items[0]
/// .checkbox` is reported with a precise JSON Pointer. Property
/// *values* (e.g. `style: plain`, `align: center`) never trigger a
/// finding — the walker matches keys, not strings.
fn walk_unwired(node: &Value, json_path: &str, errors: &mut Vec<Value>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                if let Some(reason) = forbidden_wiring_key(key) {
                    errors.push(json!({
                        "path": child_path,
                        "message": format!(
                            "{reason} -- remove this key from layout.yaml (unwired-subset rule); wiring is added by /spec:define when it produces composition.yaml"
                        ),
                    }));
                }
                walk_unwired(val, &child_path, errors);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_unwired(v, &format!("{json_path}/{i}"), errors);
            }
        }
        _ => {}
    }
}

/// Classify `key` as a forbidden define-owned wiring key. Returns the
/// human-readable reason string when the key is forbidden, or `None`
/// when the key is allowed in unwired layout documents.
///
/// Edge cases:
/// - `when` (bare) is the required `stateEntry.when` field; allowed.
/// - `<x>-when` patterns require both the hyphen and the `-when`
///   suffix, so `when` alone never matches. The minimum kebab-case
///   form is at least 6 characters (`a-when`) which the length guard
///   enforces.
fn forbidden_wiring_key(key: &str) -> Option<&'static str> {
    match key {
        "maps_to" => Some("`maps_to` is define-owned screen-to-route wiring"),
        "bind" => Some("`bind` is define-owned field binding"),
        "event" => Some("`event` is define-owned event wiring"),
        "error" => Some("`error` is define-owned validation-error wiring"),
        "trigger" => Some("overlay `trigger` is define-owned"),
        _ if key.ends_with("-when") && key.len() > 5 => {
            Some("conditional visual `*-when` keys are define-owned wiring")
        }
        _ => None,
    }
}
