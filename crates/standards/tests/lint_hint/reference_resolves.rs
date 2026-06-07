//! Integration test for the `reference-resolves` hint evaluator.
//!
//! Exercises the evaluator's mechanism — unresolved `markdown-link`
//! detection, the `target-prefixes` / `target-suffix` / `image` config
//! filters, and the `symlink` source scoped by `path-prefix` — over a
//! framework model, with no reference to any specify rule id.

use std::fs;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn run(project: &std::path::Path, rule_id: &str, hints: Vec<RuleHint>) -> Vec<(String, String)> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(rule_id, hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    for finding in &outcome.findings {
        assert_eq!(finding.rule_id.as_deref(), Some(rule_id));
    }
    outcome
        .findings
        .iter()
        .filter_map(|f| {
            let loc = f.location.as_ref()?;
            match &f.evidence {
                FindingEvidence::Snippet { value } => Some((loc.path.clone(), value.clone())),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn flags_only_unresolved_markdown_links() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs")).expect("docs");
    fs::write(tmp.path().join("docs/there.md"), "# there\n").expect("write target");
    fs::write(tmp.path().join("docs/a.md"), "[ok](./there.md) and [bad](./missing.md)\n")
        .expect("write a");

    let flagged = run(
        tmp.path(),
        "UNI-900",
        vec![
            hint(HintKind::PathPattern, "docs/**/*.md"),
            hint(HintKind::ReferenceResolves, "markdown-link"),
        ],
    );
    assert_eq!(flagged, vec![("docs/a.md".to_string(), "./missing.md".to_string())]);
}

#[test]
fn restricts_to_configured_target_prefixes() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs")).expect("docs");
    fs::write(
        tmp.path().join("docs/a.md"),
        "[ref](references/gone.md) and [other](../also-gone.md)\n",
    )
    .expect("write a");

    let flagged = run(
        tmp.path(),
        "UNI-901",
        vec![
            hint(HintKind::PathPattern, "docs/**/*.md"),
            hint_with_config(
                HintKind::ReferenceResolves,
                "markdown-link",
                Some(json!({ "target-prefixes": ["references/"] })),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec![("docs/a.md".to_string(), "references/gone.md".to_string())],
        "only the references/-prefixed broken link is flagged",
    );
}

#[test]
fn selects_image_embeds_by_suffix() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs")).expect("docs");
    fs::write(
        tmp.path().join("docs/a.md"),
        "![svg](missing.svg)\n![png](missing.png)\n[plain](missing.md)\n",
    )
    .expect("write a");

    let flagged = run(
        tmp.path(),
        "UNI-902",
        vec![
            hint(HintKind::PathPattern, "docs/**/*.md"),
            hint_with_config(
                HintKind::ReferenceResolves,
                "markdown-link",
                Some(json!({ "image": true, "target-suffix": ".svg" })),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec![("docs/a.md".to_string(), "missing.svg".to_string())],
        "only the .svg image embed is flagged; the png embed and plain link are skipped",
    );
}

#[cfg(unix)]
#[test]
fn flags_broken_symlinks_by_prefix() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("plugins/p")).expect("plugins");
    fs::create_dir_all(tmp.path().join("docs")).expect("docs");
    symlink("nope.md", tmp.path().join("plugins/p/dangling.md")).expect("plugins symlink");
    symlink("nope.md", tmp.path().join("docs/dangling.md")).expect("docs symlink");

    let flagged: Vec<String> = {
        let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("build");
        let rule = make_rule(
            "UNI-903",
            vec![hint_with_config(
                HintKind::ReferenceResolves,
                "symlink",
                Some(json!({ "path-prefix": "plugins/" })),
            )],
        );
        let runner: &dyn ToolRunner = &NoToolRunner;
        let outcome = evaluate(
            &rule,
            rule.rule_hints.as_deref().unwrap_or_default(),
            &model,
            tmp.path(),
            runner,
            1,
        )
        .expect("evaluate");
        outcome.findings.iter().filter_map(|f| Some(f.location.as_ref()?.path.clone())).collect()
    };
    assert_eq!(
        flagged,
        vec!["plugins/p/dangling.md".to_string()],
        "only the broken symlink under plugins/ is flagged",
    );
}
