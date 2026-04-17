//! `PipelineView` — a resolved schema paired with every brief its
//! pipeline references, with cross-reference validations applied.

use std::collections::HashSet;
use std::path::Path;

use specify_error::Error;

use crate::brief::Brief;
use crate::schema::{Phase, ResolvedSchema, Schema};

/// A schema plus every brief referenced by `pipeline.{define,build,merge}`,
/// iterated in pipeline order.
#[derive(Debug)]
pub struct PipelineView {
    pub schema: ResolvedSchema,
    pub briefs: Vec<(Phase, Brief)>,
}

impl PipelineView {
    /// Resolve `schema_value`, load every referenced brief from the
    /// schema root, and validate cross-references:
    ///
    /// 1. Every `PipelineEntry.brief` path exists and parses.
    /// 2. `Brief.frontmatter.id` equals the referencing `PipelineEntry.id`.
    /// 3. Every `needs` id refers to a brief that appears **earlier** in
    ///    pipeline order (define → build → merge).
    /// 4. Every `tracks` id refers to a brief in the same schema
    ///    (any phase).
    pub fn load(schema_value: &str, project_dir: &Path) -> Result<Self, Error> {
        let resolved = Schema::resolve(schema_value, project_dir)?;

        let mut briefs: Vec<(Phase, Brief)> = Vec::new();
        for (phase, entry) in resolved.schema.entries() {
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

        let known_ids: HashSet<&str> = briefs
            .iter()
            .map(|(_, b)| b.frontmatter.id.as_str())
            .collect();
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

        Ok(PipelineView {
            schema: resolved,
            briefs,
        })
    }

    /// Lookup a brief by its frontmatter id.
    pub fn brief(&self, id: &str) -> Option<&Brief> {
        self.briefs
            .iter()
            .find(|(_, b)| b.frontmatter.id == id)
            .map(|(_, b)| b)
    }

    /// Iterator over briefs belonging to `phase`.
    pub fn phase(&self, phase: Phase) -> impl Iterator<Item = &Brief> + '_ {
        self.briefs
            .iter()
            .filter(move |(p, _)| *p == phase)
            .map(|(_, b)| b)
    }
}
