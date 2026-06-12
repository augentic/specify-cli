//! Structural-identity engine for `component:` directives: skeleton
//! normalisation, the base-instance identity rule (shared with
//! `validate layout`), and the content-addressed fingerprint the
//! `infer` verb keys clusters on.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::finding::Finding;
use crate::validate::engine::shared::escape_pointer_token;

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
pub(crate) enum Skeleton {
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
pub(crate) fn check_structural_identity(
    node: &Value, json_path: &str, errors: &mut Vec<Finding>,
) {
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
                errors.push(Finding::new(
                    other.path.clone(),
                    format!(
                        "component slug `{slug}` has a different skeleton at {} than the canonical instance at {} (structural-identity rule); slug instances may differ in `bind`, `event`, `error`, asset / token references, `*-when` condition values, and free text content but their group skeleton MUST match across all base instances",
                        other.path,
                        canonical.path,
                    ),
                ));
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
pub(crate) fn build_group_skeleton(group_props: &Value) -> Skeleton {
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
pub(crate) fn build_node_skeleton(node: &Value) -> Skeleton {
    let Some(map) = node.as_object() else {
        return Skeleton::Item(String::from("<unknown>"));
    };
    if map.len() != 1 {
        return Skeleton::Item(String::from("<unknown>"));
    }
    let (key, val) = map.iter().next().expect("len 1");
    if key == "group" { build_group_skeleton(val) } else { Skeleton::Item(key.clone()) }
}

/// Compute a canonical, content-addressed fingerprint over a normalised
/// [`Skeleton`] tree: a deterministic byte serialisation followed by
/// SHA-256, rendered as a lowercase hex string.
///
/// Identity is **exact** by mandate — two groups share a fingerprint
/// iff their normalised skeletons are byte-equal. All tolerance (value-,
/// state-, and asset-level variation) is already discarded by
/// [`build_group_skeleton`] before the hash is taken, so the fingerprint
/// adds no strictness over the existing [`check_structural_identity`]
/// rule (which the inference verb must never contradict).
///
/// The fingerprint *string* is required only where a stable cross-process
/// key is needed (the candidate-cache entry and the bind-time collision
/// suffix); in-process clustering can key on the `Skeleton` directly.
pub(crate) fn fingerprint(skeleton: &Skeleton) -> String {
    let mut canonical = String::new();
    encode_skeleton(skeleton, &mut canonical);
    let digest = Sha256::digest(canonical.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Infallible: writing to a String never errors.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Append a canonical, deterministic byte encoding of `skeleton` to `buf`.
///
/// The grammar is unambiguous over the constrained skeleton alphabet
/// (item kinds and `*-when` keys are kebab-case, never containing the
/// `;:[](),` delimiters):
///
/// - `Item(kind)`  → `I:<kind>;`
/// - `Group`       → `G[<when_keys joined by ,>](<child encodings…>);`
///
/// `when_keys` are already sorted + deduped by [`build_group_skeleton`],
/// so two groups carrying the same `*-when` props in any author order
/// encode identically. Child order is preserved (item order is
/// structural).
fn encode_skeleton(skeleton: &Skeleton, buf: &mut String) {
    match skeleton {
        Skeleton::Item(kind) => {
            buf.push_str("I:");
            buf.push_str(kind);
            buf.push(';');
        }
        Skeleton::Group { when_keys, items } => {
            buf.push_str("G[");
            buf.push_str(&when_keys.join(","));
            buf.push_str("](");
            for item in items {
                encode_skeleton(item, buf);
            }
            buf.push_str(");");
        }
    }
}

/// Project a normalised [`Skeleton`] into the name-free JSON fragment
/// the `infer` report carries as the cluster's representative skeleton.
/// Mirrors the [`encode_skeleton`] grammar in structured form so the
/// build skill can read the shape it must name.
pub(crate) fn skeleton_to_json(skeleton: &Skeleton) -> Value {
    match skeleton {
        Skeleton::Item(kind) => json!({ "item": kind }),
        Skeleton::Group { when_keys, items } => json!({
            "group": {
                "when_keys": when_keys,
                "items": items.iter().map(skeleton_to_json).collect::<Vec<_>>(),
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{build_group_skeleton, fingerprint, skeleton_to_json};

    fn group(items: Value) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("items".to_string(), items);
        Value::Object(map)
    }

    #[test]
    fn fingerprint_is_stable_across_calls() {
        let skeleton = build_group_skeleton(&group(json!([
            { "icon-button": { "bind": "home", "event": "Navigate(Home)" } },
            { "icon-button": { "bind": "search", "event": "Navigate(Search)" } },
        ])));
        assert_eq!(fingerprint(&skeleton), fingerprint(&skeleton));
    }

    #[test]
    fn fingerprint_ignores_wiring_values() {
        // Same skeleton (two icon-buttons) with different bind / event
        // wiring must collapse to one fingerprint — tolerance lives in
        // normalisation, never in the hash.
        let a = build_group_skeleton(&group(json!([
            { "icon-button": { "bind": "home", "event": "Navigate(Home)" } },
            { "icon-button": { "bind": "search", "event": "Navigate(Search)" } },
        ])));
        let b = build_group_skeleton(&group(json!([
            { "icon-button": { "bind": "profile", "event": "Navigate(Profile)" } },
            { "icon-button": { "bind": "inbox", "event": "Navigate(Inbox)" } },
        ])));
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn fingerprint_distinguishes_structural_divergence() {
        // An extra item is genuine structural divergence: distinct
        // skeleton, distinct fingerprint.
        let two = build_group_skeleton(&group(json!([
            { "icon-button": {} },
            { "icon-button": {} },
        ])));
        let three = build_group_skeleton(&group(json!([
            { "icon-button": {} },
            { "icon-button": {} },
            { "icon-button": {} },
        ])));
        assert_ne!(fingerprint(&two), fingerprint(&three));
    }

    #[test]
    fn fingerprint_distinguishes_when_key_presence() {
        let bare = build_group_skeleton(&json!({ "items": [ { "text": {} } ] }));
        let conditional =
            build_group_skeleton(&json!({ "active-when": "$x", "items": [ { "text": {} } ] }));
        assert_ne!(fingerprint(&bare), fingerprint(&conditional));
    }

    #[test]
    fn skeleton_json_mirrors_tree_shape() {
        let skeleton = build_group_skeleton(&json!({
            "active-when": "$route",
            "items": [
                { "icon-button": {} },
                { "group": { "items": [ { "text": {} } ] } },
            ],
        }));
        let projected = skeleton_to_json(&skeleton);
        assert_eq!(projected["group"]["when_keys"], json!(["active-when"]));
        assert_eq!(projected["group"]["items"][0], json!({ "item": "icon-button" }));
        assert_eq!(
            projected["group"]["items"][1],
            json!({ "group": { "when_keys": [], "items": [ { "item": "text" } ] } })
        );
    }
}
