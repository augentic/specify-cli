//! Cross-artifact reference resolution: token references against
//! `tokens.yaml` categories and static asset references against
//! `assets.yaml` ids.

use serde_json::Value;

use super::finding::Finding;
use crate::validate::engine::assets::collect_asset_references;
use crate::validate::engine::shared::escape_pointer_token;

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
pub(super) fn resolve_token_references(
    composition: &Value, tokens: &Value, errors: &mut Vec<Finding>,
) {
    walk_token_refs(composition, "", tokens, errors);
}

/// Recursive walker driving [`resolve_token_references`]. Matches on
/// the well-known token-bearing keys and recurses through the rest of
/// the document. The category lookup is centralised in
/// [`token_category_for_key`] so the walker stays small.
fn walk_token_refs(node: &Value, json_path: &str, tokens: &Value, errors: &mut Vec<Finding>) {
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
    category: &str, name: &str, json_path: &str, tokens: &Value, errors: &mut Vec<Finding>,
) {
    let exists =
        tokens.get(category).and_then(Value::as_object).is_some_and(|m| m.contains_key(name));
    if !exists {
        errors.push(Finding::new(
            json_path,
            format!(
                "composition references unknown {category} token `{name}` -- not present in tokens.yaml under `{category}.{name}`",
            ),
        ));
    }
}

/// Walk a composition document and append an error for every static
/// asset reference whose name is not declared under `assets.<id>` in
/// the supplied assets manifest. Reuses [`collect_asset_references`]
/// so the reference shapes (`image.name`, `icon.name`,
/// `icon-button.icon`, `fab.icon`) stay in lock-step between
/// composition mode (this function) and assets mode's own
/// composition-discovery path.
pub(super) fn resolve_asset_references(
    composition: &Value, assets: &Value, errors: &mut Vec<Finding>,
) {
    let asset_ids = assets.get("assets").and_then(Value::as_object);
    let refs = collect_asset_references(composition);
    for asset_ref in &refs {
        let exists = asset_ids.is_some_and(|m| m.contains_key(&asset_ref.id));
        if !exists {
            errors.push(Finding::new(
                asset_ref.path.clone(),
                format!(
                    "composition references unknown asset id `{}` -- not present in assets.yaml",
                    asset_ref.id,
                ),
            ));
        }
    }
}
