//! Entity-fact DTOs for the `WorkspaceModel` envelope.
//!
//! Each struct is one fact family the `specify lint` indexer
//! produces. Nested keys are kebab-case (`line-start`, `from-path`,
//! `frontmatter-ref`, …) per
//! `specify-cli/schemas/lint/workspace-model.schema.json`; every
//! struct carries `#[serde(rename_all = "kebab-case")]`.

use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::{AdapterAxis, FileKind};
use crate::rules::Origin;

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
    /// `true` when the link is an image embed (`![alt](src)`) rather
    /// than a plain `[label](target)` link. Omitted from the wire when
    /// `false` so plain-link facts keep their existing shape.
    #[serde(default, skip_serializing_if = "is_false")]
    pub image: bool,
}

/// `skip_serializing_if` predicate: omit a `bool` field when `false`.
#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde skip predicates take `&T`")]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// `fenced_block` fact — closed fence body extracted for fence-aware evaluators
/// (CORE-037, CORE-017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FencedBlock {
    /// Project-relative path of the markdown file.
    pub path: String,
    /// 1-based line of the first body line inside the fence.
    pub line_start: u32,
    /// 1-based line of the closing fence delimiter.
    pub line_end: u32,
    /// Info string from the opening fence (`json`, `text`, …); empty when absent.
    pub lang: String,
    /// Fence body lines joined with `\n` (excludes opening/closing delimiters).
    pub body: String,
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

/// Closed brief-scope discriminant: parent brief vs phase sub-brief.
///
/// The two carry different size budgets (a rule supplies the caps via
/// `config`), so the scope is the selector a `cardinality` brief
/// metric narrows on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BriefScope {
    /// Parent orchestrator brief at
    /// `adapters/<axis>/<adapter>/briefs/<operation>.md`.
    Parent,
    /// Phase sub-brief under
    /// `adapters/<axis>/<adapter>/briefs/{build,extract}/**/*.md`.
    Phase,
}

/// `brief` fact per the `WorkspaceModel` entity families —
/// extracted from `adapters/**/briefs/**/*.md` under the framework
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
    /// Operation slug for the brief. For a parent brief this is the
    /// brief stem (`extract`, `survey`, `shape`, `build`, `merge`);
    /// for a phase sub-brief it is the phase directory (`build`,
    /// `extract`).
    pub operation: String,
    /// Parent vs phase classification — the size-budget selector.
    pub scope: BriefScope,
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
    /// Operation slugs declared as the `briefs:` map keys in the
    /// manifest body. Empty when the manifest omits the field or
    /// declares an empty map. Consumed by the `kind: set-coverage`
    /// and `kind: set-eq` interpreters via the `adapter-briefs`
    /// discriminator to detect manifests whose `briefs.keys()` do
    /// not match the rule-supplied expected operation set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub brief_keys: Vec<String>,
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
/// Produced by the indexer pass that recognises
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
