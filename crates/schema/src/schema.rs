//! `Schema`, `Pipeline`, `PipelineEntry`, `Phase`, `ResolvedSchema`,
//! `SchemaSource` — the in-memory model of `schema.yaml` plus the local /
//! cache resolution algorithm. Remote (HTTP) resolution is explicitly the
//! agent's job per RFC-1; this crate only walks the filesystem.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::ValidationResult;

const SCHEMA_JSON_SCHEMA: &str = include_str!("../../../schemas/schema.schema.json");

/// In-memory representation of a `schema.yaml` file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Schema {
    /// Schema name (e.g. `"omnia"`).
    pub name: String,
    /// Schema version number.
    pub version: u32,
    /// Human-readable description of this schema.
    pub description: String,
    /// Parent schema to inherit from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Optional domain block describing the technology context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// The pipeline of briefs organised by phase.
    pub pipeline: Pipeline,
}

/// Pipeline phases and their brief entries.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Pipeline {
    /// Optional Layer 3 authoring-phase briefs for `/spec:plan`.
    /// Absent in pre-existing schemas; present ones expose briefs such
    /// as `discovery.md` → `propose.md` that run before the
    /// define→build→merge execution loop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<PipelineEntry>,
    /// Define-phase brief entries.
    pub define: Vec<PipelineEntry>,
    /// Build-phase brief entries.
    pub build: Vec<PipelineEntry>,
    /// Merge-phase brief entries.
    pub merge: Vec<PipelineEntry>,
}

/// One entry in a pipeline phase referencing a brief file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PipelineEntry {
    /// Unique identifier matching the brief's frontmatter `id`.
    pub id: String,
    /// Relative path to the brief markdown file.
    pub brief: String,
}

/// A `Schema` plus the directory it was resolved from and how it got there.
#[derive(Debug)]
pub struct ResolvedSchema {
    /// The parsed and (if extended) merged schema.
    pub schema: Schema,
    /// Filesystem directory the schema was loaded from.
    pub root_dir: PathBuf,
    /// How the schema was located (local workspace or agent cache).
    pub source: SchemaSource,
}

/// How a schema was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaSource {
    /// Resolved from a local `schemas/<name>/` directory.
    Local(PathBuf),
    /// Resolved from the agent-populated `.specify/.cache/<name>/` directory.
    Cached(PathBuf),
}

/// The phases of a schema's pipeline.
///
/// Serializes as the lowercase identifiers `plan | define | build | merge`
/// on the wire — this is the same wire format consumed by
/// `ChangeMetadata.outcome.phase` and by `schema.yaml::pipeline.*` keys,
/// keeping a single source of truth for phase naming.
///
/// `Plan` is the Layer 3 authoring phase (`/spec:plan`) that runs
/// ahead of the define→build→merge execution loop. It is intentionally
/// omitted from `Schema::entries()` (see that iterator's docs) — call
/// `Schema::plan_entries()` to enumerate plan-phase briefs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Phase {
    /// Layer 3 authoring phase (`/spec:plan`).
    Plan,
    /// Define phase — artifact generation.
    Define,
    /// Build phase — implementation.
    Build,
    /// Merge phase — finalisation and landing.
    Merge,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Plan => "plan",
            Self::Define => "define",
            Self::Build => "build",
            Self::Merge => "merge",
        })
    }
}

impl Schema {
    /// Resolve `schema_value` against `project_dir`.
    ///
    /// - Bare names (no `/`, no `://`) resolve against
    ///   `<project_dir>/.specify/.cache/<name>/` first (populated by the
    ///   agent), then fall back to `<project_dir>/schemas/<name>/` in the
    ///   workspace itself.
    /// - URL-shaped values (containing `://`) only resolve from cache at
    ///   `<project_dir>/.specify/.cache/<last-path-segment>/`; HTTP
    ///   fetching is the agent's responsibility.
    ///
    /// When the loaded schema has `extends`, the parent is resolved via
    /// this same function and merged on top of the child via
    /// [`Schema::merge`].
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn resolve(schema_value: &str, project_dir: &Path) -> Result<ResolvedSchema, Error> {
        let (root_dir, source) = locate_schema_root(schema_value, project_dir)?;
        let schema_path = root_dir.join("schema.yaml");
        let raw = std::fs::read_to_string(&schema_path).map_err(|err| {
            Error::SchemaResolution(format!(
                "failed to read schema file {}: {err}",
                schema_path.display()
            ))
        })?;
        let schema: Self = serde_yaml_ng::from_str(&raw).map_err(|err| {
            Error::SchemaResolution(format!("failed to parse {}: {err}", schema_path.display()))
        })?;

        let merged = if let Some(parent_value) = schema.extends.clone() {
            let parent = Self::resolve(&parent_value, project_dir)?;
            Self::merge(parent.schema, schema)
        } else {
            schema
        };

        Ok(ResolvedSchema {
            schema: merged,
            root_dir,
            source,
        })
    }

    /// Validate this in-memory schema against the embedded
    /// `schemas/schema.schema.json`. Returns one `ValidationResult` per
    /// check performed (empty = fully valid).
    #[must_use]
    pub fn validate_structure(&self) -> Vec<ValidationResult> {
        let schema_value: serde_json::Value = match serde_json::to_value(self) {
            Ok(value) => value,
            Err(err) => {
                return vec![ValidationResult::Fail {
                    rule_id: "schema.serializable",
                    rule: "schema is serializable to JSON",
                    detail: err.to_string(),
                }];
            }
        };
        validate_against_embedded_schema(
            SCHEMA_JSON_SCHEMA,
            "schema.valid",
            "schema.yaml conforms to schemas/schema.schema.json",
            &schema_value,
        )
    }

    /// Iterator over every execution-loop pipeline entry in order
    /// (define → build → merge), paired with its phase.
    ///
    /// This intentionally skips `pipeline.plan`: the plan phase is an
    /// authoring-time step driven by `/spec:plan` and is not part of
    /// the per-change execution loop that `specify change status`,
    /// `specify change outcome`, and the define/build/merge skills
    /// iterate over. Plan briefs are exposed via
    /// [`Schema::plan_entries`] instead so existing callers keep their
    /// current semantics.
    pub fn entries(&self) -> impl Iterator<Item = (Phase, &PipelineEntry)> + '_ {
        self.pipeline
            .define
            .iter()
            .map(|e| (Phase::Define, e))
            .chain(self.pipeline.build.iter().map(|e| (Phase::Build, e)))
            .chain(self.pipeline.merge.iter().map(|e| (Phase::Merge, e)))
    }

    /// Plan-phase (Layer 3 authoring) pipeline entries in declared
    /// order. Returns an empty slice for schemas that don't declare a
    /// `pipeline.plan` block.
    #[must_use]
    pub fn plan_entries(&self) -> &[PipelineEntry] {
        &self.pipeline.plan
    }

    /// Look up a pipeline entry by id. Searches the plan phase first so
    /// authoring briefs are discoverable, then the define→build→merge
    /// execution loop.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<(Phase, &PipelineEntry)> {
        self.pipeline
            .plan
            .iter()
            .map(|e| (Phase::Plan, e))
            .chain(self.entries())
            .find(|(_, e)| e.id == id)
    }

    /// Merge `child` on top of `parent`. Per schema-resolution.md:
    ///
    /// - `pipeline`: for each phase, child entries with the same `id`
    ///   replace the parent's entry in place; new ids are appended in
    ///   child order.
    /// - `domain`: child replaces parent if present, else parent is kept.
    /// - All other top-level fields (`name`, `version`, `description`)
    ///   come from the child.
    /// - `extends` is cleared — the composed schema has no unresolved
    ///   parent.
    #[must_use]
    pub fn merge(parent: Self, child: Self) -> Self {
        Self {
            name: child.name,
            version: child.version,
            description: child.description,
            extends: None,
            domain: child.domain.or(parent.domain),
            pipeline: Pipeline {
                plan: merge_phase(parent.pipeline.plan, child.pipeline.plan),
                define: merge_phase(parent.pipeline.define, child.pipeline.define),
                build: merge_phase(parent.pipeline.build, child.pipeline.build),
                merge: merge_phase(parent.pipeline.merge, child.pipeline.merge),
            },
        }
    }
}

fn merge_phase(parent: Vec<PipelineEntry>, child: Vec<PipelineEntry>) -> Vec<PipelineEntry> {
    let mut out: Vec<PipelineEntry> = Vec::with_capacity(parent.len() + child.len());
    for entry in parent {
        if let Some(override_entry) = child.iter().find(|c| c.id == entry.id) {
            out.push(override_entry.clone());
        } else {
            out.push(entry);
        }
    }
    for entry in child {
        if !out.iter().any(|e| e.id == entry.id) {
            out.push(entry);
        }
    }
    out
}

fn locate_schema_root(
    schema_value: &str, project_dir: &Path,
) -> Result<(PathBuf, SchemaSource), Error> {
    let cache_dir = project_dir.join(".specify").join(".cache");
    if schema_value.contains("://") {
        let name = schema_value
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .map(|seg| seg.split('@').next().unwrap_or(seg))
            .ok_or_else(|| {
                Error::SchemaResolution(format!(
                    "cannot derive a schema name from URL `{schema_value}`"
                ))
            })?;
        let candidate = cache_dir.join(name);
        if candidate.is_dir() {
            return Ok((candidate.clone(), SchemaSource::Cached(candidate)));
        }
        return Err(Error::SchemaResolution(format!(
            "schema `{schema_value}` not present under {}; the agent must fetch it before the CLI can resolve",
            cache_dir.display()
        )));
    }

    if schema_value.contains('/') {
        return Err(Error::SchemaResolution(format!(
            "schema value `{schema_value}` looks like a path but is not a URL; use a bare name or a full URL"
        )));
    }

    let cached = cache_dir.join(schema_value);
    if cached.is_dir() {
        return Ok((cached.clone(), SchemaSource::Cached(cached)));
    }

    let local = project_dir.join("schemas").join(schema_value);
    if local.is_dir() {
        return Ok((local.clone(), SchemaSource::Local(local)));
    }

    Err(Error::SchemaResolution(format!(
        "schema `{schema_value}` not found under {} or {}",
        cached.display(),
        local.display()
    )))
}

pub fn validate_against_embedded_schema(
    schema_source: &str, pass_rule_id: &'static str, pass_rule: &'static str,
    instance: &serde_json::Value,
) -> Vec<ValidationResult> {
    let meta_schema: serde_json::Value = match serde_json::from_str(schema_source) {
        Ok(value) => value,
        Err(err) => {
            return vec![ValidationResult::Fail {
                rule_id: "schema.meta-loadable",
                rule: "embedded JSON Schema parses as JSON",
                detail: err.to_string(),
            }];
        }
    };
    let validator = match jsonschema::validator_for(&meta_schema) {
        Ok(v) => v,
        Err(err) => {
            return vec![ValidationResult::Fail {
                rule_id: "schema.meta-compilable",
                rule: "embedded JSON Schema compiles",
                detail: err.to_string(),
            }];
        }
    };

    let errors: Vec<String> =
        validator.iter_errors(instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();

    if errors.is_empty() {
        vec![ValidationResult::Pass {
            rule_id: pass_rule_id,
            rule: pass_rule,
        }]
    } else {
        vec![ValidationResult::Fail {
            rule_id: pass_rule_id,
            rule: pass_rule,
            detail: errors.join("; "),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_display_matches_serde_wire_format() {
        assert_eq!(Phase::Plan.to_string(), "plan");
        assert_eq!(Phase::Define.to_string(), "define");
        assert_eq!(Phase::Build.to_string(), "build");
        assert_eq!(Phase::Merge.to_string(), "merge");
    }
}
