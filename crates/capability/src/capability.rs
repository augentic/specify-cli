//! `Capability`, `Pipeline`, `PipelineEntry`, `Phase`, `ResolvedCapability`,
//! `CapabilitySource` — the in-memory model of `capability.yaml` (with
//! a tolerant fallback to the pre-RFC-13 `schema.yaml` filename) plus
//! the local / cache resolution algorithm. Remote (HTTP) resolution is
//! explicitly the agent's job per RFC-1; this crate only walks the
//! filesystem.
//!
//! Phase 1.1 (RFC-13) renames the extension-primitive types from
//! `Schema` / `ResolvedSchema` / `SchemaSource` to `Capability` /
//! `ResolvedCapability` / `CapabilitySource`. Phase 1.2 routes the CLI
//! through `capability {resolve,check,pipeline}` and adds the loud
//! `schema-became-capability` diagnostic that fires when the binary
//! finds a legacy `schema.yaml` instead of `capability.yaml`. The
//! resolver itself stays tolerant — `init` and other internal callers
//! that still read `schema.yaml` on disk continue to work — so the
//! diagnostic surfaces only at the CLI command boundary.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::ValidationResult;

const CAPABILITY_JSON_SCHEMA: &str = include_str!("../../../schemas/capability.schema.json");

/// In-memory representation of a `capability.yaml` manifest.
///
/// The resolver still tolerates the pre-RFC-13 `schema.yaml` filename
/// so internal callers can read older on-disk manifests during the
/// cut-over; the `specify capability *` CLI surface refuses the legacy
/// shape with [`Error::LegacyCapabilityField`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Capability {
    /// Capability name (e.g. `"omnia"`).
    pub name: String,
    /// Capability version number.
    pub version: u32,
    /// Human-readable description of this capability.
    pub description: String,
    /// The pipeline of briefs organised by phase.
    pub pipeline: Pipeline,
}

/// Pipeline phases and their brief entries.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Pipeline {
    /// Optional Layer 3 authoring-phase briefs for `/spec:plan`.
    /// Absent in pre-existing manifests; present ones expose briefs such
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

/// A `Capability` plus the directory it was resolved from and how it got
/// there.
#[derive(Debug)]
pub struct ResolvedCapability {
    /// The parsed capability manifest.
    pub manifest: Capability,
    /// Filesystem directory the manifest was loaded from.
    pub root_dir: PathBuf,
    /// How the manifest was located (local workspace or agent cache).
    pub source: CapabilitySource,
}

/// How a capability manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilitySource {
    /// Resolved from a local `schemas/<name>/` directory.
    Local(PathBuf),
    /// Resolved from the agent-populated `.specify/.cache/<name>/` directory.
    Cached(PathBuf),
}

/// The phases of a capability's pipeline.
///
/// Serializes as the lowercase identifiers `plan | define | build | merge`
/// on the wire — this is the same wire format consumed by
/// `SliceMetadata.outcome.phase` and by `pipeline.*` keys in the
/// manifest, keeping a single source of truth for phase naming.
///
/// `Plan` is the Layer 3 authoring phase (`/spec:plan`) that runs
/// ahead of the define→build→merge execution loop. It is intentionally
/// omitted from `Capability::entries()` (see that iterator's docs) —
/// call `Capability::plan_entries()` to enumerate plan-phase briefs.
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

/// Filename of a post-RFC-13 capability manifest.
pub const CAPABILITY_FILENAME: &str = "capability.yaml";

/// Pre-RFC-13 filename of a capability manifest.
///
/// Still loaded by the resolver (so `init` and other internal callers
/// keep working) but the `specify capability *` CLI surface refuses to
/// load a directory that carries only this filename and emits
/// [`Error::LegacyCapabilityField`] instead.
pub const LEGACY_SCHEMA_FILENAME: &str = "schema.yaml";

/// Result of [`Capability::probe_dir`]. Names whether the
/// directory carries the post-RFC-13 manifest, the pre-RFC-13 manifest,
/// or neither — without doing any I/O beyond two `is_file` probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestProbe {
    /// `<dir>/capability.yaml` exists.
    Found(PathBuf),
    /// Only `<dir>/schema.yaml` exists. The CLI surface translates this
    /// into [`Error::LegacyCapabilityField`].
    Legacy(PathBuf),
    /// Neither filename is present.
    Missing,
}

impl Capability {
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
    /// The resolver prefers `capability.yaml` and falls back to the
    /// pre-RFC-13 `schema.yaml`. The fallback keeps internal callers
    /// (notably `init`) working during the cut-over; the loud
    /// [`Error::LegacyCapabilityField`] diagnostic for the legacy
    /// shape is surfaced by the CLI command layer in
    /// `src/commands/capability.rs`.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn resolve(schema_value: &str, project_dir: &Path) -> Result<ResolvedCapability, Error> {
        let (root_dir, source) = Self::locate(schema_value, project_dir)?;
        let manifest_path = match Self::probe_dir(&root_dir) {
            ManifestProbe::Found(path) | ManifestProbe::Legacy(path) => path,
            ManifestProbe::Missing => {
                return Err(Error::SchemaResolution(format!(
                    "no capability manifest at {} (expected `{}` or legacy `{}`)",
                    root_dir.display(),
                    CAPABILITY_FILENAME,
                    LEGACY_SCHEMA_FILENAME
                )));
            }
        };
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| {
            Error::SchemaResolution(format!(
                "failed to read capability manifest {}: {err}",
                manifest_path.display()
            ))
        })?;
        let manifest: Self = serde_saphyr::from_str(&raw).map_err(|err| {
            Error::SchemaResolution(format!("failed to parse {}: {err}", manifest_path.display()))
        })?;

        Ok(ResolvedCapability {
            manifest,
            root_dir,
            source,
        })
    }

    /// Locate the directory `schema_value` resolves to without reading
    /// the manifest. Mirrors [`Capability::resolve`]'s search order
    /// (cache → local) and is the entry point the CLI command layer
    /// uses to inspect the resolved directory before turning a legacy
    /// `schema.yaml` into [`Error::LegacyCapabilityField`].
    ///
    /// # Errors
    ///
    /// Returns the same `SchemaResolution` errors `resolve` would.
    pub fn locate(
        schema_value: &str, project_dir: &Path,
    ) -> Result<(PathBuf, CapabilitySource), Error> {
        locate_capability_root(schema_value, project_dir)
    }

    /// Probe `dir` for a capability manifest without reading it. Returns
    /// the post-RFC-13 path when present, otherwise the pre-RFC-13
    /// fallback path, otherwise `Missing`.
    #[must_use]
    pub fn probe_dir(dir: &Path) -> ManifestProbe {
        let cap = dir.join(CAPABILITY_FILENAME);
        if cap.is_file() {
            return ManifestProbe::Found(cap);
        }
        let legacy = dir.join(LEGACY_SCHEMA_FILENAME);
        if legacy.is_file() {
            return ManifestProbe::Legacy(legacy);
        }
        ManifestProbe::Missing
    }

    /// Validate this in-memory capability against the embedded
    /// `schemas/capability.schema.json`. Returns one `ValidationResult`
    /// per check performed (empty = fully valid).
    #[must_use]
    pub fn validate_structure(&self) -> Vec<ValidationResult> {
        let schema_value: serde_json::Value = match serde_json::to_value(self) {
            Ok(value) => value,
            Err(err) => {
                return vec![ValidationResult::Fail {
                    rule_id: "capability.serializable",
                    rule: "capability is serializable to JSON",
                    detail: err.to_string(),
                }];
            }
        };
        validate_against_schema(
            CAPABILITY_JSON_SCHEMA,
            "capability.valid",
            "capability manifest conforms to schemas/capability.schema.json",
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
    /// [`Capability::plan_entries`] instead so existing callers keep
    /// their current semantics.
    pub fn entries(&self) -> impl Iterator<Item = (Phase, &PipelineEntry)> + '_ {
        self.pipeline
            .define
            .iter()
            .map(|e| (Phase::Define, e))
            .chain(self.pipeline.build.iter().map(|e| (Phase::Build, e)))
            .chain(self.pipeline.merge.iter().map(|e| (Phase::Merge, e)))
    }

    /// Plan-phase (Layer 3 authoring) pipeline entries in declared
    /// order. Returns an empty slice for capabilities that don't declare
    /// a `pipeline.plan` block.
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

    /// Merge `child` on top of `parent`. Per the historical
    /// schema-resolution rules:
    ///
    /// - `pipeline`: for each phase, child entries with the same `id`
    ///   replace the parent's entry in place; new ids are appended in
    ///   child order.
    /// - All other top-level fields (`name`, `version`, `description`)
    ///   come from the child.
    #[must_use]
    pub fn merge(parent: Self, child: Self) -> Self {
        Self {
            name: child.name,
            version: child.version,
            description: child.description,
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

fn locate_capability_root(
    schema_value: &str, project_dir: &Path,
) -> Result<(PathBuf, CapabilitySource), Error> {
    let cache_dir = project_dir.join(".specify").join(".cache");
    if schema_value.contains("://") {
        let name = schema_value
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .map(|seg| seg.split('@').next().unwrap_or(seg))
            .ok_or_else(|| {
                Error::SchemaResolution(format!(
                    "cannot derive a capability name from URL `{schema_value}`"
                ))
            })?;
        let candidate = cache_dir.join(name);
        if candidate.is_dir() {
            return Ok((candidate.clone(), CapabilitySource::Cached(candidate)));
        }
        return Err(Error::SchemaResolution(format!(
            "capability `{schema_value}` not present under {}; the agent must fetch it before the CLI can resolve",
            cache_dir.display()
        )));
    }

    if schema_value.contains('/') {
        return Err(Error::SchemaResolution(format!(
            "capability value `{schema_value}` looks like a path but is not a URL; use a bare name or a full URL"
        )));
    }

    let cached = cache_dir.join(schema_value);
    if cached.is_dir() {
        return Ok((cached.clone(), CapabilitySource::Cached(cached)));
    }

    let local = project_dir.join("schemas").join(schema_value);
    if local.is_dir() {
        return Ok((local.clone(), CapabilitySource::Local(local)));
    }

    Err(Error::SchemaResolution(format!(
        "capability `{schema_value}` not found under {} or {}",
        cached.display(),
        local.display()
    )))
}

pub fn validate_against_schema(
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
