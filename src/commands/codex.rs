pub mod cli;

use std::io::Write;

use serde::Serialize;
use specify_domain::capability::{
    CodexProvenance, CodexSeverity, ResolvedCodex, ResolvedCodexRule,
};
use specify_error::{Error, Result, ValidationSummary};

use self::cli::CodexAction;
use crate::context::Ctx;

/// Dispatch `specify codex *`.
pub fn run(ctx: &Ctx, action: CodexAction) -> Result<()> {
    match action {
        CodexAction::Export => export(ctx),
    }
}

fn resolve(ctx: &Ctx) -> Result<ResolvedCodex> {
    ResolvedCodex::resolve(&ctx.project_dir, ctx.config.capability.as_deref(), ctx.config.hub)
}

/// Resolve the codex and either emit the export body or — when
/// resolution surfaces validation failures — render the per-rule
/// findings on the standard envelope before propagating exit-code 2.
/// The structured payload preserves the actionability that the retired
/// `specify codex validate` verb used to provide so operators can still
/// see which rule misbehaved.
fn export(ctx: &Ctx) -> Result<()> {
    match resolve(ctx) {
        Ok(codex) => {
            let rules: Vec<_> = codex.rules.iter().map(|r| RuleView::build(r, true)).collect();
            ctx.write(
                &ExportBody {
                    rule_count: Some(rules.len()),
                    error_count: 0,
                    rules,
                    results: &[],
                },
                write_export_text,
            )?;
            Ok(())
        }
        Err(Error::Validation { results }) => {
            ctx.write(
                &ExportBody {
                    rule_count: None,
                    error_count: results.len(),
                    rules: Vec::new(),
                    results: &results,
                },
                write_export_text,
            )?;
            Err(Error::Validation { results })
        }
        Err(err) => Err(err),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExportBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_count: Option<usize>,
    error_count: usize,
    rules: Vec<RuleView<'a>>,
    results: &'a [ValidationSummary],
}

fn write_export_text(w: &mut dyn Write, body: &ExportBody<'_>) -> std::io::Result<()> {
    if body.error_count == 0 {
        return writeln!(
            w,
            "Codex export is a JSON contract; rerun with `specify codex export --format json`."
        );
    }
    writeln!(w, "Codex invalid: {} error(s)", body.error_count)?;
    for r in body.results {
        let detail = r.detail.as_deref().unwrap_or(&r.rule);
        writeln!(w, "  [fail] {}: {detail}", r.rule_id)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RuleView<'a> {
    id: &'a str,
    title: &'a str,
    severity: CodexSeverity,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    source_path: String,
    #[serde(flatten)]
    provenance: &'a CodexProvenance,
}

impl<'a> RuleView<'a> {
    fn build(resolved: &'a ResolvedCodexRule, with_body: bool) -> Self {
        let rule = &resolved.rule;
        let frontmatter = &rule.frontmatter;
        Self {
            id: &frontmatter.id,
            title: &frontmatter.title,
            severity: frontmatter.severity,
            trigger: with_body.then_some(frontmatter.trigger.as_str()),
            body: with_body.then_some(rule.body.as_str()),
            source_path: rule.path.display().to_string(),
            provenance: &resolved.provenance,
        }
    }
}
