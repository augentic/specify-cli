//! Integration tests for the `cli-contract` hint evaluator.
//!
//! Exercises the three mechanism selectors against a fabricated
//! binary-injected contract: `invocations` (verb-tree + flag walk over
//! fenced shell blocks and inline code spans), `event-ids`
//! (dotted-kebab inline spans + configured JSON fields), and
//! `error-codes` (configured JSON fields). The fence language set and
//! every exemption are policy supplied by the hint's `config`, never a
//! `const` in the engine arm, and the tests cite no specify rule id.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::contract::{CliContract, CommandNode};
use specify_standards::lint::eval::{EvalEnv, HintError, ToolRunner, evaluate, evaluate_env};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint_with_config, make_rule};

fn leaf(name: &str, args: &[&str]) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        about: None,
        args: args.iter().map(ToString::to_string).collect(),
        subcommands: Vec::new(),
    }
}

fn group(name: &str, subcommands: Vec<CommandNode>) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        about: None,
        args: Vec::new(),
        subcommands,
    }
}

/// A small fabricated contract: `specify plan {next, transition}`,
/// `specify slice build`, `specify tool run`, a global `--format`
/// flag on the root, short id lists, and a two-file tests inventory.
fn contract() -> CliContract {
    let mut root = group(
        "specify",
        vec![
            group(
                "plan",
                vec![
                    leaf("next", &["--plan-dir"]),
                    leaf("transition", &["<entry>", "<status>", "--undo"]),
                ],
            ),
            group("slice", vec![leaf("build", &["<slice>", "--phase"])]),
            group("tool", vec![leaf("run", &["<tool>", "<args>"])]),
        ],
    );
    root.args = vec!["--format".to_string()];
    CliContract {
        version: 1,
        binary_version: "0.0.0-test".to_string(),
        commands: root,
        exit_codes: Vec::new(),
        error_ids: vec!["adapter-not-found".to_string(), "plan-lock-not-held".to_string()],
        journal_event_ids: vec![
            "plan.transition.approved".to_string(),
            "slice.build.started".to_string(),
            "cli.upgraded".to_string(),
        ],
        schemas: Vec::new(),
        tests: vec![
            "tests/plan/end_to_end.rs".to_string(),
            "tests/fixtures/plan/golden.json".to_string(),
        ],
    }
}

/// Evaluate one hint over `project` with the fabricated contract and
/// return the offending tokens from the structured evidence.
fn flagged_tokens(project: &Path, hint: RuleHint) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-963", vec![hint]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let cli_contract = contract();
    let env = EvalEnv {
        model: &model,
        project_dir: project,
        tool_runner: runner,
        cli_contract: Some(&cli_contract),
    };
    let outcome = evaluate_env(&rule, rule.rule_hints.as_deref().unwrap_or_default(), env, 1)
        .expect("evaluate");
    let mut tokens: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("token").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    tokens.sort();
    tokens
}

fn invocations_hint(config: serde_json::Value) -> RuleHint {
    hint_with_config(HintKind::CliContract, "invocations", Some(config))
}

/// Write a doc under `docs/` — the framework walk's include set does
/// not cover loose root-level markdown.
fn write_doc(project: &Path, name: &str, body: &str) {
    let docs = project.join("docs");
    fs::create_dir_all(&docs).expect("docs dir");
    fs::write(docs.join(name), body).expect("write doc");
}

mod invocations {
    use super::*;

    #[test]
    fn known_commands_pass() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "# Doc\n\nRun `specify plan next --plan-dir .specify` or `specify plan transition <entry> approved`.\n\n\
             ```bash\nspecify --format json plan next\nspecify plan next --plan-dir=.specify || true\n\
             RESULT=$(specify slice build my-slice --phase prepare)\n\
             specify tool run vectis -- validate composition\n\
             specify plan \\\n  transition entry-a approved\n```\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash", "sh"] })));
        assert!(flagged.is_empty(), "all cited commands exist in the contract: {flagged:?}");
    }

    #[test]
    fn unknown_verb_flagged_in_fence() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "```bash\nspecify plan nxt\n```\n");
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert_eq!(flagged, vec!["nxt".to_string()]);
    }

    #[test]
    fn unknown_flag_flagged() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "```bash\nspecify plan next --plan-root here\n```\n");
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert_eq!(flagged, vec!["--plan-root".to_string()]);
    }

    #[test]
    fn unknown_verb_flagged_in_inline_span() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "Run `specify plann next` to advance.\n");
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert_eq!(flagged, vec!["plann".to_string()]);
    }

    #[test]
    fn ignore_exempts_token() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "```bash\nspecify plan nxt\n```\n");
        let flagged = flagged_tokens(
            tmp.path(),
            invocations_hint(json!({ "langs": ["bash"], "ignore": ["nxt"] })),
        );
        assert!(flagged.is_empty(), "ignored token is exempt: {flagged:?}");
    }

    #[test]
    fn passthrough_after_double_dash_unchecked() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "```bash\nspecify tool run contract -- \"$PROJECT_ROOT/contracts\" --format json\n```\n",
        );
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert!(flagged.is_empty(), "tokens after `--` are passthrough: {flagged:?}");
    }

    #[test]
    fn positional_after_leaf_not_a_verb() {
        let tmp = tempfile::tempdir().expect("tmp");
        // `platform-v2` and `approved` are kebab positionals of a leaf
        // verb with positionals declared — not unknown subcommands.
        write_doc(
            tmp.path(),
            "doc.md",
            "```bash\nspecify plan transition platform-v2 approved\n```\n",
        );
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert!(flagged.is_empty(), "leaf positionals are not verbs: {flagged:?}");
    }

    #[test]
    fn other_fence_langs_skipped() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "```text\nspecify plan nxt\n```\n");
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert!(flagged.is_empty(), "non-configured fence languages are skipped: {flagged:?}");
    }

    #[test]
    fn comment_prose_not_scanned() {
        let tmp = tempfile::tempdir().expect("tmp");
        // "specify onto" lives in a trailing comment, not a command.
        write_doc(
            tmp.path(),
            "doc.md",
            "```bash\nmake install-cli # symlinks specify onto your PATH\n```\n",
        );
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert!(flagged.is_empty(), "comment prose is not a command: {flagged:?}");
    }

    #[test]
    fn brace_shorthand_ends_walk() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Brace expansion makes the resolved verb ambiguous, so the
        // sub-verb flag after it must not be checked against `plan`.
        write_doc(tmp.path(), "doc.md", "Run `specify plan {next, transition} --undo` to act.\n");
        let flagged = flagged_tokens(tmp.path(), invocations_hint(json!({ "langs": ["bash"] })));
        assert!(flagged.is_empty(), "brace shorthand ends the walk: {flagged:?}");
    }
}

mod event_ids {
    use super::*;

    fn event_ids_hint(config: serde_json::Value) -> RuleHint {
        hint_with_config(HintKind::CliContract, "event-ids", Some(config))
    }

    #[test]
    fn known_ids_pass_unknown_flagged() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "Journals `plan.transition.approved` then `plan.transition.aproved` and `cli.upgraded`.\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), event_ids_hint(json!({ "json-fields": ["event"] })));
        assert_eq!(flagged, vec!["plan.transition.aproved".to_string()]);
    }

    #[test]
    fn suffix_and_ignore_exemptions() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "Reads `plan.yaml` and the `plan.lifecycle` field; emits `slice.build.started`.\n",
        );
        let flagged = flagged_tokens(
            tmp.path(),
            event_ids_hint(json!({
                "json-fields": ["event"],
                "ignore-suffixes": [".yaml"],
                "ignore": ["plan.lifecycle"],
            })),
        );
        assert!(flagged.is_empty(), "file names and YAML paths are exempt: {flagged:?}");
    }

    #[test]
    fn json_field_probe_flags_unknown() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "```json\n{\"event\": \"slice.build.startd\", \"other\": \"plan.transition.approved\"}\n```\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), event_ids_hint(json!({ "json-fields": ["event"] })));
        assert_eq!(flagged, vec!["slice.build.startd".to_string()]);
    }

    #[test]
    fn foreign_namespaces_skipped() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Dotted-kebab tokens outside the contract's event families
        // (file names, library APIs, domain notation) are not
        // candidates — inline or behind a configured JSON field.
        write_doc(
            tmp.path(),
            "doc.md",
            "Reads `go.mod`, calls `os.execvp`, models `users.register.happy-path`.\n\n\
             ```json\n{\"event\": \"order.placed\"}\n```\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), event_ids_hint(json!({ "json-fields": ["event"] })));
        assert!(flagged.is_empty(), "foreign namespaces are not candidates: {flagged:?}");
    }
}

mod error_codes {
    use super::*;

    fn error_codes_hint(config: serde_json::Value) -> RuleHint {
        hint_with_config(HintKind::CliContract, "error-codes", Some(config))
    }

    #[test]
    fn unknown_code_flagged() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "```json\n{\"error\": {\"code\": \"plan-lock-not-held\"}}\n```\n\n\
             ```json\n{\"error\": {\"code\": \"plan-lock-missing\"}}\n```\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), error_codes_hint(json!({ "json-fields": ["code"] })));
        assert_eq!(flagged, vec!["plan-lock-missing".to_string()]);
    }

    #[test]
    fn prefix_and_placeholder_exemptions() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "```json\n{\"code\": \"filesystem-rename\", \"also\": {\"code\": \"<code>\"}}\n```\n",
        );
        let flagged = flagged_tokens(
            tmp.path(),
            error_codes_hint(json!({ "json-fields": ["code"], "allow-prefixes": ["filesystem-"] })),
        );
        assert!(flagged.is_empty(), "prefixed families and placeholders are exempt: {flagged:?}");
    }
}

mod test_citations {
    use super::*;

    fn test_citations_hint(config: serde_json::Value) -> RuleHint {
        hint_with_config(HintKind::CliContract, "test-citations", Some(config))
    }

    const PREFIX: &str = "https://example.test/cli/blob/main/";

    #[test]
    fn known_citations_pass_unknown_flagged() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "Proven by `tests/plan/end_to_end.rs` and `tests/plan/end_to_emd.rs`.\n\n\
             See [the goldens](https://example.test/cli/blob/main/tests/fixtures/plan) and \
             [gone](https://example.test/cli/blob/main/tests/retired/gone.rs#L10).\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), test_citations_hint(json!({ "link-prefixes": [PREFIX] })));
        assert_eq!(
            flagged,
            vec!["tests/plan/end_to_emd.rs".to_string(), "tests/retired/gone.rs".to_string()],
            "the typo'd span and the retired link target are flagged; the exact file and \
             the directory-with-inventoried-children pass"
        );
    }

    #[test]
    fn linked_and_spanned_citation_deduplicates() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(
            tmp.path(),
            "doc.md",
            "[`tests/gone.rs`](https://example.test/cli/blob/main/tests/gone.rs)\n",
        );
        let flagged =
            flagged_tokens(tmp.path(), test_citations_hint(json!({ "link-prefixes": [PREFIX] })));
        assert_eq!(flagged, vec!["tests/gone.rs".to_string()], "one finding, not two");
    }

    #[test]
    fn ignore_exempts_foreign_test_layouts() {
        let tmp = tempfile::tempdir().expect("tmp");
        // A generated downstream crate's test layout is not a CLI test.
        write_doc(tmp.path(), "doc.md", "The crate writer emits `tests/provider.rs`.\n");
        let flagged = flagged_tokens(
            tmp.path(),
            test_citations_hint(json!({
                "link-prefixes": [PREFIX],
                "ignore": ["tests/provider.rs"],
            })),
        );
        assert!(flagged.is_empty(), "ignored cited paths are exempt: {flagged:?}");
    }

    #[test]
    fn empty_inventory_disables_selector() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_doc(tmp.path(), "doc.md", "Proven by `tests/anything.rs`.\n");
        let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
        let rule =
            make_rule("UNI-963", vec![test_citations_hint(json!({ "link-prefixes": [PREFIX] }))]);
        let runner: &dyn ToolRunner = &NoToolRunner;
        let mut cli_contract = contract();
        cli_contract.tests.clear();
        let env = EvalEnv {
            model: &model,
            project_dir: tmp.path(),
            tool_runner: runner,
            cli_contract: Some(&cli_contract),
        };
        let outcome = evaluate_env(&rule, rule.rule_hints.as_deref().unwrap_or_default(), env, 1)
            .expect("evaluate");
        assert!(
            outcome.findings.is_empty(),
            "a contract without a tests inventory checks nothing: {:?}",
            outcome.findings
        );
    }
}

#[test]
fn missing_contract_is_unsupported() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_doc(tmp.path(), "doc.md", "```bash\nspecify plan next\n```\n");
    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "UNI-964",
        vec![hint_with_config(
            HintKind::CliContract,
            "invocations",
            Some(json!({ "langs": ["bash"] })),
        )],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    // The `evaluate` convenience wrapper injects no contract.
    let err = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect_err("cli-contract without an injected contract is unsupported");
    assert!(matches!(err, HintError::Unsupported { .. }), "got: {err:?}");
}
