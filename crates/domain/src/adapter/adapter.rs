//! In-memory model of `adapter.yaml` (`Adapter`, `Pipeline`,
//! `PipelineEntry`, `Phase`, `ResolvedAdapter`, `AdapterSource`)
//! plus local / cache resolution. Remote resolution is the agent's job.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, ValidationStatus, ValidationSummary};

use crate::schema::validate_value;

const ADAPTER_JSON_SCHEMA: &str = include_str!("../../../../schemas/adapter.schema.json");

/// In-memory representation of a `adapter.yaml` manifest.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Adapter {
    /// Adapter name (e.g. `"omnia"`).
    pub name: String,
    /// Adapter version number.
    pub version: u32,
    /// Human-readable description of this adapter.
    pub description: String,
    /// The pipeline of briefs organised by phase.
    pub pipeline: Pipeline,
}

/// Pipeline phases and their brief entries.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Pipeline {
    /// Optional Layer 2 authoring-phase briefs for `/change:draft`.
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

/// A `Adapter` plus the directory it was resolved from and how it got
/// there.
#[derive(Debug)]
pub struct ResolvedAdapter {
    /// The parsed adapter manifest.
    pub manifest: Adapter,
    /// Filesystem directory the manifest was loaded from.
    pub root_dir: PathBuf,
    /// How the manifest was located (local workspace or agent cache).
    pub source: AdapterSource,
}

/// How a adapter manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdapterSource {
    /// Resolved from a local `schemas/<name>/` directory.
    Local(PathBuf),
    /// Resolved from the agent-populated `.specify/.cache/<name>/` directory.
    Cached(PathBuf),
}

/// The phases of a adapter's pipeline.
///
/// Serializes as the lowercase identifiers `plan | define | build | merge`
/// on the wire — this is the same wire format consumed by
/// `SliceMetadata.outcome.phase` and by `pipeline.*` keys in the
/// manifest, keeping a single source of truth for phase naming.
///
/// `Plan` is the Layer 2 authoring phase (`/change:draft`) that runs
/// ahead of the define→build→merge execution loop. It is intentionally
/// omitted from `Adapter::entries()` (see that iterator's docs) —
/// call `Adapter::plan_entries()` to enumerate plan-phase briefs.
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
    /// Layer 2 authoring phase (`/change:draft`).
    Plan,
    /// Define phase — artifact generation.
    Define,
    /// Build phase — implementation.
    Build,
    /// Merge phase — finalisation and landing.
    Merge,
}

/// Filename of a adapter manifest.
pub const ADAPTER_FILENAME: &str = "adapter.yaml";

impl Adapter {
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
    pub fn resolve(schema_value: &str, project_dir: &Path) -> Result<ResolvedAdapter, Error> {
        let (root_dir, source) = Self::locate(schema_value, project_dir)?;
        let manifest_path = Self::probe_dir(&root_dir).ok_or_else(|| Error::Diag {
            code: "adapter-manifest-missing",
            detail: format!("no `adapter.yaml` at {}", root_dir.display()),
        })?;
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| Error::Diag {
            code: "adapter-manifest-read-failed",
            detail: format!("failed to read adapter manifest {}: {err}", manifest_path.display()),
        })?;
        let manifest: Self = serde_saphyr::from_str(&raw).map_err(|err| Error::Diag {
            code: "adapter-manifest-malformed",
            detail: format!("failed to parse {}: {err}", manifest_path.display()),
        })?;

        Ok(ResolvedAdapter {
            manifest,
            root_dir,
            source,
        })
    }

    /// Locate the directory `schema_value` resolves to without reading
    /// the manifest. Mirrors [`Adapter::resolve`]'s search order
    /// (cache → local).
    ///
    /// # Errors
    ///
    /// Returns the same resolution diagnostics `resolve` would.
    pub fn locate(
        schema_value: &str, project_dir: &Path,
    ) -> Result<(PathBuf, AdapterSource), Error> {
        locate_adapter_root(schema_value, project_dir)
    }

    /// Probe `dir` for a `adapter.yaml` manifest without reading it.
    /// Returns `Some(path)` when present, `None` otherwise.
    #[must_use]
    pub fn probe_dir(dir: &Path) -> Option<PathBuf> {
        let cap = dir.join(ADAPTER_FILENAME);
        cap.is_file().then_some(cap)
    }

    /// Validate this in-memory adapter against the embedded
    /// `schemas/adapter.schema.json`. Returns one [`ValidationSummary`]
    /// per check performed.
    #[must_use]
    pub fn validate_structure(&self) -> Vec<ValidationSummary> {
        let schema_value: serde_json::Value = match serde_json::to_value(self) {
            Ok(value) => value,
            Err(err) => {
                return vec![ValidationSummary {
                    status: ValidationStatus::Fail,
                    rule_id: "adapter.serializable".into(),
                    rule: "adapter is serializable to JSON".into(),
                    detail: Some(err.to_string()),
                }];
            }
        };
        validate_value(
            &schema_value,
            ADAPTER_JSON_SCHEMA,
            "adapter.valid",
            "adapter manifest conforms to schemas/adapter.schema.json",
        )
    }

    /// Iterator over every execution-loop pipeline entry in order
    /// (define → build → merge), paired with its phase.
    ///
    /// This intentionally skips `pipeline.plan`: the plan phase is an
    /// authoring-time step driven by `/change:draft` and is not part of
    /// the per-change execution loop that `specify change status`,
    /// `specify change outcome`, and the define/build/merge skills
    /// iterate over. Plan briefs are exposed via
    /// [`Adapter::plan_entries`] instead so existing callers keep
    /// their current semantics.
    pub fn entries(&self) -> impl Iterator<Item = (Phase, &PipelineEntry)> + '_ {
        self.pipeline
            .define
            .iter()
            .map(|e| (Phase::Define, e))
            .chain(self.pipeline.build.iter().map(|e| (Phase::Build, e)))
            .chain(self.pipeline.merge.iter().map(|e| (Phase::Merge, e)))
    }

    /// Plan-phase (Layer 2 authoring) pipeline entries in declared
    /// order. Returns an empty slice for adapters that don't declare
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

fn locate_adapter_root(
    schema_value: &str, project_dir: &Path,
) -> Result<(PathBuf, AdapterSource), Error> {
    let cache_dir = project_dir.join(".specify").join(".cache");
    if schema_value.contains("://") {
        let name = schema_value
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .map(|seg| seg.split('@').next().unwrap_or(seg))
            .ok_or_else(|| Error::Diag {
                code: "adapter-url-name-unresolved",
                detail: format!("cannot derive a adapter name from URL `{schema_value}`"),
            })?;
        let candidate = cache_dir.join(name);
        if candidate.is_dir() {
            return Ok((candidate.clone(), AdapterSource::Cached(candidate)));
        }
        return Err(Error::Diag {
            code: "adapter-cache-missing",
            detail: format!(
                "adapter `{schema_value}` not present under {}; the agent must fetch it before the CLI can resolve",
                cache_dir.display()
            ),
        });
    }

    if schema_value.contains('/') {
        return Err(Error::Diag {
            code: "adapter-value-malformed",
            detail: format!(
                "adapter value `{schema_value}` looks like a path but is not a URL; use a bare name or a full URL"
            ),
        });
    }

    let cached = cache_dir.join(schema_value);
    if cached.is_dir() {
        return Ok((cached.clone(), AdapterSource::Cached(cached)));
    }

    let local = project_dir.join("schemas").join(schema_value);
    if local.is_dir() {
        return Ok((local.clone(), AdapterSource::Local(local)));
    }

    Err(Error::Diag {
        code: "adapter-not-found",
        detail: format!(
            "adapter `{schema_value}` not found under {} or {}",
            cached.display(),
            local.display()
        ),
    })
}
