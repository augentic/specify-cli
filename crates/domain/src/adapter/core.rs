//! Axis-split adapter manifests and the per-axis loader entry points.
//!
//! Source adapters and target adapters share a manifest shape on the
//! wire (`adapter.yaml`) but carry disjoint closed operation sets:
//! [`SourceOperation`] (`enumerate | extract`) vs. [`TargetOperation`]
//! (`shape | build | merge`). The in-memory split into
//! [`SourceAdapter`] / [`TargetAdapter`] pushes the kebab-string
//! boundary out to the YAML parse step — `briefs.keys()` is now the
//! typed source-of-truth operation iterator, with serde rejecting any
//! unknown variant before downstream code ever sees a string.
//!
//! See [DECISIONS.md §"Operations typed at parse boundary"] for the
//! rationale.
//!
//! [DECISIONS.md §"Operations typed at parse boundary"]: ../../../../DECISIONS.md#operations-typed-at-parse-boundary

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, ValidationStatus};

use crate::adapter::operation::{SourceOperation, TargetOperation};
use crate::schema::validate_value;

/// Filename of an adapter manifest.
///
/// Source and target adapters share the `adapter.yaml` filename per
/// RFC-25 §Adapter implementation shape; the directory's axis (under
/// `adapters/sources/` or `adapters/targets/`) and the manifest's
/// `axis:` field disambiguate.
pub const ADAPTER_FILENAME: &str = "adapter.yaml";

/// Parent directory for in-repo adapter trees.
pub const ADAPTERS_DIR: &str = "adapters";

/// Manifest-cache root segment under `.specify/.cache/`.
///
/// `.specify/.cache/manifests/{sources,targets}/<name>/` mirrors the
/// in-repo `adapters/{sources,targets}/<name>/` tree. Paired with
/// [`EXTRACTIONS_CACHE_DIR`] so the manifest and RFC-27 §D8 extraction
/// caches own disjoint roots (see [DECISIONS.md §"Cache layout"]).
///
/// [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout
pub const MANIFESTS_CACHE_DIR: &str = "manifests";

/// Extraction-cache root segment under `.specify/.cache/`.
///
/// `.specify/.cache/extractions/<adapter>/<fingerprint>/` holds the
/// RFC-27 §D8 per-source extraction result cache (with `index.jsonl`
/// at the adapter root). Per-adapter only — extraction is a source-axis
/// operation — and partitioned from [`MANIFESTS_CACHE_DIR`] so each
/// cache owns its own tree (no co-tenancy heuristic; see
/// [DECISIONS.md §"Cache layout"]).
///
/// [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout
pub const EXTRACTIONS_CACHE_DIR: &str = "extractions";

const ADAPTER_JSON_SCHEMA: &str = include_str!("../../../../schemas/adapter.schema.json");
const SOURCE_JSON_SCHEMA: &str = include_str!("../../../../schemas/source.schema.json");
const TARGET_JSON_SCHEMA: &str = include_str!("../../../../schemas/target.schema.json");

/// Axis discriminator for an adapter manifest.
///
/// Source vs target — see RFC-25 §Adapter axis. The closed enum is
/// used by the resolver dispatcher (`commands::resolve_adapter`) and
/// the manifest-cache helpers ([`cache_dir`], [`adapter_axis_dir`]);
/// the in-memory manifests themselves are axis-typed
/// ([`SourceAdapter`] / [`TargetAdapter`]) so internal call sites no
/// longer carry the `axis` argument forward past the resolver
/// boundary.
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
    /// Axis segment under [`ADAPTERS_DIR`] — `"sources"` for source
    /// adapters, `"targets"` for target adapters.
    #[must_use]
    pub const fn dir_segment(self) -> &'static str {
        match self {
            Self::Source => "sources",
            Self::Target => "targets",
        }
    }

    /// The complementary axis. Used by the cross-axis uniqueness
    /// probe (see [DECISIONS.md §"Adapter name uniqueness"]) to
    /// reject a name that resolves under both `adapters/sources/` and
    /// `adapters/targets/`.
    ///
    /// [DECISIONS.md §"Adapter name uniqueness"]: ../../../../DECISIONS.md#adapter-name-uniqueness
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Source => Self::Target,
            Self::Target => Self::Source,
        }
    }
}

/// `<project_dir>/adapters/{sources,targets}/` for the given axis.
#[must_use]
pub fn adapter_axis_dir(project_dir: &Path, axis: Axis) -> PathBuf {
    project_dir.join(ADAPTERS_DIR).join(axis.dir_segment())
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

/// Closed enum for the optional `cache:` field on an adapter manifest
/// (RFC-27 §D8). Single variant in v1; widened only behind an RFC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CacheMode {
    /// `cache: opt-out` — the CLI bypasses the cache for every run
    /// of this adapter; the matching `slice.extract.cache-miss`
    /// journal event carries `reason: adapter-opt-out`.
    OptOut,
}

/// Where an adapter manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterLocation {
    /// Resolved from `<project_dir>/adapters/{sources,targets}/<name>/`.
    Local(PathBuf),
    /// Resolved from the manifest cache at
    /// `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`.
    /// The manifest cache mirrors the in-repo adapter tree
    /// (`adapter.yaml` plus brief markdown); the RFC-27 §D8 extraction
    /// result cache lives in a sibling tree under
    /// `.specify/.cache/extractions/<adapter>/` — see
    /// [DECISIONS.md §"Cache layout"].
    ///
    /// [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
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

/// Manifest cache root for `(axis, name)` —
/// `.specify/.cache/manifests/{sources,targets}/<name>/`.
///
/// This is the agent-populated mirror of `adapters/{sources,targets}/<name>/`
/// — `adapter.yaml` plus the brief markdown files it references. The
/// RFC-27 §D8 per-source extraction result cache lives in a separate
/// sibling tree under `.specify/.cache/extractions/<adapter>/` (with
/// `index.jsonl` at the adapter root) so the two caches no longer share
/// a root; see [DECISIONS.md §"Cache layout"].
///
/// Path-only helper — the directory may or may not exist on disk.
///
/// [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
#[must_use]
pub fn cache_dir(project_dir: &Path, axis: Axis, name: &str) -> PathBuf {
    project_dir
        .join(".specify")
        .join(".cache")
        .join(MANIFESTS_CACHE_DIR)
        .join(axis.dir_segment())
        .join(name)
}

/// In-memory representation of a source-adapter manifest
/// (`adapters/sources/<name>/adapter.yaml`).
///
/// Constructed by [`SourceAdapter::resolve`] after the wire YAML has
/// been validated against `schemas/adapter.schema.json` +
/// `schemas/source.schema.json`. The typed `briefs` map carries the
/// closed [`SourceOperation`] set — unknown keys are rejected at
/// serde-parse time before this struct is ever materialised.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceAdapter {
    /// Kebab-case adapter name; must match the directory under
    /// `adapters/sources/<name>/`.
    pub name: String,
    /// Major adapter version.
    pub version: u32,
    /// Axis discriminator on the wire. Always [`Axis::Source`] after a
    /// successful [`SourceAdapter::resolve`]; the field is retained
    /// so YAML round-trips byte-for-byte through serde.
    pub axis: Axis,
    /// Typed source-operation → relative brief path map. Closed by
    /// `source.schema.json#/properties/briefs`: every source manifest
    /// declares `enumerate` + `extract`. `briefs.keys()` is the
    /// canonical operation iterator — exposed via
    /// [`SourceAdapter::operations`].
    pub briefs: BTreeMap<SourceOperation, String>,
    /// Optional declared WASI tools per RFC-15.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdapterToolDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional cache opt-out switch (RFC-27 §D8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheMode>,
}

/// In-memory representation of a target-adapter manifest
/// (`adapters/targets/<name>/adapter.yaml`).
///
/// Constructed by [`TargetAdapter::resolve`] after the wire YAML has
/// been validated against `schemas/adapter.schema.json` +
/// `schemas/target.schema.json`. The typed `briefs` map carries the
/// closed [`TargetOperation`] set — unknown keys are rejected at
/// serde-parse time before this struct is ever materialised.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetAdapter {
    /// Kebab-case adapter name; must match the directory under
    /// `adapters/targets/<name>/`.
    pub name: String,
    /// Major adapter version.
    pub version: u32,
    /// Axis discriminator on the wire. Always [`Axis::Target`] after
    /// a successful [`TargetAdapter::resolve`]; the field is retained
    /// so YAML round-trips byte-for-byte through serde.
    pub axis: Axis,
    /// Typed target-operation → relative brief path map. Closed by
    /// `target.schema.json#/properties/briefs`: every target manifest
    /// declares `shape` + `build` + `merge`. `briefs.keys()` is the
    /// canonical operation iterator — exposed via
    /// [`TargetAdapter::operations`].
    pub briefs: BTreeMap<TargetOperation, String>,
    /// Optional declared WASI tools per RFC-15.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdapterToolDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional cache opt-out switch (RFC-27 §D8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheMode>,
}

/// A parsed [`SourceAdapter`] paired with the directory it loaded from
/// and where it was located (in-repo vs. agent-populated cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSourceAdapter {
    /// Parsed manifest.
    pub manifest: SourceAdapter,
    /// Filesystem directory the manifest was loaded from.
    pub root_dir: PathBuf,
    /// Whether the manifest came from
    /// `.specify/.cache/manifests/sources/<name>/` or from
    /// `<project_dir>/adapters/sources/<name>/`.
    pub location: AdapterLocation,
}

/// A parsed [`TargetAdapter`] paired with the directory it loaded from
/// and where it was located (in-repo vs. agent-populated cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTargetAdapter {
    /// Parsed manifest.
    pub manifest: TargetAdapter,
    /// Filesystem directory the manifest was loaded from.
    pub root_dir: PathBuf,
    /// Whether the manifest came from
    /// `.specify/.cache/manifests/targets/<name>/` or from
    /// `<project_dir>/adapters/targets/<name>/`.
    pub location: AdapterLocation,
}

impl SourceAdapter {
    /// Resolve a source adapter by kebab-case name.
    ///
    /// Probe order, per RFC-25 §Resolver and cache:
    ///
    /// 1. `<project_dir>/.specify/.cache/manifests/sources/<name>/adapter.yaml`
    ///    (agent-populated manifest cache).
    /// 2. `<project_dir>/adapters/sources/<name>/adapter.yaml` (in-repo).
    ///
    /// # Errors
    ///
    /// Returns `Error::Diag` with one of the following codes:
    /// - `adapter-not-found` — neither cache nor local directory exists.
    /// - `adapter-manifest-missing` — directory exists but no `adapter.yaml`.
    /// - `adapter-manifest-read-failed` — manifest exists but cannot be read.
    /// - `adapter-manifest-malformed` — manifest parses as something
    ///   other than the [`SourceAdapter`] shape.
    /// - `adapter-axis-mismatch` — manifest's `axis:` does not match
    ///   [`Axis::Source`].
    /// - `adapter-name-mismatch` — manifest's `name:` does not match
    ///   `name`.
    /// - `adapter-schema-violation` — manifest fails the source-axis
    ///   JSON Schema.
    pub fn resolve(name: &str, project_dir: &Path) -> Result<ResolvedSourceAdapter, Error> {
        let (root_dir, location, manifest_path, raw_value) =
            load_validated(Axis::Source, name, project_dir)?;

        let manifest: Self = serde_json::from_value(raw_value).map_err(|err| Error::Diag {
            code: "adapter-manifest-malformed",
            detail: format!("failed to deserialize {}: {err}", manifest_path.display()),
        })?;

        check_axis_and_name(Axis::Source, name, manifest.axis, &manifest.name, &manifest_path)?;

        Ok(ResolvedSourceAdapter {
            manifest,
            root_dir,
            location,
        })
    }

    /// Locate the source-adapter directory `(Source, name)` resolves
    /// to without reading the manifest. Mirrors
    /// [`SourceAdapter::resolve`]'s probe order (cache → local).
    ///
    /// # Errors
    ///
    /// Returns `adapter-not-found` when neither cache nor local
    /// directory exists.
    pub fn locate(name: &str, project_dir: &Path) -> Result<(PathBuf, AdapterLocation), Error> {
        locate_axis(Axis::Source, name, project_dir)
    }

    /// Resolve the manifest's brief path for `operation` against
    /// `root_dir`. Returns `None` when the operation is not declared
    /// by this manifest. Source manifests always declare both
    /// operations after a successful schema-validated load, so this
    /// returns `Some` in practice.
    #[must_use]
    pub fn brief_path(&self, root_dir: &Path, operation: SourceOperation) -> Option<PathBuf> {
        self.briefs.get(&operation).map(|relative| root_dir.join(relative))
    }

    /// Iterator over the source operations this adapter declares, in
    /// ascending kebab-name order (`enumerate < extract`). After the
    /// collapse of the dedicated `operations[]` field (review 1.A1)
    /// and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &SourceOperation> {
        self.briefs.keys()
    }
}

impl TargetAdapter {
    /// Resolve a target adapter by kebab-case name.
    ///
    /// Probe order, per RFC-25 §Resolver and cache:
    ///
    /// 1. `<project_dir>/.specify/.cache/manifests/targets/<name>/adapter.yaml`
    ///    (agent-populated manifest cache).
    /// 2. `<project_dir>/adapters/targets/<name>/adapter.yaml` (in-repo).
    ///
    /// # Errors
    ///
    /// Returns `Error::Diag` with one of the following codes:
    /// - `adapter-not-found` — neither cache nor local directory exists.
    /// - `adapter-manifest-missing` — directory exists but no `adapter.yaml`.
    /// - `adapter-manifest-read-failed` — manifest exists but cannot be read.
    /// - `adapter-manifest-malformed` — manifest parses as something
    ///   other than the [`TargetAdapter`] shape.
    /// - `adapter-axis-mismatch` — manifest's `axis:` does not match
    ///   [`Axis::Target`].
    /// - `adapter-name-mismatch` — manifest's `name:` does not match
    ///   `name`.
    /// - `adapter-schema-violation` — manifest fails the target-axis
    ///   JSON Schema.
    pub fn resolve(name: &str, project_dir: &Path) -> Result<ResolvedTargetAdapter, Error> {
        let (root_dir, location, manifest_path, raw_value) =
            load_validated(Axis::Target, name, project_dir)?;

        let manifest: Self = serde_json::from_value(raw_value).map_err(|err| Error::Diag {
            code: "adapter-manifest-malformed",
            detail: format!("failed to deserialize {}: {err}", manifest_path.display()),
        })?;

        check_axis_and_name(Axis::Target, name, manifest.axis, &manifest.name, &manifest_path)?;

        Ok(ResolvedTargetAdapter {
            manifest,
            root_dir,
            location,
        })
    }

    /// Locate the target-adapter directory `(Target, name)` resolves
    /// to without reading the manifest. Mirrors
    /// [`TargetAdapter::resolve`]'s probe order (cache → local).
    ///
    /// # Errors
    ///
    /// Returns `adapter-not-found` when neither cache nor local
    /// directory exists.
    pub fn locate(name: &str, project_dir: &Path) -> Result<(PathBuf, AdapterLocation), Error> {
        locate_axis(Axis::Target, name, project_dir)
    }

    /// Resolve the manifest's brief path for `operation` against
    /// `root_dir`. Returns `None` when the operation is not declared
    /// by this manifest. Target manifests always declare all three
    /// operations after a successful schema-validated load, so this
    /// returns `Some` in practice.
    #[must_use]
    pub fn brief_path(&self, root_dir: &Path, operation: TargetOperation) -> Option<PathBuf> {
        self.briefs.get(&operation).map(|relative| root_dir.join(relative))
    }

    /// Iterator over the target operations this adapter declares, in
    /// ascending kebab-name order (`build < merge < shape`). After
    /// the collapse of the dedicated `operations[]` field (review
    /// 1.A1) and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &TargetOperation> {
        self.briefs.keys()
    }
}

impl ResolvedSourceAdapter {
    /// Convenience accessor combining [`SourceAdapter::brief_path`]
    /// and the resolved [`ResolvedSourceAdapter::root_dir`].
    #[must_use]
    pub fn brief_path(&self, operation: SourceOperation) -> Option<PathBuf> {
        self.manifest.brief_path(&self.root_dir, operation)
    }
}

impl ResolvedTargetAdapter {
    /// Convenience accessor combining [`TargetAdapter::brief_path`]
    /// and the resolved [`ResolvedTargetAdapter::root_dir`].
    #[must_use]
    pub fn brief_path(&self, operation: TargetOperation) -> Option<PathBuf> {
        self.manifest.brief_path(&self.root_dir, operation)
    }
}

/// Shared load + schema-validate pipeline used by both axis-specific
/// resolvers.
///
/// Returns the root directory, the [`AdapterLocation`] tag, the
/// canonical manifest path (for error messages), and the
/// schema-validated `serde_json::Value` ready for typed deserialisation
/// by the caller. Keeps the duplicated bytes between
/// [`SourceAdapter::resolve`] and [`TargetAdapter::resolve`] down to a
/// single `serde_json::from_value` + axis/name check.
fn load_validated(
    axis: Axis, name: &str, project_dir: &Path,
) -> Result<(PathBuf, AdapterLocation, PathBuf, serde_json::Value), Error> {
    let (root_dir, location) = locate_axis(axis, name, project_dir)?;
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
    let raw_value: serde_json::Value = serde_saphyr::from_str(&raw).map_err(|err| Error::Diag {
        code: "adapter-manifest-malformed",
        detail: format!("failed to parse {}: {err}", manifest_path.display()),
    })?;
    validate_schema(axis, &manifest_path, &raw_value)?;

    Ok((root_dir, location, manifest_path, raw_value))
}

fn locate_axis(
    axis: Axis, name: &str, project_dir: &Path,
) -> Result<(PathBuf, AdapterLocation), Error> {
    let cached = cache_dir(project_dir, axis, name);
    // The manifest cache owns its own root
    // (`.specify/.cache/manifests/{sources,targets}/<name>/`), disjoint
    // from the RFC-27 §D8 extraction cache under
    // `.specify/.cache/extractions/<adapter>/`. A bare cache directory
    // is therefore always a manifest mirror — see
    // [DECISIONS.md §"Cache layout"].
    let location = if cached.is_dir() {
        AdapterLocation::Cached(cached)
    } else {
        let local = adapter_axis_dir(project_dir, axis).join(name);
        if local.is_dir() {
            AdapterLocation::Local(local)
        } else {
            return Err(Error::Diag {
                code: "adapter-not-found",
                detail: format!(
                    "adapter `{name}` (axis `{axis}`) not found at {} or {}",
                    cache_dir(project_dir, axis, name).display(),
                    adapter_axis_dir(project_dir, axis).join(name).display(),
                ),
            });
        }
    };
    // Cross-axis uniqueness invariant — see DECISIONS.md
    // §"Adapter name uniqueness". A name that also resolves on the
    // opposite axis is rejected here so every loader entry point
    // (resolve / locate / public callers) hits the same gate.
    if let Some(sibling) = sibling_manifest_path(axis.opposite(), name, project_dir) {
        return Err(axis_collision_error(name, axis, location.path(), &sibling));
    }
    let path = location.path().clone();
    Ok((path, location))
}

/// Probe the (axis, name) pair for an `adapter.yaml` under both the
/// manifest cache and the in-repo tree. Returns the first hit — the
/// cache wins over `local`, mirroring [`locate_axis`]'s probe order —
/// or `None` when neither location declares a manifest. A bare
/// directory without `adapter.yaml` is treated as absent so the
/// cross-axis collision check fires only on declared manifests, not
/// stale empty cache slots.
fn sibling_manifest_path(axis: Axis, name: &str, project_dir: &Path) -> Option<PathBuf> {
    let cached = cache_dir(project_dir, axis, name);
    if cached.join(ADAPTER_FILENAME).is_file() {
        return Some(cached);
    }
    let local = adapter_axis_dir(project_dir, axis).join(name);
    if local.join(ADAPTER_FILENAME).is_file() {
        return Some(local);
    }
    None
}

fn axis_collision_error(
    name: &str, located_axis: Axis, located_path: &Path, sibling_path: &Path,
) -> Error {
    let opposite = located_axis.opposite();
    Error::validation_failed(
        "adapter-name-axis-collision",
        format!("adapter name `{name}` is unique across axes"),
        format!(
            "adapter name `{name}` is declared under both `adapters/sources/` and `adapters/targets/` (or the equivalent `.specify/.cache/manifests/{{sources,targets}}/<name>/` mirror); names must be unique across axes (axis `{located_axis}`: {}; axis `{opposite}`: {})",
            located_path.display(),
            sibling_path.display(),
        ),
    )
}

/// Validate that installing or resolving `name` on `axis` does not
/// collide with an existing declaration on the opposite axis.
///
/// Used at `specify init` time (with `axis = Axis::Target`, since
/// `init` only caches target adapters) before the per-axis manifest
/// cache directory at
/// `.specify/.cache/manifests/{sources,targets}/<name>/` is rewritten,
/// so the operator hits a clear collision diagnostic ahead of the
/// downstream `TargetAdapter::resolve` call. The same invariant fires
/// inside the private `locate_axis` helper used by
/// [`SourceAdapter::resolve`] / [`TargetAdapter::resolve`] for every
/// loader path; this one-sided helper is the cheap "the side I'm about
/// to install on may not yet exist on disk" variant.
///
/// # Errors
///
/// Returns [`Error::Validation`] with the kebab discriminant
/// `adapter-name-axis-collision` when the opposite axis already
/// declares a manifest for `name`. The error body names both axes.
pub fn check_axis_unique_for_name(axis: Axis, name: &str, project_dir: &Path) -> Result<(), Error> {
    let opposite = axis.opposite();
    let Some(sibling) = sibling_manifest_path(opposite, name, project_dir) else {
        return Ok(());
    };
    // The error body must name both axes; pass through the
    // axis-being-installed as the "located" axis even when no
    // manifest exists on disk for it yet — the diagnostic prose is
    // about the *name* clash, not which side resolved first.
    let here = adapter_axis_dir(project_dir, axis).join(name);
    Err(axis_collision_error(name, axis, &here, &sibling))
}

fn check_axis_and_name(
    expected_axis: Axis, expected_name: &str, manifest_axis: Axis, manifest_name: &str,
    manifest_path: &Path,
) -> Result<(), Error> {
    if manifest_axis != expected_axis {
        return Err(Error::Diag {
            code: "adapter-axis-mismatch",
            detail: format!(
                "{} declares axis `{manifest_axis}`, but resolver was asked for axis `{expected_axis}`",
                manifest_path.display(),
            ),
        });
    }
    if manifest_name != expected_name {
        return Err(Error::Diag {
            code: "adapter-name-mismatch",
            detail: format!(
                "{} declares name `{manifest_name}` but lives under `{expected_name}/`",
                manifest_path.display(),
            ),
        });
    }
    Ok(())
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
            project.join(".specify/.cache/manifests/sources/documentation")
        );
        assert_eq!(
            cache_dir(project, Axis::Target, "omnia"),
            project.join(".specify/.cache/manifests/targets/omnia")
        );
    }

    #[test]
    fn source_cache_field_defaults_to_none() {
        let yaml = r"name: documentation
version: 1
axis: source
briefs:
  enumerate: briefs/enumerate.md
  extract: briefs/extract.md
";
        let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(manifest.cache, None, "missing cache field must default to None");
        let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
        assert!(
            !rendered.contains("cache:"),
            "absent cache field must elide on write, got:\n{rendered}"
        );
    }

    #[test]
    fn source_cache_opt_out_round_trips() {
        let yaml = r"name: documentation
version: 1
axis: source
briefs:
  enumerate: briefs/enumerate.md
  extract: briefs/extract.md
cache: opt-out
";
        let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(manifest.cache, Some(CacheMode::OptOut));
        assert_eq!(
            manifest.operations().copied().collect::<Vec<_>>(),
            vec![SourceOperation::Enumerate, SourceOperation::Extract]
        );
        let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
        assert!(
            rendered.contains("cache: opt-out"),
            "cache: opt-out must round-trip as kebab-case, got:\n{rendered}"
        );
        let reparsed: SourceAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(manifest, reparsed);
    }

    #[test]
    fn target_briefs_typed_at_parse_boundary() {
        let yaml = r"name: omnia
version: 1
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
";
        let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(
            manifest.operations().copied().collect::<Vec<_>>(),
            // BTreeMap key order: build < merge < shape (kebab-case).
            vec![TargetOperation::Build, TargetOperation::Merge, TargetOperation::Shape]
        );
    }

    #[test]
    fn unknown_brief_key_rejected_at_serde_parse() {
        // `shape` is a target operation; appearing on a source manifest
        // must fail at the typed `briefs: BTreeMap<SourceOperation, _>`
        // deserialisation boundary before any downstream code runs.
        let yaml = r"name: bogus
version: 1
axis: source
briefs:
  enumerate: briefs/enumerate.md
  shape: briefs/shape.md
";
        let err = serde_saphyr::from_str::<SourceAdapter>(yaml)
            .expect_err("unknown source operation must be rejected");
        let detail = err.to_string();
        assert!(
            detail.contains("shape") || detail.contains("enumerate"),
            "expected closed-enum diagnostic, got: {detail}"
        );
    }
}
