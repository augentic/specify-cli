//! Shared scaffolding for the hint-evaluator integration
//! tests.
//!
//! Each `tests/review_hint_*.rs` integration crate `mod`s this file
//! to share the [`FakeToolRunner`] / `make_rule` / `build_model`
//! plumbing. The scaffolding lives under `tests/<helper>/mod.rs`
//! per Rust's idiom; the host workspace forbids `mod.rs` everywhere
//! else (see `docs/standards/coding-standards.md` §"Module layout").

#![allow(dead_code, reason = "Some helpers are only consumed by a subset of test crates.")]

use std::path::Path;

use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner};
use specify_lints::rules::{
    Applicability, DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule, Severity,
};

pub fn make_rule(rule_id: &str, hints: Vec<DeterministicHint>) -> ResolvedRule {
    ResolvedRule {
        rule_id: rule_id.to_string(),
        title: format!("{rule_id} test rule"),
        severity: Severity::Important,
        trigger: format!("Trigger for {rule_id}"),
        lint_mode: None,
        applicability: None,
        deterministic_hints: if hints.is_empty() { None } else { Some(hints) },
        references: None,
        origin: Origin::Shared,
        path_root: PathRoot::RulesRoot,
        path: format!("shared/{rule_id}.md"),
        body: String::new(),
        deprecated: None,
    }
}

pub fn make_rule_with_adapter(
    rule_id: &str, adapter: &str, hints: Vec<DeterministicHint>,
) -> ResolvedRule {
    let mut rule = make_rule(rule_id, hints);
    rule.applicability = Some(Applicability {
        adapters: Some(vec![adapter.to_string()]),
        languages: None,
        artifacts: None,
        paths: None,
    });
    rule
}

pub fn hint(kind: HintKind, value: &str) -> DeterministicHint {
    DeterministicHint {
        kind,
        value: value.to_string(),
        description: None,
    }
}

pub struct NoToolRunner;

impl ToolRunner for NoToolRunner {
    fn run(
        &self, _tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        Err(ToolRunError::Runtime("no tool runner wired".to_string()))
    }

    fn is_declared(&self, _tool_name: &str) -> bool {
        false
    }
}

pub struct FakeToolRunner {
    pub declared: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

impl ToolRunner for FakeToolRunner {
    fn run(
        &self, _tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        Ok(ToolOutput {
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            exit_code: self.exit_code,
        })
    }

    fn is_declared(&self, _tool_name: &str) -> bool {
        self.declared
    }
}
