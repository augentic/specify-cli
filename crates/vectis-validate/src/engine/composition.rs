//! `validate composition` — schema validation, structural-identity,
//! sibling auto-invoke (tokens / assets), and cross-artifact reference
//! resolution. The structural-identity engine is shared with
//! `validate layout`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::assets::collect_asset_references;
use super::paths::{discover_artifact, resolve_default_path};
use super::run_inner;
use super::shared::{composition_validator, escape_pointer_token, parse_yaml_file};
use crate::error::VectisError;
use crate::{CommandOutcome, ValidateMode};

/// Validate `composition.yaml` as the lifecycle artifact.
///
/// The mode performs four checks:
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
///
/// `maps_to` / `bind` / `event` / overlay `trigger` / navigation
/// target full resolution is deferred. The schema's regex patterns
/// shape-check these fields at parse time, but resolution against
/// `design.md` / `specs/` belongs to a follow-on rule.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved file is
/// unreadable, and [`VectisError::Internal`] if the embedded schema
/// fails to compile.
pub(super) fn validate(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let target = path.map_or_else(
        || resolve_default_path(ValidateMode::Composition),
        std::path::Path::to_path_buf,
    );

    let source = std::fs::read_to_string(&target).map_err(|err| VectisError::InvalidProject {
        message: format!("composition.yaml not readable at {}: {err}", target.display()),
    })?;

    let mut errors: Vec<Value> = Vec::new();
    // Composition mode has no warning class in v1; the empty array
    // stays in the envelope so the shape matches the other modes.
    let warnings: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    match serde_saphyr::from_str::<Value>(&source) {
        Ok(instance) => {
            let validator = composition_validator()?;
            for err in validator.iter_errors(&instance) {
                errors.push(json!({
                    "path": err.instance_path().to_string(),
                    "message": err.to_string(),
                }));
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
                resolve_token_references(&instance, &tokens_value, &mut errors);
            }
            if let Some(ref assets_path) = assets_sibling
                && let Some(assets_value) = parse_yaml_file(assets_path)
            {
                resolve_asset_references(&instance, &assets_value, &mut errors);
            }
        }
        Err(err) => {
            errors.push(json!({
                "path": "",
                "message": format!("invalid YAML: {err}"),
            }));
        }
    }

    let mut envelope = json!({
        "mode": ValidateMode::Composition.as_str(),
        "path": target.display().to_string(),
        "errors": errors,
        "warnings": warnings,
    });
    // Only emit `results` when we actually folded something in.
    if !results.is_empty()
        && let Value::Object(ref mut map) = envelope
    {
        map.insert("results".to_string(), Value::Array(results));
    }

    Ok(CommandOutcome::Success(envelope))
}

/// Walk a composition document and append an error for every token
/// reference whose value is not present in `tokens` under the expected
/// category.
///
/// V1 token-ref categories:
///
/// - `color`, `background`, `border.color` → `colors.<name>`
/// - `elevation` (groupProps) → `elevation.<name>`
/// - `gap`, `padding`, `padding.<side>` (when string-valued) →
///   `spacing.<name>`
/// - `corner_radius` (when string-valued) → `cornerRadius.<name>`
///
/// Skipped for v1 (deliberately ambiguous, deferred to a later rule):
///
/// - `style` — the schema declares `style: { type: string }` with no
///   enum; it is a typography ref on `text` items but a presentation
///   enum on `button`/`list`/etc. Without a per-item-kind classifier,
///   autoresolving it generates false positives.
/// - `size.width` / `size.height` — the schema's `sizingValue` only
///   permits `"fill"` / `"hug"` strings, so these never reference
///   tokens.
fn resolve_token_references(composition: &Value, tokens: &Value, errors: &mut Vec<Value>) {
    walk_token_refs(composition, "", tokens, errors);
}

/// Recursive walker driving [`resolve_token_references`]. Matches on
/// the well-known token-bearing keys and recurses through the rest of
/// the document. The category lookup is centralised in
/// [`token_category_for_key`] so the walker stays small.
fn walk_token_refs(node: &Value, json_path: &str, tokens: &Value, errors: &mut Vec<Value>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));

                if let Some(category) = token_category_for_key(key)
                    && let Some(name) = val.as_str()
                {
                    check_token_ref(category, name, &child_path, tokens, errors);
                }

                // `padding` may also be a paddingSpec object. Walk
                // each side as a spacing ref. The string-valued
                // `padding: md` case is already handled above.
                if key == "padding"
                    && let Some(side_map) = val.as_object()
                {
                    for (side, side_val) in side_map {
                        if let Some(name) = side_val.as_str() {
                            let side_path = format!("{child_path}/{}", escape_pointer_token(side));
                            check_token_ref("spacing", name, &side_path, tokens, errors);
                        }
                    }
                }

                walk_token_refs(val, &child_path, tokens, errors);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_token_refs(v, &format!("{json_path}/{i}"), tokens, errors);
            }
        }
        _ => {}
    }
}

/// Map a composition-document key to the `tokens.yaml` category its
/// string value resolves against, or `None` when the key does not
/// carry a deterministic token reference in v1.
const fn token_category_for_key(key: &str) -> Option<&'static str> {
    match key.as_bytes() {
        b"color" | b"background" => Some("colors"),
        b"elevation" => Some("elevation"),
        b"gap" | b"padding" => Some("spacing"),
        b"corner_radius" => Some("cornerRadius"),
        _ => None,
    }
}

/// Resolve `name` against `tokens.<category>` and append an error to
/// `errors` when it is absent. The error message names both the
/// category and the offending name so an operator can fix it without
/// re-reading the manifest.
fn check_token_ref(
    category: &str, name: &str, json_path: &str, tokens: &Value, errors: &mut Vec<Value>,
) {
    let exists =
        tokens.get(category).and_then(Value::as_object).is_some_and(|m| m.contains_key(name));
    if !exists {
        errors.push(json!({
            "path": json_path,
            "message": format!(
                "composition references unknown {category} token `{name}` -- not present in tokens.yaml under `{category}.{name}`",
            ),
        }));
    }
}

/// Walk a composition document and append an error for every static
/// asset reference whose name is not declared under `assets.<id>` in
/// the supplied assets manifest. Reuses [`collect_asset_references`]
/// so the reference shapes (`image.name`, `icon.name`,
/// `icon-button.icon`, `fab.icon`) stay in lock-step between
/// composition mode (this function) and assets mode's own
/// composition-discovery path.
fn resolve_asset_references(composition: &Value, assets: &Value, errors: &mut Vec<Value>) {
    let asset_ids = assets.get("assets").and_then(Value::as_object);
    let refs = collect_asset_references(composition);
    for asset_ref in &refs {
        let exists = asset_ids.is_some_and(|m| m.contains_key(&asset_ref.id));
        if !exists {
            errors.push(json!({
                "path": asset_ref.path,
                "message": format!(
                    "composition references unknown asset id `{}` -- not present in assets.yaml",
                    asset_ref.id,
                ),
            }));
        }
    }
}

/// Recorded `component: <slug>` instance for the structural-identity
/// engine. The `path` is a JSON Pointer that points at the group that
/// bears the directive, so an identity violation can name both halves.
struct ComponentInstance {
    /// Kebab-case component slug declared by the directive.
    slug: String,
    /// Normalised skeleton derived from the group's `items:` array.
    skeleton: Skeleton,
    /// JSON Pointer indicating where this instance's group lives.
    path: String,
    /// `true` when the instance lives inside a
    /// `screens.<name>.platforms.<plat>.*` sub-tree. Platform overrides
    /// MAY diverge from the base skeleton — we collect them but do not
    /// enforce base-equality against them.
    in_platform_override: bool,
}

/// Normalised structural skeleton for a group's children. Keeps just
/// enough information to detect material divergence (item kinds,
/// nested-group nesting, `*-when` key presence) while ignoring leaf
/// wiring values: slug instances MAY differ in `bind`, `event`,
/// `error`, asset / token references, and free text content.
/// (`*-when` keys' *condition values* are wiring; their *presence*
/// participates in skeleton identity.)
#[derive(Debug, Eq, PartialEq, Clone)]
enum Skeleton {
    /// A leaf item identified by its single property key (e.g.
    /// `text`, `icon-button`, `checkbox`, `image`). Item leaf
    /// properties are deliberately ignored.
    Item(String),
    /// A group: ordered children plus the sorted, deduplicated set of
    /// `*-when`-keyed properties present on the group props
    /// (presence-only; condition values do not participate).
    Group { when_keys: Vec<String>, items: Vec<Self> },
}

/// Walk a YAML sub-tree (typically the `screens` value) and validate
/// the structural-identity rule for every `component: <slug>`
/// directive present. Shared between layout mode and composition mode.
pub(super) fn check_structural_identity(node: &Value, json_path: &str, errors: &mut Vec<Value>) {
    let mut instances: Vec<ComponentInstance> = Vec::new();
    walk_for_components(node, json_path, false, &mut instances);

    let mut by_slug: BTreeMap<String, Vec<&ComponentInstance>> = BTreeMap::new();
    for inst in &instances {
        by_slug.entry(inst.slug.clone()).or_default().push(inst);
    }

    for (slug, group) in by_slug {
        // Per-instance `platforms.*` overrides MAY diverge from the
        // base skeleton. We only enforce identity across the base
        // instances; platform-override instances are collected for
        // completeness but not compared here.
        let base: Vec<&ComponentInstance> =
            group.iter().filter(|i| !i.in_platform_override).copied().collect();
        if base.len() < 2 {
            continue;
        }
        let canonical = base[0];
        for other in base.iter().skip(1) {
            if other.skeleton != canonical.skeleton {
                errors.push(json!({
                    "path": other.path,
                    "message": format!(
                        "component slug `{slug}` has a different skeleton at {} than the canonical instance at {} (structural-identity rule); slug instances may differ in `bind`, `event`, `error`, asset / token references, `*-when` condition values, and free text content but their group skeleton MUST match across all base instances",
                        other.path,
                        canonical.path,
                    ),
                }));
            }
        }
    }
}

/// Recursive walker for [`check_structural_identity`]. Every group
/// shaped as `{ "group": { "component": <slug>, "items": [...], ... } }`
/// produces a [`ComponentInstance`]; nested groups inside it are also
/// visited so `component:` directives nested inside a component group
/// are still picked up. The `in_platform` parameter tracks whether we
/// are currently descending through a
/// `screens.<name>.platforms.<plat>.*` sub-tree.
fn walk_for_components(
    node: &Value, json_path: &str, in_platform: bool, out: &mut Vec<ComponentInstance>,
) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                let descend_in_platform = in_platform || key == "platforms";
                if key == "group"
                    && let Some(component) = val.get("component").and_then(Value::as_str)
                {
                    out.push(ComponentInstance {
                        slug: component.to_string(),
                        skeleton: build_group_skeleton(val),
                        path: child_path.clone(),
                        in_platform_override: in_platform,
                    });
                }
                walk_for_components(val, &child_path, descend_in_platform, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                walk_for_components(v, &format!("{json_path}/{i}"), in_platform, out);
            }
        }
        _ => {}
    }
}

/// Build a [`Skeleton::Group`] from a `groupProps` JSON value. The
/// `*-when` key set is sorted + deduplicated so two groups carrying
/// the same `*-when`-keyed props (in any author order) compare equal.
/// Children are derived from the `items:` array; missing `items`
/// (schema-invalid) becomes an empty children list.
fn build_group_skeleton(group_props: &Value) -> Skeleton {
    let mut when_keys: Vec<String> = group_props
        .as_object()
        .map(|m| m.keys().filter(|k| k.ends_with("-when") && k.len() > 5).cloned().collect())
        .unwrap_or_default();
    when_keys.sort();
    when_keys.dedup();

    let items: Vec<Skeleton> = group_props
        .get("items")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(build_node_skeleton).collect())
        .unwrap_or_default();

    Skeleton::Group { when_keys, items }
}

/// Build a skeleton fragment for a single `contentNode` (an item or a
/// nested group). Each content node is either:
///
/// - `{ group: { ... } }` — a nested group, recursed via
///   [`build_group_skeleton`].
/// - `{ <kind>: <itemProps-or-null> }` — an item identified by its
///   single key (`text`, `checkbox`, `icon`, etc.). Item kind is the
///   only datum the skeleton retains; itemProps (text content,
///   bindings, colors, sizes) are wiring and ignored.
///
/// Schema-invalid shapes (zero or multi-key objects) collapse to a
/// stable `<unknown>` placeholder so the schema validator's own
/// findings remain the authoritative diagnostic.
fn build_node_skeleton(node: &Value) -> Skeleton {
    let Some(map) = node.as_object() else {
        return Skeleton::Item(String::from("<unknown>"));
    };
    if map.len() != 1 {
        return Skeleton::Item(String::from("<unknown>"));
    }
    let (key, val) = map.iter().next().expect("len 1");
    if key == "group" { build_group_skeleton(val) } else { Skeleton::Item(key.clone()) }
}
