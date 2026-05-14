//! In-memory model of `capability.yaml` (`Capability`, `Pipeline`,
//! `PipelineEntry`, `Phase`, `ResolvedCapability`, `CapabilitySource`)
//! plus local / cache resolution. Remote resolution is the agent's job.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::capability::ValidationResult;

const CAPABILITY_JSON_SCHEMA: &str = include_str!("../../../../schemas/capability.schema.json");

/// In-memory representation of a `capability.yaml` manifest.
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
    /// Optional Layer 3 authoring-phase briefs for `/change:plan`.
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
/// `Plan` is the Layer 3 authoring phase (`/change:plan`) that runs
/// ahead of the define→build→merge execution loop. It is intentionally
/// omitted from `Capability::entries()` (see that iterator's docs) —
/// call `Capability::plan_entries()` to enumerate plan-phase briefs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum Phase {
    /// Layer 3 authoring phase (`/change:plan`).
    Plan,
    /// Define phase — artifact generation.
    Define,
    /// Build phase — implementation.
    Build,
    /// Merge phase — finalisation and landing.
    Merge,
}

/// Filename of a capability manifest.
pub const CAPABILITY_FILENAME: &str = "capability.yaml";

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
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn resolve(schema_value: &str, project_dir: &Path) -> Result<ResolvedCapability, Error> {
        let (root_dir, source) = Self::locate(schema_value, project_dir)?;
        let manifest_path = Self::probe_dir(&root_dir).ok_or_else(|| Error::Diag {
            code: "capability-manifest-missing",
            detail: format!("no `capability.yaml` at {}", root_dir.display()),
        })?;
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| Error::Diag {
            code: "capability-manifest-read-failed",
            detail: format!(
                "failed to read capability manifest {}: {err}",
                manifest_path.display()
            ),
        })?;
        let manifest: Self = serde_saphyr::from_str(&raw).map_err(|err| Error::Diag {
            code: "capability-manifest-malformed",
            detail: format!("failed to parse {}: {err}", manifest_path.display()),
        })?;

        Ok(ResolvedCapability {
            manifest,
            root_dir,
            source,
        })
    }

    /// Locate the directory `schema_value` resolves to without reading
    /// the manifest. Mirrors [`Capability::resolve`]'s search order
    /// (cache → local).
    ///
    /// # Errors
    ///
    /// Returns the same resolution diagnostics `resolve` would.
    pub fn locate(
        schema_value: &str, project_dir: &Path,
    ) -> Result<(PathBuf, CapabilitySource), Error> {
        locate_capability_root(schema_value, project_dir)
    }

    /// Probe `dir` for a `capability.yaml` manifest without reading it.
    /// Returns `Some(path)` when present, `None` otherwise.
    #[must_use]
    pub fn probe_dir(dir: &Path) -> Option<PathBuf> {
        let cap = dir.join(CAPABILITY_FILENAME);
        cap.is_file().then_some(cap)
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
                    rule_id: "capability.serializable".into(),
                    rule: "capability is serializable to JSON".into(),
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
    /// authoring-time step driven by `/change:plan` and is not part of
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
            .ok_or_else(|| Error::Diag {
                code: "capability-url-name-unresolved",
                detail: format!("cannot derive a capability name from URL `{schema_value}`"),
            })?;
        let candidate = cache_dir.join(name);
        if candidate.is_dir() {
            return Ok((candidate.clone(), CapabilitySource::Cached(candidate)));
        }
        return Err(Error::Diag {
            code: "capability-cache-missing",
            detail: format!(
                "capability `{schema_value}` not present under {}; the agent must fetch it before the CLI can resolve",
                cache_dir.display()
            ),
        });
    }

    if schema_value.contains('/') {
        return Err(Error::Diag {
            code: "capability-value-malformed",
            detail: format!(
                "capability value `{schema_value}` looks like a path but is not a URL; use a bare name or a full URL"
            ),
        });
    }

    let cached = cache_dir.join(schema_value);
    if cached.is_dir() {
        return Ok((cached.clone(), CapabilitySource::Cached(cached)));
    }

    let local = project_dir.join("schemas").join(schema_value);
    if local.is_dir() {
        return Ok((local.clone(), CapabilitySource::Local(local)));
    }

    Err(Error::Diag {
        code: "capability-not-found",
        detail: format!(
            "capability `{schema_value}` not found under {} or {}",
            cached.display(),
            local.display()
        ),
    })
}

/// Validate `instance` against the embedded JSON Schema `schema_source`.
///
/// Emits one `ValidationResult` per error plus a single `Pass` (tagged with
/// `pass_rule_id` / `pass_rule`) when the schema accepts the value.
#[must_use]
pub fn validate_against_schema(
    schema_source: &str, pass_rule_id: &'static str, pass_rule: &'static str,
    instance: &serde_json::Value,
) -> Vec<ValidationResult> {
    let meta_schema: serde_json::Value = match serde_json::from_str(schema_source) {
        Ok(value) => value,
        Err(err) => {
            return vec![ValidationResult::Fail {
                rule_id: "schema.meta-loadable".into(),
                rule: "embedded JSON Schema parses as JSON".into(),
                detail: err.to_string(),
            }];
        }
    };
    let validator = match jsonschema::validator_for(&meta_schema) {
        Ok(v) => v,
        Err(err) => {
            return vec![ValidationResult::Fail {
                rule_id: "schema.meta-compilable".into(),
                rule: "embedded JSON Schema compiles".into(),
                detail: err.to_string(),
            }];
        }
    };

    let errors: Vec<String> =
        validator.iter_errors(instance).map(|e| format!("{}: {}", e.instance_path(), e)).collect();

    if errors.is_empty() {
        vec![ValidationResult::Pass {
            rule_id: pass_rule_id.into(),
            rule: pass_rule.into(),
        }]
    } else {
        vec![ValidationResult::Fail {
            rule_id: pass_rule_id.into(),
            rule: pass_rule.into(),
            detail: errors.join("; "),
        }]
    }
}
