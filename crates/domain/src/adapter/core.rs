//! `Adapter`, `Axis`, `ResolvedAdapter`, and the axis-aware loader.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, ValidationStatus};

use crate::schema::validate_value;

/// Filename of an adapter manifest.
///
/// Source and target adapters share the `adapter.yaml` filename per
/// RFC-25 §Adapter implementation shape; the directory's axis (under
/// `sources/` or `targets/`) and the manifest's `axis:` field
/// disambiguate.
pub const ADAPTER_FILENAME: &str = "adapter.yaml";

const ADAPTER_JSON_SCHEMA: &str = include_str!("../../../../schemas/adapter.schema.json");
const SOURCE_JSON_SCHEMA: &str = include_str!("../../../../schemas/source.schema.json");
const TARGET_JSON_SCHEMA: &str = include_str!("../../../../schemas/target.schema.json");

/// Axis discriminator for an adapter manifest.
///
/// Source vs target — see RFC-25 §Adapter axis.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Axis {
    /// Source adapter — `enumerate` + `extract`.
    Source,
    /// Target adapter — `shape` + `build` + `merge`.
    Target,
}

impl Axis {
    /// Directory segment under `<project_dir>` and `.specify/.cache/`
    /// — `"sources"` for source adapters, `"targets"` for target adapters.
    #[must_use]
    pub const fn dir_segment(self) -> &'static str {
        match self {
            Self::Source => "sources",
            Self::Target => "targets",
        }
    }
}

/// One declared WASI tool inside an adapter manifest.
///
/// Decoupled from [`specify_tool::manifest::Tool`] so adapter loading
/// does not pull in the WASI runtime surface; sidecar `tools.yaml`
/// continues to be the authoritative source for tool resolution.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdapterToolDeclaration {
    /// Kebab-case tool name.
    pub name: String,
    /// Optional semver or `sha256:<digest>` version pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Optional permission grants forwarded to the WASI runner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
}

/// In-memory representation of an adapter manifest.
///
/// Loaded from `sources/<name>/adapter.yaml` or
/// `targets/<name>/adapter.yaml`; the shape is the union of
/// `schemas/source.schema.json` and `schemas/target.schema.json`, with
/// the axis-specific refinements (closed operation sets, axis literal)
/// enforced by [`Adapter::resolve`] via the matching schema.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    /// Kebab-case adapter name; must match the directory under
    /// `<axis-dir>/<name>/`.
    pub name: String,
    /// Major adapter version.
    pub version: u32,
    /// Axis discriminator.
    pub axis: Axis,
    /// Closed list of operations. Sources: `enumerate`, `extract`.
    /// Targets: `shape`, `build`, `merge`.
    pub operations: Vec<String>,
    /// Map from operation name to a relative brief path.
    pub briefs: BTreeMap<String, String>,
    /// Optional declared WASI tools per RFC-15.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdapterToolDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A parsed [`Adapter`] paired with the directory it loaded from and
/// where it was located (in-repo vs. agent-populated cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAdapter {
    /// Parsed manifest.
    pub manifest: Adapter,
    /// Filesystem directory the manifest was loaded from.
    pub root_dir: PathBuf,
    /// Whether the manifest came from `.specify/.cache/{axis}/<name>/`
    /// or from `<project_dir>/{axis}/<name>/`.
    pub location: AdapterLocation,
}

/// Where an adapter manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterLocation {
    /// Resolved from `<project_dir>/{axis}/<name>/`.
    Local(PathBuf),
    /// Resolved from `<project_dir>/.specify/.cache/{axis}/<name>/`.
    Cached(PathBuf),
}

impl AdapterLocation {
    /// Kebab-case label for JSON envelopes (`"local"` / `"cached"`).
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Cached(_) => "cached",
        }
    }

    /// Underlying filesystem path.
    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        match self {
            Self::Local(path) | Self::Cached(path) => path,
        }
    }
}

/// `.specify/.cache/{sources,targets}/<name>/` for a given axis and name.
///
/// Path-only helper — the directory may or may not exist on disk.
#[must_use]
pub fn cache_dir(project_dir: &Path, axis: Axis, name: &str) -> PathBuf {
    project_dir.join(".specify").join(".cache").join(axis.dir_segment()).join(name)
}

impl Adapter {
    /// Resolve an adapter by axis and kebab-case name.
    ///
    /// Probe order, per RFC-25 §Resolver and cache:
    ///
    /// 1. `<project_dir>/.specify/.cache/{axis}/<name>/adapter.yaml`
    ///    (agent-populated cache).
    /// 2. `<project_dir>/{axis}/<name>/adapter.yaml` (in-repo).
    ///
    /// Returns the parsed manifest, the directory it came from, and an
    /// [`AdapterLocation`] tag for downstream renderers. The loader is
    /// path-agnostic: tests pass a temp `project_dir` containing the
    /// fixtures.
    ///
    /// # Errors
    ///
    /// Returns `Error::Diag` with one of the following codes:
    /// - `adapter-not-found` — neither cache nor local directory exists.
    /// - `adapter-manifest-missing` — directory exists but no `adapter.yaml`.
    /// - `adapter-manifest-read-failed` — manifest exists but cannot be read.
    /// - `adapter-manifest-malformed` — manifest parses as something
    ///   other than the [`Adapter`] shape.
    /// - `adapter-axis-mismatch` — manifest's `axis:` does not match the
    ///   requested axis.
    /// - `adapter-name-mismatch` — manifest's `name:` does not match the
    ///   requested name.
    /// - `adapter-schema-violation` — manifest fails the
    ///   axis-specific JSON Schema.
    pub fn resolve(axis: Axis, name: &str, project_dir: &Path) -> Result<ResolvedAdapter, Error> {
        let (root_dir, location) = Self::locate(axis, name, project_dir)?;
        let manifest_path = root_dir.join(ADAPTER_FILENAME);
        if !manifest_path.is_file() {
            return Err(Error::Diag {
                code: "adapter-manifest-missing",
                detail: format!("no `adapter.yaml` at {}", root_dir.display()),
            });
        }
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| Error::Diag {
            code: "adapter-manifest-read-failed",
            detail: format!("failed to read adapter manifest {}: {err}", manifest_path.display()),
        })?;

        // Validate against the schema first so a more specific error
        // bubbles up than serde's free-form parse failure.
        let raw_value: serde_json::Value =
            serde_saphyr::from_str(&raw).map_err(|err| Error::Diag {
                code: "adapter-manifest-malformed",
                detail: format!("failed to parse {}: {err}", manifest_path.display()),
            })?;
        validate_schema(axis, &manifest_path, &raw_value)?;

        let manifest: Self = serde_saphyr::from_str(&raw).map_err(|err| Error::Diag {
            code: "adapter-manifest-malformed",
            detail: format!("failed to deserialize {}: {err}", manifest_path.display()),
        })?;

        if manifest.axis != axis {
            return Err(Error::Diag {
                code: "adapter-axis-mismatch",
                detail: format!(
                    "{} declares axis `{}`, but resolver was asked for axis `{axis}`",
                    manifest_path.display(),
                    manifest.axis
                ),
            });
        }
        if manifest.name != name {
            return Err(Error::Diag {
                code: "adapter-name-mismatch",
                detail: format!(
                    "{} declares name `{}` but lives under `{name}/`",
                    manifest_path.display(),
                    manifest.name
                ),
            });
        }

        Ok(ResolvedAdapter {
            manifest,
            root_dir,
            location,
        })
    }

    /// Locate the directory `(axis, name)` resolves to without reading
    /// the manifest. Mirrors [`Adapter::resolve`]'s probe order
    /// (cache → local).
    ///
    /// # Errors
    ///
    /// Returns the same `adapter-not-found` diagnostic [`Adapter::resolve`]
    /// would.
    pub fn locate(
        axis: Axis, name: &str, project_dir: &Path,
    ) -> Result<(PathBuf, AdapterLocation), Error> {
        let cached = cache_dir(project_dir, axis, name);
        if cached.is_dir() {
            return Ok((cached.clone(), AdapterLocation::Cached(cached)));
        }
        let local = project_dir.join(axis.dir_segment()).join(name);
        if local.is_dir() {
            return Ok((local.clone(), AdapterLocation::Local(local)));
        }
        Err(Error::Diag {
            code: "adapter-not-found",
            detail: format!(
                "adapter `{name}` (axis `{axis}`) not found at {} or {}",
                cached.display(),
                local.display()
            ),
        })
    }

    /// Resolve the manifest's brief path for `operation` against
    /// [`ResolvedAdapter::root_dir`]. Returns `None` when the operation
    /// is not declared by this manifest.
    #[must_use]
    pub fn brief_path(&self, root_dir: &Path, operation: &str) -> Option<PathBuf> {
        self.briefs.get(operation).map(|relative| root_dir.join(relative))
    }
}

impl ResolvedAdapter {
    /// Convenience accessor combining [`Adapter::brief_path`] and the
    /// resolved [`ResolvedAdapter::root_dir`].
    #[must_use]
    pub fn brief_path(&self, operation: &str) -> Option<PathBuf> {
        self.manifest.brief_path(&self.root_dir, operation)
    }
}

fn validate_schema(
    axis: Axis, manifest_path: &Path, instance: &serde_json::Value,
) -> Result<(), Error> {
    // Shape gate first — catches violations both schemas share.
    run_schema(ADAPTER_JSON_SCHEMA, manifest_path, instance, "adapter")?;
    // Axis-specific refinement (operation set + axis literal).
    let (schema, label) = match axis {
        Axis::Source => (SOURCE_JSON_SCHEMA, "source"),
        Axis::Target => (TARGET_JSON_SCHEMA, "target"),
    };
    run_schema(schema, manifest_path, instance, label)
}

fn run_schema(
    schema_source: &str, manifest_path: &Path, instance: &serde_json::Value, label: &str,
) -> Result<(), Error> {
    let rule = format!("{} conforms to embedded {label} schema", manifest_path.display());
    for summary in validate_value(instance, schema_source, "adapter-schema-violation", &rule) {
        if summary.status == ValidationStatus::Fail {
            return Err(Error::Diag {
                code: "adapter-schema-violation",
                detail: format!(
                    "{} violates {label} schema: {}",
                    manifest_path.display(),
                    summary.detail.unwrap_or_default()
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_dir_segment_round_trips() {
        assert_eq!(Axis::Source.dir_segment(), "sources");
        assert_eq!(Axis::Target.dir_segment(), "targets");
    }

    #[test]
    fn cache_dir_routes_by_axis() {
        let project = Path::new("/proj");
        assert_eq!(
            cache_dir(project, Axis::Source, "documentation"),
            project.join(".specify/.cache/sources/documentation")
        );
        assert_eq!(
            cache_dir(project, Axis::Target, "omnia"),
            project.join(".specify/.cache/targets/omnia")
        );
    }
}
