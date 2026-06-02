//! The `V1ToV2` migrator (RFC-30 §"Concrete migrators").
//!
//! Covers the five structural 1.x → 2.0 breaking changes:
//!
//! 1. legacy `pipeline:` manifest key → axis-split `briefs:` keys;
//! 2. monolithic `adapters/<name>/adapter.yaml` →
//!    `adapters/{sources,targets}/<name>/adapter.yaml` (the original is
//!    [`MigrationAction::Remove`]d, its brief files
//!    [`MigrationAction::Move`]d);
//! 3. retired `/change:` slash-namespace references in operator notes
//!    (`change.md` / `AGENTS.md`) → the 2.0 `/spec:` namespace;
//! 4. legacy `## Candidate inventory` → the 2.0 `## Lead inventory`
//!    block, each lead carrying a stable `(source, lead)` id;
//! 5. `plan.yaml` — strip the dropped per-slice `target` field; each
//!    slice keeps its `project` (the target resolves on demand from the
//!    bound project).
//!
//! Transforms 1 and 2 are realised together: a monolithic adapter is
//! relocated into its axis subtree with its `pipeline:` key rewritten to
//! `briefs:` in one move. [`V1ToV2::plan`] inspects the project and only
//! emits an action when the source actually needs it, so `plan` over an
//! already-2.0 tree yields an empty (no-op) plan. [`V1ToV2::apply`]
//! delegates to the shared [`apply_staged`] harness.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use specify_error::Error;

use super::{
    MigrationAction, MigrationKind, MigrationPlan, MigrationReport, Migrator, apply_staged,
};
use crate::adapter::{ADAPTER_FILENAME, Axis};
use crate::change::Plan;

/// Top-level directory holding adapter trees (`adapters/`).
const ADAPTERS_DIR: &str = "adapters";

/// Legacy candidate-inventory heading replaced by [`LEAD_HEADING`].
const CANDIDATE_HEADING: &str = "## Candidate inventory";

/// Canonical 2.0 lead-inventory heading.
const LEAD_HEADING: &str = "## Lead inventory";

/// Retired 1.x slash-namespace literal rewritten to [`SPEC_NAMESPACE`].
const CHANGE_NAMESPACE: &str = "/change:";

/// 2.0 slash-namespace the retired `/change:` references map onto.
const SPEC_NAMESPACE: &str = "/spec:";

/// Operator-authored note files transform 3 conservatively rewrites.
const NOTE_FILES: &[&str] = &["change.md", "AGENTS.md"];

/// The 1.x → 2.0 structural migrator.
///
/// Unit struct — it holds no state. The single registered instance is
/// reached through [`super::migrator_for`]; [`Self::id`] echoes
/// [`MigrationKind::id`].
#[derive(Debug, Clone, Copy, Default)]
pub struct V1ToV2;

impl Migrator for V1ToV2 {
    fn id(&self) -> &'static str {
        MigrationKind::V1ToV2.id()
    }

    fn plan(&self, project_dir: &Path) -> Result<MigrationPlan, Error> {
        let mut actions = Vec::new();
        adapter_actions(project_dir, &mut actions)?;
        note_actions(project_dir, &mut actions)?;
        discovery_actions(project_dir, &mut actions)?;
        plan_actions(project_dir, &mut actions)?;
        Ok(MigrationPlan::new(MigrationKind::V1ToV2, actions))
    }

    fn apply(&self, project_dir: &Path, plan: &MigrationPlan) -> Result<MigrationReport, Error> {
        apply_staged(project_dir, plan)
    }
}

/// Loose view over a monolithic v1 `adapter.yaml`.
///
/// Reads only the fields the split needs; unknown fields are tolerated
/// so a richer v1 manifest still parses. The `pipeline:` map is the v1
/// spelling of the 2.0 `briefs:` map.
#[derive(Debug, Deserialize)]
struct V1Manifest {
    /// Kebab-case adapter name.
    name: String,
    /// Major adapter version; defaults to `1` when omitted.
    #[serde(default)]
    version: Option<u32>,
    /// Operation → brief-path map (the v1 `pipeline:` key).
    #[serde(default)]
    pipeline: BTreeMap<String, String>,
    /// Optional human-readable summary, carried through verbatim.
    #[serde(default)]
    description: Option<String>,
}

/// Transforms 1 + 2: relocate every monolithic adapter into its axis
/// subtree, rewriting `pipeline:` to `briefs:` and removing the original.
fn adapter_actions(project_dir: &Path, actions: &mut Vec<MigrationAction>) -> Result<(), Error> {
    let adapters_dir = project_dir.join(ADAPTERS_DIR);
    let read = match std::fs::read_dir(&adapters_dir) {
        Ok(read) => read,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Filesystem {
                op: "migrate-scan-adapters",
                path: adapters_dir,
                source,
            });
        }
    };

    let mut names: Vec<String> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "migrate-scan-adapters",
            path: adapters_dir.clone(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // The axis subtrees are the 2.0 destinations, never monolithic
        // sources; skip them so an already-split tree is a no-op.
        if name == Axis::Source.dir_segment() || name == Axis::Target.dir_segment() {
            continue;
        }
        if adapters_dir.join(&name).join(ADAPTER_FILENAME).is_file() {
            names.push(name);
        }
    }
    names.sort();

    for name in names {
        split_adapter(&adapters_dir, &name, actions)?;
    }
    Ok(())
}

/// Emit the rewrite + brief moves + removal for one monolithic adapter.
fn split_adapter(
    adapters_dir: &Path, name: &str, actions: &mut Vec<MigrationAction>,
) -> Result<(), Error> {
    let manifest_path = adapters_dir.join(name).join(ADAPTER_FILENAME);
    let raw = read_file(&manifest_path)?;
    let manifest: V1Manifest = serde_saphyr::from_str(&raw)?;
    let axis = classify_axis(&manifest.pipeline, &manifest_path)?;
    let version = manifest.version.unwrap_or(1);

    let old_dir = PathBuf::from(ADAPTERS_DIR).join(name);
    let new_dir = PathBuf::from(ADAPTERS_DIR).join(axis.dir_segment()).join(name);

    let contents =
        render_manifest(&manifest.name, version, axis, &manifest.pipeline, manifest.description);
    actions.push(MigrationAction::Rewrite {
        path: new_dir.join(ADAPTER_FILENAME),
        contents,
    });

    for relative_brief in manifest.pipeline.values() {
        actions.push(MigrationAction::Move {
            from: old_dir.join(relative_brief),
            to: new_dir.join(relative_brief),
        });
    }

    actions.push(MigrationAction::Remove {
        path: old_dir.join(ADAPTER_FILENAME),
    });
    Ok(())
}

/// Infer an adapter's axis from its operation set. A pure source set
/// (`extract` / `survey`) is a source adapter; a pure target set
/// (`build` / `merge` / `shape`) is a target adapter. An empty or mixed
/// set cannot be classified.
fn classify_axis(pipeline: &BTreeMap<String, String>, manifest_path: &Path) -> Result<Axis, Error> {
    let source_ops = ["extract", "survey"];
    let target_ops = ["build", "merge", "shape"];
    let all_source =
        !pipeline.is_empty() && pipeline.keys().all(|k| source_ops.contains(&k.as_str()));
    let all_target =
        !pipeline.is_empty() && pipeline.keys().all(|k| target_ops.contains(&k.as_str()));
    match (all_source, all_target) {
        (true, false) => Ok(Axis::Source),
        (false, true) => Ok(Axis::Target),
        _ => Err(Error::Diag {
            code: "migrate-adapter-axis-unknown",
            detail: format!(
                "{} has a pipeline whose keys map to neither a pure source \
                 (`extract`/`survey`) nor a pure target (`build`/`merge`/`shape`) \
                 operation set",
                manifest_path.display(),
            ),
        }),
    }
}

/// Build the 2.0 `adapter.yaml` body. Field order matches the
/// `SourceAdapter` / `TargetAdapter` struct (`name`, `version`, `axis`,
/// `execution`, `briefs`, `description`); briefs render in sorted
/// (`BTreeMap`) order so the output is byte-stable. v1 manifests carry
/// no `execution`, so the migrator stamps the `agent` default.
fn render_manifest(
    name: &str, version: u32, axis: Axis, briefs: &BTreeMap<String, String>,
    description: Option<String>,
) -> String {
    let mut out = String::new();
    out.push_str("name: ");
    out.push_str(name);
    out.push('\n');
    out.push_str("version: ");
    out.push_str(&version.to_string());
    out.push('\n');
    out.push_str("axis: ");
    out.push_str(&axis.to_string());
    out.push('\n');
    out.push_str("execution: agent\n");
    out.push_str("briefs:\n");
    for (operation, brief) in briefs {
        out.push_str("  ");
        out.push_str(operation);
        out.push_str(": ");
        out.push_str(brief);
        out.push('\n');
    }
    if let Some(description) = description {
        out.push_str("description: ");
        out.push_str(&description);
        out.push('\n');
    }
    out
}

/// Transform 3: rewrite literal retired `/change:` slash-namespace
/// references in the conservative [`NOTE_FILES`] set.
fn note_actions(project_dir: &Path, actions: &mut Vec<MigrationAction>) -> Result<(), Error> {
    for note in NOTE_FILES {
        let path = project_dir.join(note);
        if !path.is_file() {
            continue;
        }
        let raw = read_file(&path)?;
        if raw.contains(CHANGE_NAMESPACE) {
            actions.push(MigrationAction::Rewrite {
                path: PathBuf::from(note),
                contents: raw.replace(CHANGE_NAMESPACE, SPEC_NAMESPACE),
            });
        }
    }
    Ok(())
}

/// One legacy candidate block parsed out of `## Candidate inventory`.
struct Candidate {
    /// Candidate identifier (the `### <id>` heading), reused as the
    /// stable 2.0 lead id.
    id: String,
    /// Source key that surfaced the candidate.
    source: String,
    /// Per-source synopsis.
    synopsis: String,
}

/// Transform 4: reformat a legacy `## Candidate inventory` discovery
/// document into the 2.0 `## Lead inventory` shape.
fn discovery_actions(project_dir: &Path, actions: &mut Vec<MigrationAction>) -> Result<(), Error> {
    let path = project_dir.join("discovery.md");
    if !path.is_file() {
        return Ok(());
    }
    let raw = read_file(&path)?;
    if !raw.lines().any(|line| line.trim() == CANDIDATE_HEADING) {
        return Ok(());
    }
    let contents = reformat_discovery(&raw, &path)?;
    actions.push(MigrationAction::Rewrite {
        path: PathBuf::from("discovery.md"),
        contents,
    });
    Ok(())
}

/// Render the legacy candidate inventory as a 2.0 lead inventory.
///
/// Prose before the inventory heading and any trailing section after it
/// round-trip verbatim. Each `### <id>` candidate becomes a
/// `### <source>:<id>` lead block with a `lead` / `source` / `synopsis`
/// bullet list, matching the layout `Discovery::render` emits.
fn reformat_discovery(raw: &str, path: &Path) -> Result<String, Error> {
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();

    let mut prefix = String::new();
    let mut cursor = 0;
    while cursor < lines.len() && strip(lines[cursor]).trim() != CANDIDATE_HEADING {
        prefix.push_str(lines[cursor]);
        cursor += 1;
    }
    // Skip the candidate-inventory heading line itself.
    cursor += 1;

    let mut candidates: Vec<Candidate> = Vec::new();
    while cursor < lines.len() {
        let line = strip(lines[cursor]);
        if line.starts_with("## ") {
            break;
        }
        if line.starts_with("### ") {
            let (candidate, next) = parse_candidate(&lines, cursor, path)?;
            candidates.push(candidate);
            cursor = next;
            continue;
        }
        cursor += 1;
    }
    let suffix = lines[cursor..].concat();

    let mut out = String::new();
    out.push_str(&prefix);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(LEAD_HEADING);
    out.push_str("\n\n");
    for (idx, candidate) in candidates.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str("### ");
        out.push_str(&candidate.source);
        out.push(':');
        out.push_str(&candidate.id);
        out.push_str("\n\n");
        out.push_str("- lead: ");
        out.push_str(&candidate.id);
        out.push('\n');
        out.push_str("- source: ");
        out.push_str(&candidate.source);
        out.push('\n');
        out.push_str("- synopsis: ");
        out.push_str(&candidate.synopsis);
        out.push('\n');
    }
    if !suffix.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&suffix);
    }
    Ok(out)
}

/// Parse one `### <id>` candidate block starting at `start`. Returns the
/// candidate and the index of the first line past the block.
fn parse_candidate(lines: &[&str], start: usize, path: &Path) -> Result<(Candidate, usize), Error> {
    let id = strip(lines[start]).strip_prefix("### ").unwrap_or("").trim().to_string();
    let mut cursor = start + 1;
    let mut source: Option<String> = None;
    let mut synopsis: Option<String> = None;
    while cursor < lines.len() {
        let line = strip(lines[cursor]);
        if line.starts_with("### ") || line.starts_with("## ") {
            break;
        }
        if let Some(body) = line.trim_start().strip_prefix("- ")
            && let Some((key, value)) = body.split_once(':')
        {
            match key.trim() {
                "source" => source = Some(value.trim().to_string()),
                "synopsis" => synopsis = Some(value.trim().to_string()),
                _ => {}
            }
        }
        cursor += 1;
    }

    let source = source.ok_or_else(|| Error::Diag {
        code: "migrate-discovery-candidate-incomplete",
        detail: format!("candidate `{id}` in {} is missing a `source` bullet", path.display()),
    })?;
    let synopsis = synopsis.ok_or_else(|| Error::Diag {
        code: "migrate-discovery-candidate-incomplete",
        detail: format!("candidate `{id}` in {} is missing a `synopsis` bullet", path.display()),
    })?;
    Ok((Candidate { id, source, synopsis }, cursor))
}

/// Transform 5: strip the dropped per-slice `target` field from
/// `plan.yaml`. Parsing through the typed [`Plan`] drops `target`
/// (an unknown field) and re-serialises the 2.0 shape, preserving each
/// slice's `project`.
fn plan_actions(project_dir: &Path, actions: &mut Vec<MigrationAction>) -> Result<(), Error> {
    let path = project_dir.join("plan.yaml");
    if !path.is_file() {
        return Ok(());
    }
    let raw = read_file(&path)?;
    if !plan_has_target(&raw)? {
        return Ok(());
    }
    let plan: Plan = serde_saphyr::from_str(&raw)?;
    let mut contents = serde_saphyr::to_string(&plan)?;
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    actions.push(MigrationAction::Rewrite {
        path: PathBuf::from("plan.yaml"),
        contents,
    });
    Ok(())
}

/// `true` when any slice in `raw` still carries the dropped `target`
/// field. The loose view ignores every other field so it stays robust
/// across both v1 and 2.0 plan shapes.
fn plan_has_target(raw: &str) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct RawPlan {
        #[serde(default)]
        slices: Vec<RawSlice>,
    }
    #[derive(Deserialize)]
    struct RawSlice {
        #[serde(default)]
        target: Option<String>,
    }
    let parsed: RawPlan = serde_saphyr::from_str(raw)?;
    Ok(parsed.slices.iter().any(|slice| slice.target.is_some()))
}

/// Read a file the migrator inspects, mapping I/O failure to a
/// `migrate-read` [`Error::Filesystem`].
fn read_file(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
        op: "migrate-read",
        path: path.to_path_buf(),
        source,
    })
}

/// Strip a trailing `\r?\n` from a `split_inclusive('\n')` line.
fn strip(line: &str) -> &str {
    line.strip_suffix('\n').map_or(line, |s| s.strip_suffix('\r').unwrap_or(s))
}
