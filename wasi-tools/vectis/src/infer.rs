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
//! `--composition` supplies the baseline groups. `--candidate-cache`
//! (RFC-40 §B4) folds screenshot stage-6 candidate skeletons into the
//! same clustering pass: each cache entry stores a normalised `group`
//! fragment keyed by provenance (`<slice>/<screen>/<group-path>.yaml`),
//! and the fingerprint is recomputed **at read time** through the one
//! `build_group_skeleton` normaliser — no agent-written fingerprint is
//! ever trusted. A cached skeleton and a baseline group with the same
//! fingerprint cluster as one candidate, giving inference cross-slice
//! memory before the baseline accumulates.
//!
//! `--parts` (RFC-40 §C2) folds the operator-authored parts input into
//! the same read-time-fingerprint pass: each part's `group` fragment is
//! normalised and registered as a **pinned binding** `{ fingerprint →
//! slug }`. A pinned fingerprint carries two authorities — **naming**
//! (the cluster's `bound_slug` echoes the operator slug, so the build
//! skill leaves it untouched) and **promotion** (a matched pin clusters
//! as soon as it hits ≥1 baseline/cache group, bypassing
//! `--min-occurrences`). A pin matching zero groups is surfaced under
//! `unmatched_parts`. The tool still invents no name of its own — it
//! only echoes the operator's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

    /// Candidate-cache directory (RFC-40 §B4): screenshot stage-6
    /// candidate skeletons, keyed by provenance, folded into clustering.
    #[arg(long)]
    pub candidate_cache: Option<PathBuf>,

    /// Operator parts file (RFC-40 §C2): authoritative parts that seed
    /// inference with naming + promotion authority, fingerprinted at
    /// read time through the single normaliser.
    #[arg(long)]
    pub parts: Option<PathBuf>,

    /// Minimum distinct screens a group must span to cluster.
    #[arg(long, default_value_t = 2)]
    pub min_occurrences: u32,
}

/// One observed `group` instance, before clustering. Carries the raw
/// material the report needs; the fingerprint is computed at cluster time.
struct GroupOccurrence {
    /// Screen slug this group was found under. For a baseline group this
    /// is the composition screen slug; for a candidate-cache entry it is
    /// the provenance `<screen>` segment (change-wide unique, so it
    /// dedups against the baseline screen of the same name).
    screen: String,
    /// Top-level screen region the group lives in (`header`, `body`, …).
    region: String,
    /// Normalised structural skeleton of the group.
    skeleton: Skeleton,
    /// `event:` targets wired anywhere inside the group subtree.
    event_targets: BTreeSet<String>,
    /// Non-authoritative name hint from a candidate-cache entry's
    /// `candidate_component` label (RFC-40 §B4). `None` for baseline
    /// groups. Surfaced as evidence; never sets `bound_slug`.
    candidate_name: Option<String>,
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
    /// Union of candidate-cache `candidate_component` name hints across
    /// the cluster (RFC-40 §B4). Surfaced as non-authoritative evidence.
    candidate_names: BTreeSet<String>,
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
    if let Some(ref cache_dir) = args.candidate_cache
        && cache_dir.is_dir()
    {
        collect_cached_groups(cache_dir, &mut occurrences);
    }

    // Step 0 (RFC-40 §C2): register a pinned binding per operator part,
    // fingerprinted at read time through the single normaliser — so the
    // pin is byte-comparable with discovered fingerprints by construction.
    let pins = if let Some(ref parts_path) = args.parts {
        collect_part_pins(parts_path)
    } else {
        Vec::new()
    };

    // Fingerprints actually present in the baseline/cache, used to split
    // matched pins (promoted + named) from unmatched pins (reported).
    let observed: BTreeSet<String> =
        occurrences.iter().map(|occ| fingerprint(&occ.skeleton)).collect();

    // §C6 dedup: when two parts normalise to one skeleton, the
    // lexicographically-first slug binds the fingerprint.
    let mut pin_slug_by_fp: BTreeMap<String, String> = BTreeMap::new();
    let mut unmatched_parts: BTreeSet<String> = BTreeSet::new();
    for pin in pins {
        if observed.contains(&pin.fingerprint) {
            pin_slug_by_fp.entry(pin.fingerprint).or_insert(pin.slug);
        } else {
            // §C2 step 4: a pin matching zero groups is not projected —
            // it is surfaced as an unused part (§C5), never a cluster.
            unmatched_parts.insert(pin.slug);
        }
    }

    let clusters = cluster(occurrences, args.min_occurrences, &pin_slug_by_fp);

    Ok(json!({
        "version": 1,
        "clusters": clusters,
        "unmatched_parts": unmatched_parts.into_iter().collect::<Vec<_>>(),
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
                        candidate_name: None,
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

/// Read every `*.yaml` candidate-cache entry under `dir`, normalise its
/// `group` fragment through the **single** [`build_group_skeleton`]
/// path, and fold it into the occurrence list (RFC-40 §B4). The cache's
/// on-disk key carries no identity — the fingerprint is recomputed at
/// read time during clustering — so no agent-written fingerprint is
/// trusted. Malformed entries and entries without a `group` fragment are
/// skipped: inference is best-effort, never an abort.
fn collect_cached_groups(dir: &Path, out: &mut Vec<GroupOccurrence>) {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_yaml_files(dir, &mut files);
    // Deterministic read order keeps the representative-screen and
    // candidate-name accumulation stable across runs.
    files.sort();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        let Ok(entry) = serde_saphyr::from_str::<Value>(&source) else {
            continue;
        };
        let Some(group) = entry.get("group") else {
            continue;
        };
        let mut event_targets = BTreeSet::new();
        collect_event_targets(group, &mut event_targets);
        out.push(GroupOccurrence {
            screen: cache_screen_id(dir, file),
            region: entry.get("region").and_then(Value::as_str).unwrap_or_default().to_string(),
            skeleton: build_group_skeleton(group),
            event_targets,
            candidate_name: entry
                .get("candidate_component")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}

/// Recursively collect every `*.yaml` file under `dir` into `out`.
fn collect_yaml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "yaml") {
            out.push(path);
        }
    }
}

/// Derive the candidate-cache screen identity from a file's provenance
/// path. The §B4 layout is `<slice>/<screen>/<group-path>.yaml`, so the
/// `<screen>` segment (relative-path component index 1) is the screen.
/// Because composition screen slugs are change-wide unique (A2a), a
/// cached candidate for screen `home` and a baseline `home` group dedup
/// to one distinct screen by construction. A shallower path falls back
/// to the file stem so a malformed layout still yields a stable key.
fn cache_screen_id(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let components: Vec<String> =
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    components.get(1).cloned().unwrap_or_else(|| {
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    })
}

/// One operator part registered as a pinned `{ fingerprint → slug }`
/// binding (RFC-40 §C2 step 0). The fingerprint is recomputed at read
/// time, so it is byte-comparable with discovered fingerprints.
struct PartPin {
    /// Operator-authored part slug.
    slug: String,
    /// Read-time fingerprint of the part's normalised `group` skeleton.
    fingerprint: String,
}

/// Read every part in `parts.yaml`, normalise each `group` fragment
/// through the **single** [`build_group_skeleton`] path, and fingerprint
/// it at read time (RFC-40 §C2 step 0). A malformed parts file, or a
/// part without a `group`, yields no pin — the host already schema-gates
/// the file, so this read is best-effort and never aborts inference.
fn collect_part_pins(path: &Path) -> Vec<PartPin> {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_saphyr::from_str::<Value>(&source) else {
        return Vec::new();
    };
    let Some(parts) = doc.get("parts").and_then(Value::as_object) else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|(slug, entry)| {
            let group = entry.get("group")?;
            Some(PartPin {
                slug: slug.clone(),
                fingerprint: fingerprint(&build_group_skeleton(group)),
            })
        })
        .collect()
}

/// Cluster occurrences by structural fingerprint and project each
/// above-threshold cluster into a name-free report entry. Counting is by
/// **distinct screen** — a group repeated within one screen counts once.
///
/// `pin_slug_by_fp` carries operator-part pins (RFC-40 §C2): a cluster
/// whose fingerprint matches a pin bypasses `min_occurrences` (promotion
/// authority) and carries `bound_slug: <operator-slug>` + `pinned: true`
/// (naming authority); the tool still invents no name of its own.
fn cluster(
    occurrences: Vec<GroupOccurrence>, min_occurrences: u32,
    pin_slug_by_fp: &BTreeMap<String, String>,
) -> Vec<Value> {
    let mut by_fp: BTreeMap<String, Cluster> = BTreeMap::new();
    for occ in occurrences {
        let fp = fingerprint(&occ.skeleton);
        let entry = by_fp.entry(fp).or_insert_with(|| Cluster {
            screens: BTreeSet::new(),
            region: occ.region.clone(),
            skeleton: occ.skeleton.clone(),
            representative_screen: occ.screen.clone(),
            event_targets: BTreeSet::new(),
            candidate_names: BTreeSet::new(),
        });
        if occ.screen < entry.representative_screen {
            entry.representative_screen.clone_from(&occ.screen);
            entry.region.clone_from(&occ.region);
            entry.skeleton = occ.skeleton.clone();
        }
        entry.screens.insert(occ.screen);
        entry.event_targets.extend(occ.event_targets);
        if let Some(name) = occ.candidate_name {
            entry.candidate_names.insert(name);
        }
    }

    by_fp
        .into_iter()
        // A pinned fingerprint bypasses the threshold (§C2 promotion);
        // every other cluster must span ≥ min_occurrences distinct screens.
        .filter(|(fp, c)| {
            pin_slug_by_fp.contains_key(fp)
                || u32::try_from(c.screens.len()).is_ok_and(|n| n >= min_occurrences)
        })
        .map(|(fp, c)| {
            let mut item_kinds = BTreeSet::new();
            skeleton_item_kinds(&c.skeleton, &mut item_kinds);
            let mut evidence = json!({
                "region": c.region,
                "item_kinds": item_kinds.into_iter().collect::<Vec<_>>(),
                "event_targets": c.event_targets.into_iter().collect::<Vec<_>>(),
            });
            // Candidate-cache name hints are non-authoritative evidence
            // (RFC-40 §B4): emit them only when present so a baseline-only
            // report keeps its `region` / `item_kinds` / `event_targets`
            // evidence shape unchanged.
            if !c.candidate_names.is_empty()
                && let Value::Object(ref mut map) = evidence
            {
                map.insert(
                    "candidate_names".to_string(),
                    json!(c.candidate_names.into_iter().collect::<Vec<_>>()),
                );
            }
            let mut entry = json!({
                "fingerprint": fp.clone(),
                "occurrences": c.screens.len(),
                "screens": c.screens.into_iter().collect::<Vec<_>>(),
                "skeleton": skeleton_to_json(&c.skeleton),
                "evidence": evidence,
                "bound_slug": Value::Null,
            });
            // §C2 step 5: a matched pin echoes the operator slug and
            // flags the cluster `pinned` (emitted only when true, so a
            // pin-free report keeps its existing cluster shape).
            if let Some(slug) = pin_slug_by_fp.get(&fp)
                && let Value::Object(ref mut map) = entry
            {
                map.insert("bound_slug".to_string(), Value::String(slug.clone()));
                map.insert("pinned".to_string(), Value::Bool(true));
            }
            entry
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
