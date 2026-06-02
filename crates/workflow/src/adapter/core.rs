//! Axis-split adapter manifests and the per-axis loader entry points.
//!
//! Source adapters and target adapters share a manifest shape on the
//! wire (`adapter.yaml`) but carry disjoint closed operation sets:
//! [`SourceOperation`] (`extract | survey`) vs. [`TargetOperation`]
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
use specify_error::Error;
use specify_schema::{
    ADAPTER_JSON_SCHEMA, SOURCE_JSON_SCHEMA, TARGET_JSON_SCHEMA, ValidationStatus,
};

use crate::adapter::operation::{SourceOperation, TargetOperation};
use crate::schema::validate_value;

/// Filename of an adapter manifest.
///
/// Source and target adapters share the `adapter.yaml` filename per
/// workflow §Adapter implementation shape; the directory's axis (under
/// `adapters/sources/` or `adapters/targets/`) and the manifest's
/// `axis:` field disambiguate.
pub const ADAPTER_FILENAME: &str = "adapter.yaml";

/// Parent directory for in-repo adapter trees.
pub const ADAPTERS_DIR: &str = "adapters";

/// Manifest-cache root segment under `.specify/.cache/`.
///
/// `.specify/.cache/manifests/{sources,targets}/<name>/` mirrors the
/// in-repo `adapters/{sources,targets}/<name>/` tree. Paired with
/// [`EXTRACTIONS_CACHE_DIR`] so the manifest and extraction cache fingerprint contract extraction
/// caches own disjoint roots (see [DECISIONS.md §"Cache layout"]).
///
/// [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout
pub const MANIFESTS_CACHE_DIR: &str = "manifests";

/// Extraction-cache root segment under `.specify/.cache/`.
///
/// `.specify/.cache/extractions/<adapter>/<fingerprint>/` holds the
/// extraction cache fingerprint contract per-source extraction result cache (with `index.jsonl`
/// at the adapter root). Per-adapter only — extraction is a source-axis
/// operation — and partitioned from [`MANIFESTS_CACHE_DIR`] so each
/// cache owns its own tree (no co-tenancy heuristic; see
/// [DECISIONS.md §"Cache layout"]).
///
/// [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout
pub const EXTRACTIONS_CACHE_DIR: &str = "extractions";

/// Axis discriminator for an adapter manifest.
///
/// Source vs target — see workflow §Adapter axis. The closed enum is
/// used by the resolver dispatcher (`commands::resolve_adapter`) and
/// the manifest-cache helpers ([`cache_dir`], `adapter_axis_dir`);
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
    /// Source adapter — `extract` + `survey`.
    Source,
    /// Target adapter — `shape` + `build` + `merge`.
    Target,
}

impl Axis {
    /// Axis segment under `ADAPTERS_DIR` — `"sources"` for source
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

/// One adapter-declared build input inside a target manifest (RFC-29d).
///
/// Each entry names a path the target's `build` operation consumes,
/// relative to the build request's `inputs.root` (the slice tree). The
/// CLI assembles the request's `inputs.artifacts.additional[]` from
/// this list and (in a later change) raises `target-build-input-missing`
/// when a `required` path is absent. v1 keeps the declaration a flat
/// path list — globs and conditional inputs are deferred.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BuildInputDeclaration {
    /// Path relative to the build request's `inputs.root`.
    pub path: String,
    /// Whether `build` requires this input; a missing `required` path
    /// is a build-time abort once the matching check lands.
    pub required: bool,
}

/// Closed enum for the optional `cache:` field on an adapter manifest
/// (extraction cache fingerprint contract). Single variant in v1; widened only behind an accepted design change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CacheMode {
    /// `cache: opt-out` — the CLI bypasses the cache for every run
    /// of this adapter; the matching `slice.extract.cache-miss`
    /// journal event carries `reason: adapter-opt-out`.
    OptOut,
}

/// Closed adapter execution mode (RFC-29 D9).
///
/// Declared by the required `execution:` field on `adapter.yaml`.
/// Source and target adapters share the enum; RFC-29 M1 (D9) landed the
/// source-side dispatch — the target-side `build` / `merge` dispatch
/// follows in M3, so target manifests carry `agent` as a placeholder
/// until then. See DECISIONS.md §"Adapter execution mode (D9)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Execution {
    /// `execution: agent` — the adapter's brief is executed by an agent
    /// against the sandbox preopens. The CLI orchestrates inputs and
    /// validates outputs against the schemas, but does not cache the
    /// result: `agent` forces `cache: opt-out` (see
    /// [`SourceAdapter::effective_cache_mode`]).
    Agent,
    /// `execution: tool` — `survey` / `extract` (sources) or `build` /
    /// `merge` (targets) are dispatched through a declared WASI tool or
    /// a built-in deterministic Rust path.
    Tool,
}

/// Where an adapter manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterLocation {
    /// Resolved from `<project_dir>/adapters/{sources,targets}/<name>/`.
    Local(PathBuf),
    /// Resolved from the manifest cache at
    /// `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`.
    /// The manifest cache mirrors the in-repo adapter tree
    /// (`adapter.yaml` plus brief markdown); the extraction cache fingerprint contract extraction
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
/// extraction cache fingerprint contract per-source extraction result cache lives in a separate
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
    /// Closed adapter execution mode (RFC-29 D9). Required by
    /// `source.schema.json`; modelled as `Option` (mirroring
    /// `description`) so the typed `check_execution` gate rejects a
    /// manifest that omits it with `adapter-execution-mode-required`
    /// rather than defaulting silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    /// Typed source-operation → relative brief path map. Closed by
    /// `source.schema.json#/properties/briefs`: every source manifest
    /// declares `extract` + `survey`. `briefs.keys()` is the
    /// canonical operation iterator — exposed via
    /// [`SourceAdapter::operations`].
    pub briefs: BTreeMap<SourceOperation, String>,
    /// Optional declared WASI tools for declared WASI tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdapterToolDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional cache opt-out switch (extraction cache fingerprint contract).
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
    /// Closed adapter execution mode (RFC-29 D9). Required by
    /// `target.schema.json`; modelled as `Option` (mirroring
    /// `description`) so the typed `check_execution` gate rejects a
    /// manifest that omits it with `adapter-execution-mode-required`
    /// rather than defaulting silently. Target dispatch lands in M3;
    /// first-party target manifests carry `agent` as a placeholder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Execution>,
    /// Typed target-operation → relative brief path map. Closed by
    /// `target.schema.json#/properties/briefs`: every target manifest
    /// declares `shape` + `build` + `merge`. `briefs.keys()` is the
    /// canonical operation iterator — exposed via
    /// [`TargetAdapter::operations`].
    pub briefs: BTreeMap<TargetOperation, String>,
    /// Optional declared WASI tools for declared WASI tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AdapterToolDeclaration>,
    /// Optional adapter-declared build inputs (RFC-29d). Each entry is
    /// a path relative to the build request's `inputs.root`, flagged
    /// `required`; the CLI assembles `inputs.artifacts.additional[]`
    /// from this list. Defaults to an empty list when omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<BuildInputDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional cache opt-out switch (extraction cache fingerprint contract).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheMode>,
}

/// A parsed [`SourceAdapter`] paired with the [`AdapterLocation`] it
/// loaded from (in-repo vs. agent-populated cache). The filesystem
/// directory is reachable through [`AdapterLocation::path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSourceAdapter {
    /// Parsed manifest.
    pub manifest: SourceAdapter,
    /// Whether the manifest came from
    /// `.specify/.cache/manifests/sources/<name>/` or from
    /// `<project_dir>/adapters/sources/<name>/`, and the directory
    /// itself via [`AdapterLocation::path`].
    pub location: AdapterLocation,
}

/// A parsed [`TargetAdapter`] paired with the [`AdapterLocation`] it
/// loaded from (in-repo vs. agent-populated cache). The filesystem
/// directory is reachable through [`AdapterLocation::path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTargetAdapter {
    /// Parsed manifest.
    pub manifest: TargetAdapter,
    /// Whether the manifest came from
    /// `.specify/.cache/manifests/targets/<name>/` or from
    /// `<project_dir>/adapters/targets/<name>/`, and the directory
    /// itself via [`AdapterLocation::path`].
    pub location: AdapterLocation,
}

impl SourceAdapter {
    /// Resolve a source adapter by kebab-case name.
    ///
    /// Probe order, per workflow §Resolver and cache:
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
        let (manifest, location, manifest_path) =
            resolve_typed::<Self>(Axis::Source, name, project_dir)?;
        check_axis_and_name(Axis::Source, name, manifest.axis, &manifest.name, &manifest_path)?;
        check_execution(manifest.execution, manifest.cache, &manifest_path)?;
        Ok(ResolvedSourceAdapter { manifest, location })
    }

    /// Iterator over the source operations this adapter declares, in
    /// ascending kebab-name order (`extract < survey`). After the
    /// collapse of the dedicated `operations[]` field (review 1.A1)
    /// and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &SourceOperation> {
        self.briefs.keys()
    }

    /// Effective extraction-cache mode after applying the
    /// `execution: agent` forced opt-out (RFC-29 D9). When
    /// `execution: agent` the cache is always bypassed regardless of
    /// the declared `cache:` field; otherwise the declared mode (or its
    /// absence) applies. The source-operation runner (RFC-29 D1)
    /// consumes this rather than the raw [`Self::cache`] field.
    #[must_use]
    pub const fn effective_cache_mode(&self) -> Option<CacheMode> {
        effective_cache_mode(self.execution, self.cache)
    }
}

/// Shared `execution: agent` forced-opt-out rule (RFC-29 D9) behind
/// both [`SourceAdapter::effective_cache_mode`] and
/// [`TargetAdapter::effective_cache_mode`] (REVIEW.md A9): an
/// `agent`-dispatched adapter always bypasses the cache regardless of
/// the declared `cache:` field; otherwise the declared mode applies.
const fn effective_cache_mode(
    execution: Option<Execution>, cache: Option<CacheMode>,
) -> Option<CacheMode> {
    if matches!(execution, Some(Execution::Agent)) { Some(CacheMode::OptOut) } else { cache }
}

impl TargetAdapter {
    /// Resolve a target adapter by kebab-case name.
    ///
    /// Probe order, per workflow §Resolver and cache:
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
        let (manifest, location, manifest_path) =
            resolve_typed::<Self>(Axis::Target, name, project_dir)?;
        check_axis_and_name(Axis::Target, name, manifest.axis, &manifest.name, &manifest_path)?;
        check_execution(manifest.execution, manifest.cache, &manifest_path)?;
        Ok(ResolvedTargetAdapter { manifest, location })
    }

    /// Iterator over the target operations this adapter declares, in
    /// ascending kebab-name order (`build < merge < shape`). After
    /// the collapse of the dedicated `operations[]` field (review
    /// 1.A1) and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &TargetOperation> {
        self.briefs.keys()
    }

    /// Effective extraction-cache mode after applying the
    /// `execution: agent` forced opt-out (RFC-29 D9). Mirrors
    /// [`SourceAdapter::effective_cache_mode`]; target dispatch (M3)
    /// will consume it once `build` / `merge` become CLI-owned.
    #[must_use]
    pub const fn effective_cache_mode(&self) -> Option<CacheMode> {
        effective_cache_mode(self.execution, self.cache)
    }
}

/// Locate, schema-validate, and typed-deserialise an adapter manifest
/// of the axis-specific shape `M`.
///
/// Wraps [`load_validated`] with the `serde_json::from_value` step that
/// was duplicated byte-for-byte between [`SourceAdapter::resolve`] and
/// [`TargetAdapter::resolve`]. Returns the parsed manifest, its
/// [`AdapterLocation`], and the canonical manifest path; callers run the
/// axis/name coherence check ([`check_axis_and_name`]) against the
/// typed manifest fields and wrap the result into the matching
/// `Resolved*Adapter`.
fn resolve_typed<M: serde::de::DeserializeOwned>(
    axis: Axis, name: &str, project_dir: &Path,
) -> Result<(M, AdapterLocation, PathBuf), Error> {
    let (location, manifest_path, raw_value) = load_validated(axis, name, project_dir)?;
    let manifest: M = serde_json::from_value(raw_value).map_err(|err| Error::Diag {
        code: "adapter-manifest-malformed",
        detail: format!("failed to deserialize {}: {err}", manifest_path.display()),
    })?;
    Ok((manifest, location, manifest_path))
}

/// Shared load + schema-validate pipeline used by both axis-specific
/// resolvers.
///
/// Returns the [`AdapterLocation`] tag (whose [`AdapterLocation::path`]
/// is the root directory), the canonical manifest path (for error
/// messages), and the schema-validated `serde_json::Value` ready for
/// typed deserialisation by [`resolve_typed`].
fn load_validated(
    axis: Axis, name: &str, project_dir: &Path,
) -> Result<(AdapterLocation, PathBuf, serde_json::Value), Error> {
    let location = locate_axis(axis, name, project_dir)?;
    let root_dir = location.path();
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

    Ok((location, manifest_path, raw_value))
}

fn locate_axis(axis: Axis, name: &str, project_dir: &Path) -> Result<AdapterLocation, Error> {
    let cached = cache_dir(project_dir, axis, name);
    let local = adapter_axis_dir(project_dir, axis).join(name);
    // The manifest cache owns its own root
    // (`.specify/.cache/manifests/{sources,targets}/<name>/`), disjoint
    // from the extraction cache under
    // `.specify/.cache/extractions/<adapter>/`. A bare cache directory
    // is therefore always a manifest mirror — see
    // [DECISIONS.md §"Cache layout"].
    let location = if cached.is_dir() {
        AdapterLocation::Cached(cached)
    } else if local.is_dir() {
        AdapterLocation::Local(local)
    } else {
        return Err(Error::Diag {
            code: "adapter-not-found",
            detail: format!(
                "adapter `{name}` (axis `{axis}`) not found at {} or {}",
                cached.display(),
                local.display(),
            ),
        });
    };
    // Cross-axis uniqueness invariant — see DECISIONS.md
    // §"Adapter name uniqueness". `specrun` is fork-and-exit, so the
    // pair of `is_file` probes below is cheaper than memoising them
    // behind process-global state; `init` / `init --hub` and the
    // manifest-cache write boundary call [`check_axis_unique_for_name`]
    // eagerly on the same invariant.
    if let Some(sibling) = sibling_manifest_path(axis.opposite(), name, project_dir) {
        return Err(axis_collision_error(name, axis, location.path(), &sibling));
    }
    Ok(location)
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
/// Used at `specrun init` time (with `axis = Axis::Target`, since
/// `init` only caches target adapters) before the per-axis manifest
/// cache directory at
/// `.specify/.cache/manifests/{sources,targets}/<name>/` is rewritten,
/// so the operator hits a clear collision diagnostic ahead of the
/// downstream `TargetAdapter::resolve` call. The same invariant fires
/// inside the private `locate_axis` helper used by
/// [`SourceAdapter::resolve`] / [`TargetAdapter::resolve`]; this
/// one-sided helper is the cheap "the side I'm about to install on may
/// not yet exist on disk" variant.
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

/// Typed `execution`-mode gate (RFC-29 D9), run after schema validation
/// and the axis/name coherence check.
///
/// Two single-signal aborts, both `Error::Validation` (exit 2) with the
/// kebab `error` discriminants from the RFC-29 wire contract:
///
/// - `adapter-execution-mode-required` — the manifest omits `execution`.
///   The per-axis JSON Schemas also mark `execution` `required`, so this
///   typed gate is the belt-and-suspenders that refuses to default
///   silently when a manifest reaches the loader through a path that
///   bypassed schema validation.
/// - `adapter-execution-agent-cache-conflict` — `execution: agent` is
///   declared together with a `cache:` mode other than the forced
///   opt-out. This arm is a forward guard: [`CacheMode`] is
///   single-variant (`OptOut`) today and `source.schema.json` /
///   `target.schema.json` enumerate only `["opt-out"]`, so no legal
///   manifest can declare a non-opt-out cache mode — the arm cannot
///   fire until the cache enum widens. The runtime "agent forces
///   opt-out" behaviour itself is modelled by
///   [`SourceAdapter::effective_cache_mode`] /
///   [`TargetAdapter::effective_cache_mode`], not here.
fn check_execution(
    execution: Option<Execution>, cache: Option<CacheMode>, manifest_path: &Path,
) -> Result<(), Error> {
    let Some(execution) = execution else {
        return Err(Error::validation_failed(
            "adapter-execution-mode-required",
            "adapter manifest declares a closed `execution` mode",
            format!(
                "{} omits the required `execution` field (`agent` or `tool`)",
                manifest_path.display(),
            ),
        ));
    };
    if execution == Execution::Agent
        && let Some(mode) = cache
        && mode != CacheMode::OptOut
    {
        return Err(Error::validation_failed(
            "adapter-execution-agent-cache-conflict",
            "`execution: agent` forces `cache: opt-out`",
            format!(
                "{} declares `execution: agent` with `cache: {mode}`; agent execution forces `cache: opt-out`",
                manifest_path.display(),
            ),
        ));
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
mod tests;
