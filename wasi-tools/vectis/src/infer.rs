//! `vectis infer` subcommand — deterministic component-identity detection.
//!
//! The verb clusters structurally-identical `group` subtrees across the
//! composition baseline and emits a **name-free** cluster report as JSON.
//! It performs *identity* and *clustering* only — it invents **no**
//! component names (that is the build skill's job; see RFC-40 §B2 "The
//! identity / label / bookkeeping split"). Each cluster carries only the
//! deterministic facts a namer needs: the structural fingerprint, the
//! occurrence count, the screen provenance list, the representative
//! normalised skeleton, and the raw semantic evidence (region, item
//! kinds, `event` targets) passed through verbatim.
//!
//! Detection scope is the `group` (RFC-40 §B2 "Detection scope"): the
//! walk descends through every screen region — including `states`,
//! `overlays`, and `platforms` — but only a `group` is a detection unit.
//!
//! This step (baseline-only) reads `--composition`. The `--candidate-cache`
//! (RFC-40 §B4) and `--parts` (RFC-40 §C2) inputs are accepted but inert
//! until later steps wire them in.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::Args as ClapArgs;
use serde_json::{Value, json};

use crate::validate::engine::composition::{
    Skeleton, build_group_skeleton, fingerprint, skeleton_to_json,
};
use crate::{VectisError, render_json as render_value};

/// Screen-entry keys whose sub-trees may carry `group` nodes. `name`,
/// `description`, and `maps_to` are scalar metadata and never walked.
const SCREEN_REGIONS: &[&str] =
    &["header", "body", "footer", "fab", "states", "overlays", "platforms"];

/// Arguments accepted by `vectis infer`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct InferArgs {
    /// Composition baseline to cluster (`.specify/specs/composition.yaml`).
    #[arg(long)]
    pub composition: PathBuf,

    /// Candidate-cache directory (RFC-40 §B4). Inert until Step 7.
    #[arg(long)]
    pub candidate_cache: Option<PathBuf>,

    /// Operator parts file (RFC-40 §C2). Inert until Step 11.
    #[arg(long)]
    pub parts: Option<PathBuf>,

    /// Minimum distinct screens a group must span to cluster.
    #[arg(long, default_value_t = 2)]
    pub min_occurrences: u32,
}

/// One observed `group` instance, before clustering. Carries the raw
/// material the report needs; the fingerprint is computed at cluster time.
struct GroupOccurrence {
    /// Screen slug this group was found under.
    screen: String,
    /// Top-level screen region the group lives in (`header`, `body`, …).
    region: String,
    /// Normalised structural skeleton of the group.
    skeleton: Skeleton,
    /// `event:` targets wired anywhere inside the group subtree.
    event_targets: BTreeSet<String>,
}

/// Accumulator for one fingerprint cluster.
struct Cluster {
    /// Distinct screens this fingerprint appears on.
    screens: BTreeSet<String>,
    /// Region of the lexicographically-smallest screen (deterministic).
    region: String,
    /// Representative skeleton (identical across the cluster by construction).
    skeleton: Skeleton,
    /// The lexicographically-smallest screen seen so far.
    representative_screen: String,
    /// Union of `event:` targets across every instance in the cluster.
    event_targets: BTreeSet<String>,
}

/// Run the inference engine over the composition baseline.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the composition file is
/// unreadable or is not valid YAML.
pub fn run(args: &InferArgs) -> Result<Value, VectisError> {
    let source =
        std::fs::read_to_string(&args.composition).map_err(|err| VectisError::InvalidProject {
            message: format!(
                "composition baseline not readable at {}: {err}",
                args.composition.display()
            ),
        })?;
    let instance: Value =
        serde_saphyr::from_str(&source).map_err(|err| VectisError::InvalidProject {
            message: format!(
                "composition baseline at {} is not valid YAML: {err}",
                args.composition.display()
            ),
        })?;

    let mut occurrences: Vec<GroupOccurrence> = Vec::new();
    collect_baseline_groups(&instance, &mut occurrences);

    let clusters = cluster(occurrences, args.min_occurrences);

    Ok(json!({
        "version": 1,
        "clusters": clusters,
        "unmatched_parts": [],
    }))
}

/// Render an inference outcome as pretty-printed JSON with an exit code.
/// A successful report always exits 0 — it is informational; runtime
/// errors carry the typed-error payload and the error's own exit code.
#[must_use]
pub fn render_json(outcome: Result<Value, VectisError>) -> (String, u8) {
    match outcome {
        Ok(value) => (render_value(&value), 0),
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

/// Collect every `group` occurrence across both composition shapes —
/// `screens.<slug>` (baseline) and `delta.added` / `delta.modified`
/// (change-local). `delta.removed` carries no screen body, so it is skipped.
fn collect_baseline_groups(instance: &Value, out: &mut Vec<GroupOccurrence>) {
    if let Some(screens) = instance.get("screens").and_then(Value::as_object) {
        for (slug, entry) in screens {
            collect_screen_groups(slug, entry, out);
        }
    }
    if let Some(delta) = instance.get("delta").and_then(Value::as_object) {
        for section in ["added", "modified"] {
            if let Some(screens) = delta.get(section).and_then(Value::as_object) {
                for (slug, entry) in screens {
                    collect_screen_groups(slug, entry, out);
                }
            }
        }
    }
}

/// Walk one screen entry's region sub-trees collecting every `group`.
fn collect_screen_groups(screen: &str, entry: &Value, out: &mut Vec<GroupOccurrence>) {
    let Some(map) = entry.as_object() else {
        return;
    };
    for (key, val) in map {
        if SCREEN_REGIONS.contains(&key.as_str()) {
            walk_region_for_groups(screen, key, val, out);
        }
    }
}

/// Recurse through a region sub-tree, recording every `{ group: … }`
/// node (top-level and nested) as a [`GroupOccurrence`].
fn walk_region_for_groups(
    screen: &str, region: &str, node: &Value, out: &mut Vec<GroupOccurrence>,
) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                if key == "group" {
                    let mut event_targets = BTreeSet::new();
                    collect_event_targets(val, &mut event_targets);
                    out.push(GroupOccurrence {
                        screen: screen.to_string(),
                        region: region.to_string(),
                        skeleton: build_group_skeleton(val),
                        event_targets,
                    });
                }
                walk_region_for_groups(screen, region, val, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk_region_for_groups(screen, region, v, out);
            }
        }
        _ => {}
    }
}

/// Collect every `event:` string value reachable inside a group subtree.
fn collect_event_targets(node: &Value, out: &mut BTreeSet<String>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                if key == "event"
                    && let Some(target) = val.as_str()
                {
                    out.insert(target.to_string());
                }
                collect_event_targets(val, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_event_targets(v, out);
            }
        }
        _ => {}
    }
}

/// Cluster occurrences by structural fingerprint and project each
/// above-threshold cluster into a name-free report entry. Counting is by
/// **distinct screen** — a group repeated within one screen counts once.
fn cluster(occurrences: Vec<GroupOccurrence>, min_occurrences: u32) -> Vec<Value> {
    let mut by_fp: BTreeMap<String, Cluster> = BTreeMap::new();
    for occ in occurrences {
        let fp = fingerprint(&occ.skeleton);
        let entry = by_fp.entry(fp).or_insert_with(|| Cluster {
            screens: BTreeSet::new(),
            region: occ.region.clone(),
            skeleton: occ.skeleton.clone(),
            representative_screen: occ.screen.clone(),
            event_targets: BTreeSet::new(),
        });
        if occ.screen < entry.representative_screen {
            entry.representative_screen.clone_from(&occ.screen);
            entry.region.clone_from(&occ.region);
            entry.skeleton = occ.skeleton.clone();
        }
        entry.screens.insert(occ.screen);
        entry.event_targets.extend(occ.event_targets);
    }

    by_fp
        .into_iter()
        .filter(|(_, c)| u32::try_from(c.screens.len()).is_ok_and(|n| n >= min_occurrences))
        .map(|(fp, c)| {
            let mut item_kinds = BTreeSet::new();
            skeleton_item_kinds(&c.skeleton, &mut item_kinds);
            json!({
                "fingerprint": fp,
                "occurrences": c.screens.len(),
                "screens": c.screens.into_iter().collect::<Vec<_>>(),
                "skeleton": skeleton_to_json(&c.skeleton),
                "evidence": {
                    "region": c.region,
                    "item_kinds": item_kinds.into_iter().collect::<Vec<_>>(),
                    "event_targets": c.event_targets.into_iter().collect::<Vec<_>>(),
                },
                "bound_slug": Value::Null,
            })
        })
        .collect()
}

/// Collect the distinct leaf item kinds present in a skeleton tree.
fn skeleton_item_kinds(skeleton: &Skeleton, out: &mut BTreeSet<String>) {
    match skeleton {
        Skeleton::Item(kind) => {
            out.insert(kind.clone());
        }
        Skeleton::Group { items, .. } => {
            for item in items {
                skeleton_item_kinds(item, out);
            }
        }
    }
}
