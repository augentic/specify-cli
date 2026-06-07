//! Integration test for the `content-digest-eq` hint evaluator.
//!
//! Exercises the config-driven `agent-teams-match-canonical` source —
//! every `agent-teams.md` symlink must resolve to content whose SHA-256
//! equals the canonical document named by `config: { canonical-path }`;
//! a symlink whose target digest diverges is flagged — over a framework
//! model, with no reference to any specify rule id. The canonical path
//! is policy supplied by the rule's `config`, never a `const` in the
//! engine arm.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint_with_config, make_rule};

const CANONICAL_REL: &str = "docs/reference/review-team-protocol.md";
const CANONICAL_BODY: &str = "# Review Team Protocol\n\nCanonical body.\n";
const DIVERGENT_REL: &str = "docs/reference/legacy-review-team.md";
const DIVERGENT_BODY: &str = "# Legacy\n\nDrifted copy.\n";

fn link(project: &Path, adapter: &str, target_rel: &str) {
    let link_dir = project.join("adapters/targets").join(adapter).join("references");
    fs::create_dir_all(&link_dir).expect("link parent");
    let link_path = link_dir.join("agent-teams.md");
    // adapters/targets/<adapter>/references/ is four levels deep.
    let link_target = format!("../../../../{target_rel}");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&link_target, &link_path).expect("unix symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&link_target, &link_path).expect("windows symlink");
}

fn divergent_overlays(project: &Path) -> BTreeSet<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "UNI-970",
        vec![hint_with_config(
            HintKind::ContentDigestEq,
            "agent-teams-match-canonical",
            Some(json!({ "canonical-path": CANONICAL_REL })),
        )],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("agent-team").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn flags_only_drifted_overlay() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs/reference")).expect("docs/reference");
    fs::write(tmp.path().join(CANONICAL_REL), CANONICAL_BODY).expect("canonical");
    fs::write(tmp.path().join(DIVERGENT_REL), DIVERGENT_BODY).expect("divergent");

    link(tmp.path(), "aligned-a", CANONICAL_REL);
    link(tmp.path(), "aligned-b", CANONICAL_REL);
    link(tmp.path(), "drifted", DIVERGENT_REL);

    let flagged = divergent_overlays(tmp.path());
    let expected: BTreeSet<String> =
        std::iter::once("adapters/targets/drifted/references/agent-teams.md".to_string()).collect();
    assert_eq!(flagged, expected, "only the overlay resolving to a divergent digest is flagged");
}

#[test]
fn all_aligned_overlays_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::create_dir_all(tmp.path().join("docs/reference")).expect("docs/reference");
    fs::write(tmp.path().join(CANONICAL_REL), CANONICAL_BODY).expect("canonical");

    link(tmp.path(), "aligned-a", CANONICAL_REL);
    link(tmp.path(), "aligned-b", CANONICAL_REL);

    let flagged = divergent_overlays(tmp.path());
    assert!(flagged.is_empty(), "overlays all matching canonical produce no findings: {flagged:?}");
}
