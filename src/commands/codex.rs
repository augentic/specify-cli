pub(crate) mod cli;

use std::io::Write;

use serde::Serialize;
use specify_domain::capability::{
    CodexProvenance, CodexSeverity, ResolvedCodex, ResolvedCodexRule,
};
use specify_error::{Error, Result, ValidationSummary};

use crate::cli::CodexAction;
use crate::context::Ctx;

/// Dispatch `specify codex *`.
pub(crate) fn run(ctx: &Ctx, action: CodexAction) -> Result<()> {
    match action {
        CodexAction::List => list(ctx),
        CodexAction::Show { rule_id } => show(ctx, &rule_id),
        CodexAction::Validate => validate(ctx),
        CodexAction::Export => export(ctx),
    }
}

fn resolve(ctx: &Ctx) -> Result<ResolvedCodex> {
    ResolvedCodex::resolve(&ctx.project_dir, ctx.config.capability.as_deref(), ctx.config.hub)
}

fn list(ctx: &Ctx) -> Result<()> {
    let codex = resolve(ctx)?;
    let rules: Vec<_> = codex.rules.iter().map(RuleView::summary).collect();
    ctx.write(
        &ListBody {
            rule_count: rules.len(),
            rules,
        },
        write_list_text,
    )?;
    Ok(())
}

fn show(ctx: &Ctx, rule_id: &str) -> Result<()> {
    let codex = resolve(ctx)?;
    let normalized = rule_id.to_ascii_uppercase();
    let resolved = codex
        .rules
        .iter()
        .find(|candidate| candidate.rule.normalized_id == normalized)
        .ok_or_else(|| Error::Diag {
            code: "codex-rule-not-found",
            detail: format!("rule `{rule_id}` not found"),
        })?;

    ctx.write(
        &ShowBody {
            rule: RuleView::full(resolved),
        },
        write_show_text,
    )?;
    Ok(())
}

fn validate(ctx: &Ctx) -> Result<()> {
    match resolve(ctx) {
        Ok(codex) => {
            ctx.write(
                &ValidateBody {
                    rule_count: Some(codex.rules.len()),
                    error_count: 0,
                    results: &[],
                },
                write_validate_text,
            )?;
            Ok(())
        }
        Err(Error::Validation { results }) => {
            ctx.write(
                &ValidateBody {
                    rule_count: None,
                    error_count: results.len(),
                    results: &results,
                },
                write_validate_text,
            )?;
            Err(Error::Validation { results })
        }
        Err(err) => Err(err),
    }
}

fn export(ctx: &Ctx) -> Result<()> {
    let codex = resolve(ctx)?;
    let rules: Vec<_> = codex.rules.iter().map(RuleView::full).collect();
    ctx.write(
        &ExportBody {
            rule_count: rules.len(),
            rules,
        },
        write_export_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListBody<'a> {
    rule_count: usize,
    rules: Vec<RuleView<'a>>,
}

fn write_list_text(w: &mut dyn Write, body: &ListBody<'_>) -> std::io::Result<()> {
    for rule in &body.rules {
        writeln!(w, "{}\t{}\t{}\t{}", rule.id, rule.severity, provenance_text(rule), rule.title)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody<'a> {
    rule: RuleView<'a>,
}

fn write_show_text(w: &mut dyn Write, body: &ShowBody<'_>) -> std::io::Result<()> {
    let r = &body.rule;
    writeln!(w, "id: {}", r.id)?;
    writeln!(w, "title: {}", r.title)?;
    writeln!(w, "severity: {}", r.severity)?;
    writeln!(w, "trigger: {}", r.trigger.unwrap_or_default())?;
    writeln!(w, "source: {}", r.source_path)?;
    writeln!(w, "provenance: {}", provenance_text(r))?;
    writeln!(w)?;
    write!(w, "{}", r.body.unwrap_or_default())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExportBody<'a> {
    rule_count: usize,
    rules: Vec<RuleView<'a>>,
}

fn write_export_text(w: &mut dyn Write, _body: &ExportBody<'_>) -> std::io::Result<()> {
    writeln!(w, "Codex export is a JSON contract; rerun with `specify codex export --format json`.")
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateBody<'a> {
    rule_count: Option<usize>,
    error_count: usize,
    results: &'a [ValidationSummary],
}

fn write_validate_text(w: &mut dyn Write, body: &ValidateBody<'_>) -> std::io::Result<()> {
    if body.error_count == 0 {
        return writeln!(w, "Codex OK: {} rule(s)", body.rule_count.unwrap_or(0));
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
    fn summary(resolved: &'a ResolvedCodexRule) -> Self {
        Self::build(resolved, false)
    }

    fn full(resolved: &'a ResolvedCodexRule) -> Self {
        Self::build(resolved, true)
    }

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

fn provenance_text(rule: &RuleView<'_>) -> String {
    rule.provenance.to_string()
}
