//! `WorkspaceModel` DTOs per the standards-layer contract §"`WorkspaceModel`" and
//! §"Core entity families (v1)".
//!
//! The model is the deterministic, versioned snapshot of project
//! facts the `specrun lint` indexer produces once per run
//! invocation. Per the standards-layer contract §"Persistence and query (v1 decision)" it
//! is an internal execution artifact: **not** persisted under
//! `.specify/` and **not** an operator-facing Specify artifact in
//! v1. `.specify/cache/workspace-model.v1.json` and
//! `specrun model query <selector>` are reserved surfaces with no
//! implementation behind them.
//!
//! The DTOs round-trip through `specify_schema::WORKSPACE_MODEL_JSON_SCHEMA`
//! per the standards-layer contract §"Schema location". The envelope's `version: 1`
//! discriminant pins the wire shape; per the standards-layer contract §"`WorkspaceModel`"
//! breaking indexer output bumps the version.
//!
//! Wire shape notes:
//!
//! - Top-level envelope keys are `snake_case` (`project_dir`,
//!   `scan_profile`, `markdown_sections`, …) to match the JSON
//!   Schema under `specify-cli/schemas/lint/workspace-model.schema.json`.
//! - Nested entity-fact keys are kebab-case (`line-start`,
//!   `from-path`, `frontmatter-ref`, …) per the same schema; each
//!   entity struct carries `#[serde(rename_all = "kebab-case")]`.
//! - Every array on the envelope is always serialised, even when
//!   empty, so JSON consumers can rely on the full set of fact
//!   families existing.

#![allow(
    clippy::module_name_repetitions,
    reason = "The public schema uses the `WorkspaceModel` envelope name; the surrounding `model` module is the navigational home for the DTO layer."
)]

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::rules::Origin;

/// Type-level pin of the `WorkspaceModel` envelope version.
///
/// Serialises to the integer `1` and refuses to deserialise any
/// other value per the standards-layer contract §"`WorkspaceModel`". Modelled as a unit
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

/// Closed file-kind discriminant per `WorkspaceModel` file scan. `binary` when the
/// first 8 `KiB` of the file contains a `NUL` byte; otherwise `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    /// Decoded as UTF-8 text by the indexer (replacement bytes
    /// allowed per `WorkspaceModel` file scan).
    Text,
    /// Treated as opaque bytes; regex hints skip files of this kind.
    Binary,
}

/// Closed adapter-axis discriminant per the standards-layer contract §"Core entity
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

/// Closed scan-profile discriminant per the standards-layer contract §"`WorkspaceModel`"
/// extraction inputs.
///
/// `consumer` is the only Phase 2 profile. `framework` is reserved
/// for a future framework scan; the v1 indexer refuses it. The variant exists
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
    /// Framework repo scan; reserved for a future framework scan.
    Framework,
}

/// `file` fact per the `WorkspaceModel` entity families — produced
/// by the filesystem walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct File {
    /// Project-relative path with forward slashes per the standards-layer contract
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

/// `frontmatter` fact per the `WorkspaceModel` entity families —
/// markdown `---` block extracted then YAML-parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Frontmatter {
    /// Project-relative path of the markdown file the frontmatter
    /// came from.
    pub path: String,
    /// Optional schema id the frontmatter declares (matches the
    /// registered-schema token shape from the registered-schema token shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<String>,
    /// Parsed YAML field map. Modelled as a [`serde_json::Map`] so
    /// key order round-trips byte-stably; per-key shape is
    /// rule-specific.
    pub fields: JsonMap<String, JsonValue>,
}

/// `markdown_section` fact per the `WorkspaceModel` entity families
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

/// `markdown_link` fact per the `WorkspaceModel` entity families —
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

/// `symlink` fact per the `WorkspaceModel` entity families.
///
/// Recorded but not traversed under the consumer file scan contract;
/// the framework profile additionally follows the link and records
/// the resolved canonical endpoint in [`Self::resolved_target`] per
/// the standards-layer contract §F1.
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
    /// Project-relative path of the resolved endpoint after
    /// canonicalisation. Populated only by the framework scan
    /// profile per §F1 (`follow` mode); absent under the consumer
    /// profile and absent for broken links the walker could not
    /// resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_target: Option<String>,
}

/// `skill` fact per the `WorkspaceModel` entity families —
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
    /// Number of non-frontmatter body lines under the skill body.
    /// Populated by the framework profile; absent when the indexer
    /// did not compute it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_line_count: Option<u32>,
}

/// `brief` fact per the `WorkspaceModel` entity families —
/// extracted from `adapters/**/briefs/*.md` under the framework
/// profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Brief {
    /// Project-relative path of the brief markdown file.
    pub path: String,
    /// Owning adapter axis (`sources` xor `targets`).
    pub axis: AdapterAxis,
    /// Owning adapter slug (the directory under
    /// `adapters/{sources,targets}/`).
    pub adapter: String,
    /// Operation slug for the brief (e.g. `enumerate`, `extract`,
    /// `shape`, `build`, `merge`).
    pub operation: String,
    /// `##` heading titles found in the body, in document order
    /// after fence and HTML-comment stripping.
    pub sections: Vec<String>,
    /// Total non-empty markdown body lines (frontmatter excluded
    /// when present).
    pub body_line_count: u32,
}

/// `agent_team` fact per the `WorkspaceModel` entity families.
///
/// Produced by the framework profile when it follows an
/// `agent-teams.md` symlink into the canonical review-team-protocol
/// document. The endpoint pair plus content digest lets the
/// review-team drift rule reason about both sides of the link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AgentTeam {
    /// Project-relative path of the `agent-teams.md` symlink itself.
    pub path: String,
    /// Symlink target as recorded by `read_link` (may be relative
    /// or absolute, possibly outside the project tree).
    pub target_raw: String,
    /// Project-relative path of the resolved canonical endpoint
    /// when it lives under `project_dir`; absent when the target
    /// resolves outside the tree or could not be canonicalised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_target: Option<String>,
    /// Hex-encoded SHA-256 of the resolved target file's bytes.
    /// Absent when the target is unreadable or broken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha256: Option<String>,
}

/// `adapter_manifest` fact per the `WorkspaceModel` entity families
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

/// `marketplace_entry` fact per the `WorkspaceModel` entity families
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

/// `rule_index` fact per the `WorkspaceModel` entity families —
/// rules tree discovery (reuses the rule frontmatter parser).
///
/// Named `RuleIndexEntry` rather than `Rule` so the entity-fact
/// shape does not collide with the parsed-frontmatter
/// [`crate::rules::Rule`] DTO that ships its full body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RuleIndexEntry {
    /// Rule id (matches the rule schema's `id`
    /// regex).
    pub rule_id: String,
    /// Project-relative path of the rule markdown file.
    pub path: String,
    /// Which rules tree contributed the rule. Reuses the rules contract
    /// [`crate::rules::Origin`] enum so resolver and review surfaces
    /// share one type.
    pub origin: Origin,
    /// Path back to the originating [`Frontmatter`] fact so
    /// consumers can join through the frontmatter table.
    pub frontmatter_ref: String,
}

/// `text_match` fact per the `WorkspaceModel` entity families —
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

/// `ignore_directive` fact per the `WorkspaceModel` entity families.
///
/// Produced by the RFC-33a indexer pass that recognises
/// `specify-ignore: <RULE-ID> — <rationale>` comments across the
/// closed comment-style list (C-family, hash, HTML, SQL/Lua).
/// Malformed directives (missing or empty rationale) are still
/// emitted with `rationale = None` so the directive-validation pass
/// can synthesise `UNI-022` / `UNI-023` findings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct IgnoreDirective {
    /// Project-relative path of the file containing the directive
    /// comment.
    pub path: String,
    /// 1-based line of the directive comment itself.
    pub line: u32,
    /// Rule id named by the directive. Not pattern-pinned so
    /// malformed ids surface as `UNI-023` candidates downstream.
    pub rule_id: String,
    /// Verbatim rationale text from the directive. `None` when the
    /// directive lacked a rationale (the directive-validation pass
    /// emits `UNI-022` for that case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// 1-based line the directive applies to. Inline trailing
    /// directives target their own line; block-leading directives
    /// target the next non-blank, non-comment line; directives at
    /// end-of-file with no following target line use the line
    /// number one past the file's last line so the validation pass
    /// can detect orphan placement.
    pub target_line: u32,
    /// Raw directive comment text as captured by the indexer,
    /// including delimiters (e.g. `// specify-ignore: …`,
    /// `/* specify-ignore: … */`).
    pub raw: String,
}

/// v1 `WorkspaceModel` envelope per the standards-layer contract §"`WorkspaceModel`" and
/// §"Schema location".
///
/// Important: per the standards-layer contract §"Persistence and query (v1 decision)" the
/// model is **not** persisted under `.specify/` and is **not** an
/// operator-facing Specify artifact in v1. v1 ships
/// `specrun lint --dump-model` only;
/// `.specify/cache/workspace-model.v1.json` and
/// `specrun model query <selector>` are reserved surfaces.
///
/// Top-level keys are `snake_case` to match the schema under
/// `specify-cli/schemas/lint/workspace-model.schema.json`; nested
/// entity facts carry their own kebab-case rename.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceModel {
    /// Version discriminant pinned to `1` per the standards-layer contract
    /// §"`WorkspaceModel`".
    pub version: WorkspaceModelVersion,
    /// Scan root; project root for `consumer`, framework checkout
    /// for `framework`.
    pub project_dir: String,
    /// Profile selector that controls which extractors run.
    pub scan_profile: ScanProfile,
    /// Optional narrow path list; empty means the indexer performs
    /// a full profile-specific scan by the file scan contract.
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
    /// `symlink` facts; recorded but not traversed by the file scan contract.
    pub symlinks: Vec<Symlink>,
    /// `skill` facts from `plugins/**/SKILL.md`.
    pub skills: Vec<Skill>,
    /// `adapter_manifest` facts from
    /// `adapters/{sources,targets}/**/adapter.yaml`.
    pub adapter_manifests: Vec<AdapterManifest>,
    /// `marketplace_entry` facts from
    /// `.cursor-plugin/marketplace.json`.
    pub marketplace_entries: Vec<MarketplaceEntry>,
    /// `rule_index` facts from rules tree discovery.
    pub rule_index: Vec<RuleIndexEntry>,
    /// `text_match` facts from the optional precomputed regex
    /// index.
    pub text_matches: Vec<TextMatch>,
    /// `ignore_directive` facts from the RFC-33a directive indexer.
    /// Optional in v1 envelopes per the schema; the producer always
    /// serialises the array so consumers see one consistent wire
    /// shape.
    #[serde(default)]
    pub ignore_directives: Vec<IgnoreDirective>,
    /// `brief` facts from `adapters/**/briefs/*.md` under the
    /// framework scan profile. Optional in v1 envelopes; producers
    /// omit the field when empty so the consumer profile's wire
    /// shape is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub briefs: Vec<Brief>,
    /// `agent_team` facts from followed `agent-teams.md` symlinks
    /// under the framework scan profile. Optional in v1 envelopes;
    /// producers omit the field when empty so the consumer
    /// profile's wire shape is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_teams: Vec<AgentTeam>,
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
