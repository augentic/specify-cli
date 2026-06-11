//! `kind: presence` evaluator.
//!
//! Flags the absence of a required artifact. `hint.value` selects one
//! of four mechanism selectors:
//!
//! - `frontmatter` — flag each candidate file (the rule's
//!   `path-pattern` set) that is absent from [`WorkspaceModel::frontmatter`]
//!   or whose frontmatter parsed to an empty field map (missing,
//!   unparseable, or empty frontmatter). For CORE-042.
//! - `file` — `config: { path }`; flag the single required `path`
//!   when it is absent from [`WorkspaceModel::files`]. Whole-tree (the
//!   `path-pattern` candidate set is a sentinel and unused). For
//!   CORE-011.
//! - `markdown-section` — `config: { title, level, when: { metric, min } }`;
//!   over the [`crate::lint::Skill`] facts whose `metric` reaches
//!   `min`, flag those lacking a [`crate::lint::MarkdownSection`] with
//!   the configured `title` and `level`. Whole-tree (the `path-pattern`
//!   candidate set is a sentinel and unused). For CORE-041.
//! - `directory-index` — `config: { roots, index, min-files? }`; over
//!   the directory prefixes of [`WorkspaceModel::files`], flag each
//!   directory matching a `roots` glob (`*` does not cross `/`) that
//!   holds at least `min-files` files beneath it but no `index` file
//!   directly inside it. Whole-tree. For CORE-059 (reference-corpus
//!   context-budget indexes).
//!
//! All policy (the required path, the section title / level, the metric
//! threshold, the corpus roots and index name) rides the rule's
//! `config:`; this arm names only mechanism — the selector tokens and
//! the single supported metric. Unknown selectors, an unsupported
//! metric, or a missing required config field are rejected as
//! [`super::HintError::Unsupported`] so authoring drift surfaces at
//! hint-evaluation time rather than silently passing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use glob::{MatchOptions, Pattern};
use serde::Deserialize;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const VALUE_FRONTMATTER: &str = "frontmatter";
const VALUE_FILE: &str = "file";
const VALUE_MARKDOWN_SECTION: &str = "markdown-section";
const VALUE_DIRECTORY_INDEX: &str = "directory-index";
/// The single fact metric the `markdown-section` selector gates on
/// today; naming a fact metric is mechanism, the threshold is policy.
const METRIC_SKILL_BODY_LINE_COUNT: &str = "skill-body-line-count";

/// Parsed `presence` hint configuration. Every field is optional at
/// parse time; each selector validates the fields it needs and rejects
/// the rest. The shape is schema-gated upstream by `presenceHintConfig`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct PresenceConfig {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    level: Option<u8>,
    #[serde(default)]
    when: Option<PresenceWhen>,
    #[serde(default)]
    roots: Option<Vec<String>>,
    #[serde(default)]
    index: Option<String>,
    #[serde(default)]
    min_files: Option<usize>,
}

/// The `when: { metric, min }` threshold gate for the
/// `markdown-section` selector.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct PresenceWhen {
    metric: String,
    min: u32,
}

impl PresenceConfig {
    fn parse(rule: &ResolvedRule, hint: &RuleHint) -> Result<Self, HintError> {
        let raw = hint.config.as_ref().ok_or_else(|| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "this `presence` selector requires a `config`",
        })?;
        serde_json::from_value(raw.clone()).map_err(|_ignored| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "invalid presence hint config JSON",
        })
    }
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    match hint.value.trim() {
        VALUE_FRONTMATTER => Ok(evaluate_frontmatter(rule, candidates, model, next_id)),
        VALUE_FILE => evaluate_file(rule, hint, model, next_id),
        VALUE_MARKDOWN_SECTION => evaluate_markdown_section(rule, hint, model, next_id),
        VALUE_DIRECTORY_INDEX => evaluate_directory_index(rule, hint, model, next_id),
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "only `frontmatter`, `file`, `markdown-section`, or `directory-index` is supported in v1",
        }),
    }
}

/// `frontmatter` selector: each candidate file lacking a non-empty
/// frontmatter fact (absent, unparseable, or an empty field map) is
/// flagged. Narrowed by the `path-pattern` candidate set.
fn evaluate_frontmatter(
    rule: &ResolvedRule, candidates: &[PathBuf], model: &WorkspaceModel, next_id: &mut u64,
) -> Vec<Diagnostic> {
    let present: BTreeSet<&str> = model
        .frontmatter
        .iter()
        .filter(|fm| !fm.fields.is_empty())
        .map(|fm| fm.path.as_str())
        .collect();
    let mut out: Vec<Diagnostic> = Vec::new();
    for candidate in super::candidate_set(candidates) {
        if present.contains(candidate.as_str()) {
            continue;
        }
        let summary = format!("missing or empty frontmatter in '{candidate}'");
        let finding = mint(rule, &candidate, &summary, next_id);
        out.push(finding);
    }
    out
}

/// `file` selector: flag the single required `config: { path }` when no
/// [`crate::lint::File`] fact carries that path. Whole-tree.
fn evaluate_file(
    rule: &ResolvedRule, hint: &RuleHint, model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg = PresenceConfig::parse(rule, hint)?;
    let path = cfg.path.ok_or_else(|| HintError::Unsupported {
        rule_id: rule.rule_id.clone(),
        kind: HintKind::Presence,
        reason: "`file` requires a `config: { path }`",
    })?;
    if model.files.iter().any(|file| file.path == path) {
        return Ok(Vec::new());
    }
    let summary = format!("required file '{path}' is missing");
    Ok(vec![mint(rule, &path, &summary, next_id)])
}

/// `markdown-section` selector: over the skill fact family, flag each
/// skill whose `metric` reaches `min` but carries no markdown section
/// with the configured `title` and `level`. Whole-tree.
fn evaluate_markdown_section(
    rule: &ResolvedRule, hint: &RuleHint, model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg = PresenceConfig::parse(rule, hint)?;
    let (Some(title), Some(level), Some(when)) = (cfg.title, cfg.level, cfg.when) else {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "`markdown-section` requires `config: { title, level, when: { metric, min } }`",
        });
    };
    if when.metric != METRIC_SKILL_BODY_LINE_COUNT {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "only the `skill-body-line-count` metric is supported in v1",
        });
    }
    let mut out: Vec<Diagnostic> = Vec::new();
    for skill in &model.skills {
        if skill.body_line_count.unwrap_or(0) < when.min {
            continue;
        }
        let has_section = model.markdown_sections.iter().any(|section| {
            section.path == skill.path && section.level == level && section.title == title
        });
        if has_section {
            continue;
        }
        let summary = format!("missing required '{title}' section (level {level})");
        out.push(mint(rule, &skill.path, &summary, next_id));
    }
    Ok(out)
}

/// `directory-index` selector: each directory prefix of a file fact
/// that matches a `roots` glob and holds at least `min-files` files
/// beneath it (recursive) must contain the `index` file directly. Glob
/// matching keeps `/` literal so a root pattern names one directory
/// depth. Whole-tree.
fn evaluate_directory_index(
    rule: &ResolvedRule, hint: &RuleHint, model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg = PresenceConfig::parse(rule, hint)?;
    let (Some(roots), Some(index)) = (cfg.roots, cfg.index) else {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "`directory-index` requires `config: { roots, index }`",
        });
    };
    let min_files = cfg.min_files.unwrap_or(1);
    let patterns: Vec<Pattern> = roots
        .iter()
        .map(|root| Pattern::new(root))
        .collect::<Result<_, _>>()
        .map_err(|_silenced| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Presence,
            reason: "invalid glob pattern in `roots`",
        })?;
    let options = MatchOptions {
        require_literal_separator: true,
        ..MatchOptions::default()
    };

    let mut dirs: BTreeSet<&str> = BTreeSet::new();
    for file in &model.files {
        let mut prefix = file.path.as_str();
        while let Some(pos) = prefix.rfind('/') {
            prefix = &prefix[..pos];
            if patterns.iter().any(|pattern| pattern.matches_with(prefix, options)) {
                dirs.insert(prefix);
            }
        }
    }

    let mut out: Vec<Diagnostic> = Vec::new();
    for dir in dirs {
        let beneath = format!("{dir}/");
        let count = model.files.iter().filter(|file| file.path.starts_with(&beneath)).count();
        if count < min_files {
            continue;
        }
        let required = format!("{dir}/{index}");
        if model.files.iter().any(|file| file.path == required) {
            continue;
        }
        let summary =
            format!("reference directory '{dir}' ({count} files) is missing its '{index}' index");
        out.push(mint(rule, dir, &summary, next_id));
    }
    Ok(out)
}

/// Mint one presence finding located at `path`, with structured
/// evidence carrying the offending path, and bump the id counter.
fn mint(rule: &ResolvedRule, path: &str, summary: &str, next_id: &mut u64) -> Diagnostic {
    let location = FindingLocation {
        path: path.to_owned(),
        line: None,
        column: None,
        end_line: None,
        end_column: None,
    };
    let evidence = FindingEvidence::Structured {
        summary: summary.to_owned(),
        data: serde_json::json!({ "path": path }),
        locations: None,
    };
    let title = format!("{}: {summary}", rule.title);
    let finding = make_finding(rule, *next_id, title, Some(location), evidence);
    *next_id += 1;
    finding
}
