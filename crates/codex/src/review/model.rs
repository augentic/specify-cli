//! `WorkspaceModel` DTOs per RFC-32 §"`WorkspaceModel`" and
//! §"Core entity families (v1)".
//!
//! The model is the deterministic, versioned snapshot of project
//! facts the RFC-32 indexer produces once per `specrun review`
//! invocation. Per RFC-32 §"Persistence and query (v1 decision)" it
//! is an internal execution artifact: **not** persisted under
//! `.specify/` and **not** an operator-facing Specify artifact in
//! v1. `.specify/cache/workspace-model.v1.json` and
//! `specrun model query <selector>` are reserved surfaces with no
//! implementation behind them.
//!
//! The DTOs round-trip through `specify_schema::WORKSPACE_MODEL_JSON_SCHEMA`
//! per RFC-32 §"Schema location". The envelope's `version: 1`
//! discriminant pins the wire shape; per RFC-32 §"`WorkspaceModel`"
//! breaking indexer output bumps the version.
//!
//! Wire shape notes:
//!
//! - Top-level envelope keys are `snake_case` (`project_dir`,
//!   `scan_profile`, `markdown_sections`, …) to match the JSON
//!   Schema under `specify-cli/schemas/review/workspace-model.schema.json`.
//! - Nested entity-fact keys are kebab-case (`line-start`,
//!   `from-path`, `frontmatter-ref`, …) per the same schema; each
//!   entity struct carries `#[serde(rename_all = "kebab-case")]`.
//! - Every array on the envelope is always serialised, even when
//!   empty, so JSON consumers can rely on the full set of fact
//!   families existing.

#![allow(
    clippy::module_name_repetitions,
    reason = "RFC-32 mandates the `WorkspaceModel` envelope name; the surrounding `model` module is the navigational home for the DTO layer per the §\"Library layout\" sketch."
)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::rules::Origin;

/// Type-level pin of the `WorkspaceModel` envelope version.
///
/// Serialises to the integer `1` and refuses to deserialise any
/// other value per RFC-32 §"`WorkspaceModel`". Modelled as a unit
/// struct so the [`Default`] / [`Eq`] / [`Hash`] / [`Ord`]
/// derivations propagate to [`WorkspaceModel`] without further
/// plumbing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceModelVersion;

impl Serialize for WorkspaceModelVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(1)
    }
}

impl<'de> Deserialize<'de> for WorkspaceModelVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        if value == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported WorkspaceModel version: {value} (only v1 is supported)"
            )))
        }
    }
}

/// Closed file-kind discriminant per RFC-32 §D1. `binary` when the
/// first 8 `KiB` of the file contains a `NUL` byte; otherwise `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    /// Decoded as UTF-8 text by the indexer (replacement bytes
    /// allowed per RFC-32 §D1).
    Text,
    /// Treated as opaque bytes; regex hints skip files of this kind.
    Binary,
}

/// Closed adapter-axis discriminant per RFC-32 §"Core entity
/// families (v1)". Matches the on-disk parent directory under
/// `adapters/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterAxis {
    /// Source adapter (`adapters/sources/<name>/`).
    Sources,
    /// Target adapter (`adapters/targets/<name>/`).
    Targets,
}

/// Closed scan-profile discriminant per RFC-32 §"`WorkspaceModel`"
/// extraction inputs.
///
/// `consumer` is the only Phase 2 profile. `framework` is reserved
/// for RFC-34; the indexer in S6 will refuse it. The variant exists
/// here so the v1 schema covers both names without execution.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ScanProfile {
    /// Consumer project scan; the only profile implemented in
    /// Phase 2.
    #[default]
    Consumer,
    /// Framework repo scan; reserved for RFC-34.
    Framework,
}

/// `file` fact per RFC-32 §"Core entity families (v1)" — produced
/// by the filesystem walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct File {
    /// Project-relative path with forward slashes per RFC-32
    /// §"Stability".
    pub path: String,
    /// Closed file-kind discriminant.
    pub kind: FileKind,
    /// Optional language token inferred from the extension or
    /// supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Optional content digest; populated when an extractor needs
    /// cross-file identity (e.g. canonical SHA checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// `frontmatter` fact per RFC-32 §"Core entity families (v1)" —
/// markdown `---` block extracted then YAML-parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Frontmatter {
    /// Project-relative path of the markdown file the frontmatter
    /// came from.
    pub path: String,
    /// Optional schema id the frontmatter declares (matches the
    /// registered-schema token shape from §D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<String>,
    /// Parsed YAML field map. Modelled as a [`serde_json::Map`] so
    /// key order round-trips byte-stably; per-key shape is
    /// rule-specific.
    pub fields: JsonMap<String, JsonValue>,
}

/// `markdown_section` fact per RFC-32 §"Core entity families (v1)"
/// — markdown structure pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MarkdownSection {
    /// Project-relative path of the markdown file.
    pub path: String,
    /// Markdown heading level (1–6).
    pub level: u8,
    /// Heading text after the leading `#`s, with surrounding
    /// whitespace trimmed.
    pub title: String,
    /// 1-based line of the heading line itself.
    pub line_start: u32,
    /// 1-based last line that belongs to this section (the line
    /// before the next same-or-higher-level heading, or the file's
    /// last line).
    pub line_end: u32,
    /// Number of non-heading body lines under the section.
    pub body_line_count: u32,
}

/// `markdown_link` fact per RFC-32 §"Core entity families (v1)" —
/// link scan with fence/comment stripping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MarkdownLink {
    /// Project-relative path of the markdown file containing the
    /// link.
    pub from_path: String,
    /// Verbatim link target as authored (relative path, URL, or
    /// anchor).
    pub to_raw: String,
    /// 1-based line of the link occurrence.
    pub line: u32,
    /// `true` when the target resolves on disk, `false` for broken
    /// references. Absent for off-tree URLs the indexer did not
    /// attempt to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolves: Option<bool>,
}

/// `symlink` fact per RFC-32 §"Core entity families (v1)".
/// Symlinks are recorded but not traversed per §D1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Symlink {
    /// Project-relative path of the symlink itself.
    pub path: String,
    /// Symlink target as recorded by the filesystem (may be
    /// relative or absolute).
    pub target: String,
    /// `true` when the link target does not exist on disk.
    pub broken: bool,
}

/// `skill` fact per RFC-32 §"Core entity families (v1)" —
/// extracted from `plugins/**/SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Skill {
    /// Skill name (matches the `name:` frontmatter field).
    pub name: String,
    /// Project-relative path of the `SKILL.md` file.
    pub path: String,
    /// Owning plugin slug (the directory under `plugins/`).
    pub plugin: String,
    /// Path back to the originating [`Frontmatter`] fact so
    /// consumers can join through the frontmatter table.
    pub frontmatter_ref: String,
}

/// `adapter_manifest` fact per RFC-32 §"Core entity families (v1)"
/// — extracted from `adapters/{sources,targets}/**/adapter.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AdapterManifest {
    /// Closed `sources` / `targets` discriminant.
    pub axis: AdapterAxis,
    /// Adapter name from `adapter.yaml`.
    pub name: String,
    /// Project-relative path of the `adapter.yaml` file.
    pub path: String,
    /// Optional manifest version; absent when the adapter does not
    /// pin one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `marketplace_entry` fact per RFC-32 §"Core entity families (v1)"
/// — extracted from `.cursor-plugin/marketplace.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MarketplaceEntry {
    /// Plugin slug declared by `.cursor-plugin/marketplace.json`.
    pub plugin: String,
    /// JSON-pointer-style location inside `marketplace.json` where
    /// the entry was discovered.
    pub path_in_manifest: String,
}

/// `codex_rule` fact per RFC-32 §"Core entity families (v1)" —
/// codex tree discovery (reuses the RFC-28 parser).
///
/// Named `CodexRuleFact` rather than `CodexRule` so the entity-fact
/// shape does not collide with the parsed-frontmatter
/// [`crate::rules::CodexRule`] DTO that ships its full body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CodexRuleFact {
    /// Codex rule id (matches the RFC-28 codex-rule schema's `id`
    /// regex).
    pub rule_id: String,
    /// Project-relative path of the codex rule markdown file.
    pub path: String,
    /// Which codex tree contributed the rule. Reuses the RFC-28
    /// [`crate::rules::Origin`] enum so resolver and review surfaces
    /// share one type.
    pub origin: Origin,
    /// Path back to the originating [`Frontmatter`] fact so
    /// consumers can join through the frontmatter table.
    pub frontmatter_ref: String,
}

/// `text_match` fact per RFC-32 §"Core entity families (v1)" —
/// optional precomputed regex index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TextMatch {
    /// Project-relative path of the file the match was found in.
    pub path: String,
    /// 1-based line of the match.
    pub line: u32,
    /// 1-based column of the match.
    pub column: u32,
    /// Stable identifier for the precomputed regex pattern that
    /// produced this match.
    pub pattern_id: String,
}

/// v1 `WorkspaceModel` envelope per RFC-32 §"`WorkspaceModel`" and
/// §"Schema location".
///
/// Important: per RFC-32 §"Persistence and query (v1 decision)" the
/// model is **not** persisted under `.specify/` and is **not** an
/// operator-facing Specify artifact in v1. v1 ships
/// `specrun review --dump-model` only;
/// `.specify/cache/workspace-model.v1.json` and
/// `specrun model query <selector>` are reserved surfaces.
///
/// Top-level keys are `snake_case` to match the schema under
/// `specify-cli/schemas/review/workspace-model.schema.json`; nested
/// entity facts carry their own kebab-case rename.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceModel {
    /// Version discriminant pinned to `1` per RFC-32
    /// §"`WorkspaceModel`".
    pub version: WorkspaceModelVersion,
    /// Scan root; project root for `consumer`, framework checkout
    /// for `framework`.
    pub project_dir: String,
    /// Profile selector that controls which extractors run.
    pub scan_profile: ScanProfile,
    /// Optional narrow path list; empty means the indexer performs
    /// a full profile-specific scan per §D1.
    pub artifact_paths: Vec<String>,
    /// Optional language tokens supplied by the caller or inferred
    /// from paths.
    pub languages: Vec<String>,
    /// `file` facts from the filesystem walk.
    pub files: Vec<File>,
    /// `frontmatter` facts from markdown `---` block extraction
    /// plus YAML parse.
    pub frontmatter: Vec<Frontmatter>,
    /// `markdown_section` facts from the markdown structure pass.
    pub markdown_sections: Vec<MarkdownSection>,
    /// `markdown_link` facts from the fence-aware link scan.
    pub markdown_links: Vec<MarkdownLink>,
    /// `symlink` facts; recorded but not traversed per §D1.
    pub symlinks: Vec<Symlink>,
    /// `skill` facts from `plugins/**/SKILL.md`.
    pub skills: Vec<Skill>,
    /// `adapter_manifest` facts from
    /// `adapters/{sources,targets}/**/adapter.yaml`.
    pub adapter_manifests: Vec<AdapterManifest>,
    /// `marketplace_entry` facts from
    /// `.cursor-plugin/marketplace.json`.
    pub marketplace_entries: Vec<MarketplaceEntry>,
    /// `codex_rule` facts from codex tree discovery.
    pub codex_rules: Vec<CodexRuleFact>,
    /// `text_match` facts from the optional precomputed regex
    /// index.
    pub text_matches: Vec<TextMatch>,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::WorkspaceModelVersion;

    #[test]
    fn version_serialises_as_one() {
        let v = serde_json::to_value(WorkspaceModelVersion).expect("serialise");
        assert_eq!(v, Value::from(1));
    }

    #[test]
    fn version_rejects_other_values() {
        let err = serde_json::from_value::<WorkspaceModelVersion>(Value::from(2))
            .expect_err("v2 must be rejected");
        assert!(err.to_string().contains("unsupported WorkspaceModel version"));
    }
}
