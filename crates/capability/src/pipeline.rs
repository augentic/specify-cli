//! `PipelineView` — a resolved capability paired with every brief its
//! pipeline references, with cross-reference validations applied.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use specify_error::Error;

use crate::brief::Brief;
use crate::capability::{Capability, Phase, ResolvedCapability};

/// A capability plus every brief referenced by
/// `pipeline.{define,build,merge}`, iterated in pipeline order.
#[derive(Debug)]
pub struct PipelineView {
    /// The resolved capability manifest.
    pub capability: ResolvedCapability,
    /// Briefs in pipeline order, each paired with its phase.
    pub briefs: Vec<(Phase, Brief)>,
}

impl PipelineView {
    /// Resolve `schema_value`, load every referenced brief from the
    /// capability root, and validate cross-references:
    ///
    /// 1. Every `PipelineEntry.brief` path exists and parses.
    /// 2. `Brief.frontmatter.id` equals the referencing `PipelineEntry.id`.
    /// 3. Every `needs` id refers to a brief that appears **earlier** in
    ///    pipeline order (plan → define → build → merge).
    /// 4. Every `tracks` id refers to a brief in the same capability
    ///    (any phase).
    ///
    /// Plan-phase briefs are loaded ahead of the execution-loop phases
    /// so that a define-phase brief legitimately referring back to a
    /// plan-phase brief via `needs` / `tracks` still satisfies the
    /// earlier-in-pipeline-order rule.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(schema_value: &str, project_dir: &Path) -> Result<Self, Error> {
        let resolved = Capability::resolve(schema_value, project_dir)?;

        let mut briefs: Vec<(Phase, Brief)> = Vec::new();
        let plan_iter = resolved.manifest.plan_entries().iter().map(|e| (Phase::Plan, e));
        for (phase, entry) in plan_iter.chain(resolved.manifest.entries()) {
            let brief_path = resolved.root_dir.join(&entry.brief);
            let brief = Brief::load(&brief_path)?;

            if brief.frontmatter.id != entry.id {
                return Err(Error::SchemaResolution(format!(
                    "brief at {} declares id `{}` but pipeline entry references id `{}`",
                    brief_path.display(),
                    brief.frontmatter.id,
                    entry.id
                )));
            }

            briefs.push((phase, brief));
        }

        let known_ids: HashSet<&str> =
            briefs.iter().map(|(_, b)| b.frontmatter.id.as_str()).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for (_phase, brief) in &briefs {
            for needed in &brief.frontmatter.needs {
                if !seen.contains(needed.as_str()) {
                    return Err(Error::SchemaResolution(format!(
                        "brief `{}` needs `{}` but that brief is not earlier in pipeline order",
                        brief.frontmatter.id, needed
                    )));
                }
            }
            if let Some(tracked) = &brief.frontmatter.tracks
                && !known_ids.contains(tracked.as_str())
            {
                return Err(Error::SchemaResolution(format!(
                    "brief `{}` tracks `{}` but no such brief exists in this schema",
                    brief.frontmatter.id, tracked
                )));
            }
            seen.insert(brief.frontmatter.id.as_str());
        }

        Ok(Self {
            capability: resolved,
            briefs,
        })
    }

    /// Lookup a brief by its frontmatter id.
    #[must_use]
    pub fn brief(&self, id: &str) -> Option<&Brief> {
        self.briefs.iter().find(|(_, b)| b.frontmatter.id == id).map(|(_, b)| b)
    }

    /// Iterator over briefs belonging to `phase`.
    pub fn phase(&self, phase: Phase) -> impl Iterator<Item = &Brief> + '_ {
        self.briefs.iter().filter(move |(p, _)| *p == phase).map(|(_, b)| b)
    }

    /// Briefs for `phase` in topological order derived from each brief's
    /// `needs` frontmatter. `PipelineView::load` already rejects
    /// capabilities where a brief references a later-in-pipeline `needs`
    /// target, so for well-formed capabilities this is equivalent to
    /// `self.phase(phase)`. Running Kahn's algorithm on the subgraph
    /// anyway pins the contract so the callers (e.g. the define skill
    /// driving artifact generation in dependency order) do not have to
    /// assume pipeline order is toposort order in perpetuity.
    ///
    /// Ties (two briefs with the same in-degree) are broken by their
    /// original pipeline index so output is deterministic.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn topo_order(&self, phase: Phase) -> Result<Vec<&Brief>, Error> {
        let briefs: Vec<(usize, &Brief)> = self
            .briefs
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| *p == phase)
            .map(|(idx, (_, b))| (idx, b))
            .collect();
        let ids: HashSet<&str> = briefs.iter().map(|(_, b)| b.frontmatter.id.as_str()).collect();

        // Build the in-degree map only counting `needs` edges that target
        // another brief in the same phase. Cross-phase `needs` (e.g.
        // build.needs = [specs]) are satisfied implicitly by the
        // define → build → merge ordering and do not participate here.
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (_, brief) in &briefs {
            in_degree.entry(brief.frontmatter.id.as_str()).or_insert(0);
            for need in &brief.frontmatter.needs {
                if ids.contains(need.as_str()) {
                    *in_degree.entry(brief.frontmatter.id.as_str()).or_insert(0) += 1;
                    dependents
                        .entry(need.as_str())
                        .or_default()
                        .push(brief.frontmatter.id.as_str());
                }
            }
        }

        // Use the original (pipeline-order) index as a stable tie-breaker.
        let index_of: BTreeMap<&str, usize> =
            briefs.iter().map(|(idx, b)| (b.frontmatter.id.as_str(), *idx)).collect();

        let mut ready: Vec<&str> =
            in_degree.iter().filter_map(|(id, deg)| (*deg == 0).then_some(*id)).collect();
        ready.sort_by_key(|id| index_of.get(id).copied().unwrap_or(usize::MAX));

        let mut order: Vec<&Brief> = Vec::with_capacity(briefs.len());
        while let Some(current) = ready.first().copied() {
            ready.remove(0);
            if let Some(brief) =
                briefs.iter().find(|(_, b)| b.frontmatter.id == current).map(|(_, b)| *b)
            {
                order.push(brief);
            }
            if let Some(deps) = dependents.get(current) {
                for dep in deps {
                    if let Some(count) = in_degree.get_mut(dep) {
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            ready.push(dep);
                        }
                    }
                }
                ready.sort_by_key(|id| index_of.get(id).copied().unwrap_or(usize::MAX));
            }
        }

        if order.len() != briefs.len() {
            return Err(Error::SchemaResolution(format!(
                "cycle detected in {phase:?} `needs` graph"
            )));
        }
        Ok(order)
    }

    /// Per-brief completion for every brief in `phase` relative to a
    /// change directory: `true` when the brief's `generates` target
    /// (file path or glob) resolves to at least one readable file under
    /// `change_dir`, `false` otherwise. Briefs without `generates` are
    /// omitted entirely — they don't own an artifact to check.
    ///
    /// The scan intentionally mirrors the logic previously inlined in
    /// `collect_status` in the CLI binary; consolidating it here is
    /// what lets `specify status`, `specify schema pipeline`, and the
    /// phase skills agree byte-for-byte on what "complete" means.
    #[must_use]
    pub fn completion_for(&self, phase: Phase, change_dir: &Path) -> BTreeMap<String, bool> {
        let mut out: BTreeMap<String, bool> = BTreeMap::new();
        for brief in self.phase(phase) {
            let Some(generates) = brief.frontmatter.generates.as_deref() else {
                continue;
            };
            out.insert(brief.frontmatter.id.clone(), artifact_present(change_dir, generates));
        }
        out
    }
}

/// Resolve `generates` (a literal filename or glob pattern) against
/// `change_dir`, returning `true` when it matches at least one file.
pub fn artifact_present(change_dir: &Path, generates: &str) -> bool {
    let joined = change_dir.join(generates);
    if generates.contains('*') {
        let Some(pattern) = joined.to_str() else {
            return false;
        };
        glob::glob(pattern)
            .is_ok_and(|mut entries| entries.any(|e| matches!(e, Ok(p) if p.is_file())))
    } else {
        joined.is_file()
    }
}
