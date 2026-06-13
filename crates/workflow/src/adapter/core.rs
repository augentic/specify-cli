//! Axis-split adapter manifest model and post-load coherence gates.
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
//! Resolution lives in [`super::resolve`]; schema validation and the
//! cross-axis collision probe in [`super::validate_manifest`]. This
//! module owns the manifest types, the path helpers, and the two
//! post-load coherence gates ([`check_axis_and_name`],
//! [`check_execution`]).
//!
//! See [DECISIONS.md §"Operations typed at parse boundary"] for the
//! rationale.
//!
//! [DECISIONS.md §"Operations typed at parse boundary"]: ../../../../DECISIONS.md#operations-typed-at-parse-boundary

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::Platform;
use crate::adapter::operation::{SourceOperation, TargetOperation};

/// Filename of an adapter manifest.
///
/// Source and target adapters share the `adapter.yaml` filename per
/// workflow §Adapter implementation shape; the directory's axis (under
/// `adapters/sources/` or `adapters/targets/`) and the manifest's
/// `axis:` field disambiguate.
pub const ADAPTER_FILENAME: &str = "adapter.yaml";

/// Parent directory for in-repo adapter trees.
pub const ADAPTERS_DIR: &str = "adapters";

/// Manifest-cache root segment under `.specify/cache/`.
///
/// `.specify/cache/manifests/{sources,targets}/<name>/` mirrors the
/// in-repo `adapters/{sources,targets}/<name>/` tree (see
/// [DECISIONS.md §"Cache layout"]).
///
/// [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout
pub const MANIFESTS_CACHE_DIR: &str = "manifests";

/// Axis discriminator for an adapter manifest.
///
/// Source vs target — see workflow §Adapter vocabulary. The closed enum is
/// used by the resolver dispatcher (`commands::resolve_adapter`) and
/// the manifest-cache helpers ([`cache_dir`], `adapter_axis_dir`);
/// the in-memory manifests themselves are axis-typed
/// ([`SourceAdapter`] / [`TargetAdapter`]) so internal call sites no
/// longer carry the `axis` argument forward past the resolver
/// boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
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
/// Decoupled from [`specify_tool_manifest::Tool`] so adapter loading
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

/// One adapter-declared build input inside a target manifest.
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

/// Declarative platforms capability for a target adapter manifest.
///
/// When a target declares `platforms` in its `adapter.yaml`, the CLI
/// uses this to enforce platform requirements at `specify init` time
/// and to scaffold defaults for greenfield workspace members.
///
/// - `required` — if true, `specify init` demands `--platforms`.
/// - `allowed` — the closed set of [`Platform`] tokens the target
///   accepts; any project token outside the set is rejected.
/// - `default` — the platform set scaffolded when the operator does
///   not specify (used by greenfield workspace sync).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlatformsCapability {
    /// Whether projects using this target must declare platforms.
    pub required: bool,
    /// Platforms this target accepts.
    pub allowed: Vec<Platform>,
    /// Default platform set for greenfield scaffolding.
    pub default: Vec<Platform>,
}

/// Typed outcome of [`PlatformsCapability::check`].
///
/// Each caller maps the violation onto its own diagnostic-code family
/// (`project-platforms-*` at init, `topology-cache-project-platforms-*`
/// at topology resolution) so the rules themselves live in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformsViolation {
    /// The capability demands a platform set but none was declared.
    /// Carries the capability's display-formatted `default` set for the
    /// caller's hint text.
    RequiredButMissing {
        /// Display-formatted `default` platform tokens.
        defaults: Vec<String>,
    },
    /// A non-empty platform set omits the mandatory `core` member.
    MissingCore,
    /// A declared platform is outside the capability's `allowed` set.
    /// Carries the display-formatted allowed set for the hint text.
    NotAllowed {
        /// The offending platform.
        platform: Platform,
        /// Display-formatted `allowed` platform tokens.
        allowed: Vec<String>,
    },
}

impl PlatformsCapability {
    /// Validate a declared platform set against this capability: a
    /// required capability refuses an empty set; a non-empty set must
    /// include [`Platform::Core`] and stay inside `allowed`. An empty
    /// set on a non-required capability passes (platforms are opt-in).
    ///
    /// # Errors
    ///
    /// Returns the first [`PlatformsViolation`] in rule order.
    pub fn check(&self, platforms: &[Platform]) -> Result<(), PlatformsViolation> {
        if platforms.is_empty() {
            if self.required {
                return Err(PlatformsViolation::RequiredButMissing {
                    defaults: self.default.iter().map(ToString::to_string).collect(),
                });
            }
            return Ok(());
        }
        if !platforms.contains(&Platform::Core) {
            return Err(PlatformsViolation::MissingCore);
        }
        for p in platforms {
            if !self.allowed.contains(p) {
                return Err(PlatformsViolation::NotAllowed {
                    platform: *p,
                    allowed: self.allowed.iter().map(ToString::to_string).collect(),
                });
            }
        }
        Ok(())
    }
}

/// Closed adapter execution mode.
///
/// Declared by the required `execution:` field on `adapter.yaml`.
/// Source adapters are agent-only (`source.schema.json` enumerates
/// `["agent"]`); target adapters may still declare `tool`, though the
/// target-side `build` / `merge` dispatch carries `agent` as a
/// placeholder. See DECISIONS.md §"Adapter execution mode".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Execution {
    /// `execution: agent` — the adapter's brief is executed by an agent
    /// against the sandbox preopens. The CLI orchestrates inputs and
    /// validates outputs against the schemas; agent outputs are
    /// non-deterministic, so nothing is memoized.
    Agent,
    /// `execution: tool` — target-axis only: `build` / `merge` are
    /// dispatched through a declared WASI tool or a built-in
    /// deterministic Rust path. Source adapters are agent-only.
    Tool,
}

/// Where an adapter manifest was located on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterLocation {
    /// Resolved from `<project_dir>/adapters/{sources,targets}/<name>/`.
    Local(PathBuf),
    /// Resolved from the manifest cache at
    /// `<project_dir>/.specify/cache/manifests/{sources,targets}/<name>/`.
    /// The manifest cache mirrors the in-repo adapter tree
    /// (`adapter.yaml` plus brief markdown) — see
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

/// Manifest cache root for an axis —
/// `.specify/cache/manifests/{sources,targets}/`.
///
/// Path-only helper — the directory may or may not exist on disk.
#[must_use]
pub fn cache_axis_dir(project_dir: &Path, axis: Axis) -> PathBuf {
    project_dir.join(".specify").join("cache").join(MANIFESTS_CACHE_DIR).join(axis.dir_segment())
}

/// Manifest cache root for `(axis, name)` —
/// `.specify/cache/manifests/{sources,targets}/<name>/`.
///
/// This is the agent-populated mirror of `adapters/{sources,targets}/<name>/`
/// — `adapter.yaml` plus the brief markdown files it references. See
/// [DECISIONS.md §"Cache layout"].
///
/// Path-only helper — the directory may or may not exist on disk.
///
/// [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
#[must_use]
pub fn cache_dir(project_dir: &Path, axis: Axis, name: &str) -> PathBuf {
    cache_axis_dir(project_dir, axis).join(name)
}

/// Per-operation agent scratch lane for `(adapter, segment)` —
/// `.specify/scratch/<adapter>/<segment>/`.
///
/// `<segment>` is the literal `survey` for the slice-less survey op or
/// the slice name for extract.
/// The write-only `$SCRATCH_DIR` preopen of the source-operation
/// sandbox. Rooted under the transient working-state tree
/// (`.specify/scratch/`), structurally disjoint from the memoization
/// tree at `.specify/cache/`, so a scratch write can never pollute a
/// cache artifact; see [DECISIONS.md §"Cache layout"].
///
/// Path-only helper — the directory may or may not exist on disk.
///
/// [DECISIONS.md §"Cache layout"]: ../../../../DECISIONS.md#cache-layout
#[must_use]
pub fn scratch_dir(project_dir: &Path, adapter: &str, segment: &str) -> PathBuf {
    crate::config::Layout::new(project_dir).scratch_dir().join(adapter).join(segment)
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
    /// Closed adapter execution mode. Required by
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
    /// Closed adapter execution mode. Required by
    /// `target.schema.json`; modelled as `Option` (mirroring
    /// `description`) so the typed `check_execution` gate rejects a
    /// manifest that omits it with `adapter-execution-mode-required`
    /// rather than defaulting silently. First-party target manifests
    /// carry `agent` as a placeholder.
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
    /// Optional adapter-declared build inputs. Each entry is
    /// a path relative to the build request's `inputs.root`, flagged
    /// `required`; the CLI assembles `inputs.artifacts.additional[]`
    /// from this list. Defaults to an empty list when omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<BuildInputDeclaration>,
    /// Optional human-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional platforms capability. When present the target declares
    /// the closed set of [`Platform`] tokens it accepts, whether
    /// projects must declare platforms, and the default set for
    /// greenfield scaffolding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<PlatformsCapability>,
}

/// A parsed [`SourceAdapter`] paired with the [`AdapterLocation`] it
/// loaded from (in-repo vs. agent-populated cache). The filesystem
/// directory is reachable through [`AdapterLocation::path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSourceAdapter {
    /// Parsed manifest.
    pub manifest: SourceAdapter,
    /// Whether the manifest came from
    /// `.specify/cache/manifests/sources/<name>/` or from
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
    /// `.specify/cache/manifests/targets/<name>/` or from
    /// `<project_dir>/adapters/targets/<name>/`, and the directory
    /// itself via [`AdapterLocation::path`].
    pub location: AdapterLocation,
}

impl SourceAdapter {
    /// Iterator over the source operations this adapter declares, in
    /// ascending kebab-name order (`extract < survey`). After the
    /// collapse of the dedicated `operations[]` field (review 1.A1)
    /// and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &SourceOperation> {
        self.briefs.keys()
    }
}

impl TargetAdapter {
    /// Iterator over the target operations this adapter declares, in
    /// ascending kebab-name order (`build < merge < shape`). After
    /// the collapse of the dedicated `operations[]` field (review
    /// 1.A1) and the operation-type refactor (review 1.B1),
    /// `briefs.keys()` is the canonical typed operation source.
    pub fn operations(&self) -> impl Iterator<Item = &TargetOperation> {
        self.briefs.keys()
    }
}

/// Post-load axis/name coherence gate, run by [`super::resolve`] after
/// schema validation against the typed manifest fields.
///
/// Returns `Error::Diag` with `adapter-axis-mismatch` when the
/// manifest's `axis:` disagrees with the resolver's axis, and
/// `adapter-name-mismatch` when `name:` disagrees with the directory
/// the manifest lives under.
pub(super) fn check_axis_and_name(
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

/// Typed `execution`-mode gate, run after schema validation
/// and the axis/name coherence check.
///
/// Single-signal abort, `Error::Validation` (exit 2) with the kebab
/// `error` discriminant from the wire contract:
/// `adapter-execution-mode-required` — the manifest omits `execution`.
/// The per-axis JSON Schemas also mark `execution` `required`, so this
/// typed gate is the belt-and-suspenders that refuses to default
/// silently when a manifest reaches the loader through a path that
/// bypassed schema validation.
pub(super) fn check_execution(
    execution: Option<Execution>, manifest_path: &Path,
) -> Result<(), Error> {
    if execution.is_none() {
        return Err(Error::validation_failed(
            "adapter-execution-mode-required",
            "adapter manifest declares a closed `execution` mode",
            format!("{} omits the required `execution` field", manifest_path.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
