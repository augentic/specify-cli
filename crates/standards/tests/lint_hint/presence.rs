//! Integration test for the `presence` hint evaluator.
//!
//! Exercises the four mechanism selectors over a framework model with
//! no reference to any real `CORE-NNN`:
//!
//! - `frontmatter` — candidate `SKILL.md` files absent from (or empty
//!   in) the frontmatter fact family are flagged.
//! - `file` — a single required `config: { path }` is flagged when no
//!   file fact carries it.
//! - `markdown-section` — skills whose `skill-body-line-count` reaches
//!   the `config` threshold but lack the required section are flagged,
//!   with the boundary proven at `min` and `min - 1`.
//! - `directory-index` — corpus directories matching a `roots` glob
//!   with enough files beneath them but no index file are flagged;
//!   indexed and below-threshold directories pass, and the glob stays
//!   one directory depth (`*` does not cross `/`).
//!
//! Every value (the required path, the section title / level, the
//! threshold, the corpus roots / index name) is policy supplied by the
//! rule's `config`, never a `const` in the engine arm.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_skill(project: &Path, plugin: &str, skill: &str, content: &str) {
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

/// A SKILL.md whose body holds exactly `lines` non-frontmatter body
/// lines (matching the indexer's `body_line_count`), optionally
/// carrying a `## Critical Path` H2 section.
fn skill_body(lines: usize, with_section: bool) -> String {
    let mut body = String::new();
    if with_section {
        body.push_str("## Critical Path\n");
    }
    while body.lines().count() < lines {
        body.push_str("padding line\n");
    }
    format!("---\nname: long\ndescription: Build the fixture.\n---\n{body}")
}

fn flagged_paths(project: &Path, rule_id: &str, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(rule_id, hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut paths: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("path").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    paths.sort();
    paths
}

#[test]
fn frontmatter_flags_missing_and_empty() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "ok", "good", "---\nname: good\ndescription: x\n---\n\nBody.\n");
    write_skill(tmp.path(), "bare", "none", "# No frontmatter here\n");
    write_skill(tmp.path(), "blank", "empty", "---\n\n---\n\nBody.\n");

    let flagged = flagged_paths(
        tmp.path(),
        "UNI-970",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint(HintKind::Presence, "frontmatter"),
        ],
    );
    assert_eq!(
        flagged,
        vec![
            "plugins/bare/skills/none/SKILL.md".to_string(),
            "plugins/blank/skills/empty/SKILL.md".to_string(),
        ],
        "the blockless and empty-frontmatter skills are flagged; the valid one passes",
    );
}

#[test]
fn file_flags_missing_required_path() {
    let tmp = tempfile::tempdir().expect("tmp");
    let present = tmp.path().join("docs/reference/present.md");
    fs::create_dir_all(present.parent().expect("parent")).expect("docs dir");
    fs::write(&present, "# Present\n").expect("write doc");

    let missing = flagged_paths(
        tmp.path(),
        "UNI-971",
        vec![hint_with_config(
            HintKind::Presence,
            "file",
            Some(json!({ "path": "docs/reference/absent.md" })),
        )],
    );
    assert_eq!(missing, vec!["docs/reference/absent.md".to_string()], "absent path is flagged");

    let resolved = flagged_paths(
        tmp.path(),
        "UNI-971",
        vec![hint_with_config(
            HintKind::Presence,
            "file",
            Some(json!({ "path": "docs/reference/present.md" })),
        )],
    );
    assert!(resolved.is_empty(), "present path produces no finding: {resolved:?}");
}

#[test]
fn section_flags_long_skill_without_it() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "long", "nocp", &skill_body(8, false));
    write_skill(tmp.path(), "long", "withcp", &skill_body(8, true));

    let flagged = flagged_paths(tmp.path(), "UNI-972", vec![section_hint(5)]);
    assert_eq!(
        flagged,
        vec!["plugins/long/skills/nocp/SKILL.md".to_string()],
        "the long skill lacking the section is flagged; the one carrying it passes",
    );
}

#[test]
fn section_threshold_boundary() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "edge", "at", &skill_body(5, false));
    write_skill(tmp.path(), "edge", "below", &skill_body(4, false));

    let flagged = flagged_paths(tmp.path(), "UNI-973", vec![section_hint(5)]);
    assert_eq!(
        flagged,
        vec!["plugins/edge/skills/at/SKILL.md".to_string()],
        "a skill at the threshold fires; one a line below it does not",
    );
}

/// A `markdown-section` presence hint requiring an H2 `## Critical Path`
/// once a skill reaches `min` body lines.
fn section_hint(min: u32) -> RuleHint {
    hint_with_config(
        HintKind::Presence,
        "markdown-section",
        Some(json!({
            "title": "Critical Path",
            "level": 2,
            "when": { "metric": "skill-body-line-count", "min": min }
        })),
    )
}

fn write_doc(project: &Path, rel: &str) {
    let path = project.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("doc dir");
    fs::write(&path, "# Doc\n").expect("write doc");
}

/// A `directory-index` presence hint over `adapters/*/*/references/*`
/// corpus directories requiring a `README.md` once `min` files live
/// beneath the directory.
fn directory_index_hint(min: u32) -> RuleHint {
    hint_with_config(
        HintKind::Presence,
        "directory-index",
        Some(json!({
            "roots": ["adapters/*/*/references/*"],
            "index": "README.md",
            "min-files": min
        })),
    )
}

#[test]
fn directory_index_flags_unindexed_corpus() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Unindexed corpus: two files, no README.md → flagged.
    write_doc(tmp.path(), "adapters/targets/demo/references/guides/a.md");
    write_doc(tmp.path(), "adapters/targets/demo/references/guides/b.md");
    // Indexed corpus: same shape plus README.md → passes.
    write_doc(tmp.path(), "adapters/targets/demo/references/indexed/a.md");
    write_doc(tmp.path(), "adapters/targets/demo/references/indexed/b.md");
    write_doc(tmp.path(), "adapters/targets/demo/references/indexed/README.md");
    // Sparse corpus: one file, below min-files → passes.
    write_doc(tmp.path(), "adapters/targets/demo/references/sparse/only.md");

    let flagged = flagged_paths(tmp.path(), "UNI-974", vec![directory_index_hint(2)]);
    assert_eq!(
        flagged,
        vec!["adapters/targets/demo/references/guides".to_string()],
        "only the unindexed at-threshold corpus directory is flagged",
    );
}

#[test]
fn directory_index_counts_nested_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Files nest one level deeper; the references/<sub> dir still owes
    // the index (recursive count), but the nested dirs themselves are
    // not matched by the one-depth glob.
    write_doc(tmp.path(), "adapters/targets/demo/references/examples/core/a.md");
    write_doc(tmp.path(), "adapters/targets/demo/references/examples/shell/b.md");

    let flagged = flagged_paths(tmp.path(), "UNI-975", vec![directory_index_hint(2)]);
    assert_eq!(
        flagged,
        vec!["adapters/targets/demo/references/examples".to_string()],
        "the depth-one corpus dir is flagged once; nested dirs are out of glob scope",
    );
}
