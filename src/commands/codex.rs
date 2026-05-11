pub(crate) mod cli;

use std::io::Write;

use serde::Serialize;
use specify_domain::capability::{CodexProvenance, CodexSeverity, ResolvedCodex, ResolvedCodexRule};
use specify_error::{Error, Result};

use crate::cli::CodexAction;
use crate::context::Ctx;
use crate::output::{Render, Validation, ValidationRow, display};

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
    let rules: Vec<_> = codex.rules.iter().map(RuleSummary::from).collect();
    ctx.write(&ListBody {
        rule_count: rules.len(),
        rules,
    })?;
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

    ctx.write(&ShowBody {
        rule: RuleExport::from(resolved),
    })?;
    Ok(())
}

fn validate(ctx: &Ctx) -> Result<()> {
    match resolve(ctx) {
        Ok(codex) => {
            ctx.write(&ValidateBody {
                rule_count: Some(codex.rules.len()),
                error_count: 0,
                validation: Validation { results: Vec::new() },
            })?;
            Ok(())
        }
        Err(Error::Validation { results }) => {
            ctx.write(&ValidateBody {
                rule_count: None,
                error_count: results.len(),
                validation: Validation {
                    results: results.iter().map(ValidationRow::from).collect(),
                },
            })?;
            Err(Error::Validation { results })
        }
        Err(err) => Err(err),
    }
}

fn export(ctx: &Ctx) -> Result<()> {
    let codex = resolve(ctx)?;
    let rules: Vec<_> = codex.rules.iter().map(RuleExport::from).collect();
    ctx.write(&ExportBody {
        rule_count: rules.len(),
        rules,
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListBody<'a> {
    rule_count: usize,
    rules: Vec<RuleSummary<'a>>,
}

impl Render for ListBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for rule in &self.rules {
            writeln!(
                w,
                "{}\t{}\t{}\t{}",
                rule.id,
                rule.severity,
                provenance_text(rule),
                rule.title
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody<'a> {
    rule: RuleExport<'a>,
}

impl Render for ShowBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let r = &self.rule;
        writeln!(w, "id: {}", r.id)?;
        writeln!(w, "title: {}", r.title)?;
        writeln!(w, "severity: {}", r.severity)?;
        writeln!(w, "trigger: {}", r.trigger)?;
        writeln!(w, "source: {}", r.source_path)?;
        writeln!(w, "provenance: {}", export_provenance_text(r))?;
        writeln!(w)?;
        write!(w, "{}", r.body)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExportBody<'a> {
    rule_count: usize,
    rules: Vec<RuleExport<'a>>,
}

impl Render for ExportBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "Codex export is a JSON contract; rerun with `specify codex export --format json`."
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateBody<'a> {
    rule_count: Option<usize>,
    error_count: usize,
    #[serde(flatten)]
    validation: Validation<ValidationRow<'a>>,
}

impl Render for ValidateBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.error_count == 0 {
            return writeln!(w, "Codex OK: {} rule(s)", self.rule_count.unwrap_or(0));
        }
        writeln!(w, "Codex invalid: {} error(s)", self.error_count)?;
        for r in &self.validation.results {
            let detail = r.detail.unwrap_or(r.rule);
            writeln!(w, "  [fail] {}: {detail}", r.rule_id)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RuleSummary<'a> {
    id: &'a str,
    title: &'a str,
    severity: &'static str,
    source_path: String,
    provenance_kind: &'static str,
    capability_name: Option<&'a str>,
    capability_version: Option<u32>,
    catalog_name: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct RuleExport<'a> {
    id: &'a str,
    title: &'a str,
    severity: &'static str,
    trigger: &'a str,
    body: &'a str,
    source_path: String,
    provenance_kind: &'static str,
    capability_name: Option<&'a str>,
    capability_version: Option<u32>,
    catalog_name: Option<&'a str>,
}

impl<'a> From<&'a ResolvedCodexRule> for RuleSummary<'a> {
    fn from(resolved: &'a ResolvedCodexRule) -> Self {
        let provenance = provenance_fields(&resolved.provenance);
        Self {
            id: &resolved.rule.frontmatter.id,
            title: &resolved.rule.frontmatter.title,
            severity: severity_label(resolved.rule.frontmatter.severity),
            source_path: display(&resolved.rule.path),
            provenance_kind: provenance.kind,
            capability_name: provenance.capability_name,
            capability_version: provenance.capability_version,
            catalog_name: provenance.catalog_name,
        }
    }
}

impl<'a> From<&'a ResolvedCodexRule> for RuleExport<'a> {
    fn from(resolved: &'a ResolvedCodexRule) -> Self {
        let rule = &resolved.rule;
        let frontmatter = &rule.frontmatter;
        let provenance = provenance_fields(&resolved.provenance);
        Self {
            id: &frontmatter.id,
            title: &frontmatter.title,
            severity: severity_label(frontmatter.severity),
            trigger: &frontmatter.trigger,
            body: &rule.body,
            source_path: display(&rule.path),
            provenance_kind: provenance.kind,
            capability_name: provenance.capability_name,
            capability_version: provenance.capability_version,
            catalog_name: provenance.catalog_name,
        }
    }
}

struct ProvenanceFields<'a> {
    kind: &'static str,
    capability_name: Option<&'a str>,
    capability_version: Option<u32>,
    catalog_name: Option<&'a str>,
}

const fn provenance_fields(provenance: &CodexProvenance) -> ProvenanceFields<'_> {
    match provenance {
        CodexProvenance::Capability { name, version } => ProvenanceFields {
            kind: "capability",
            capability_name: Some(name.as_str()),
            capability_version: Some(*version),
            catalog_name: None,
        },
        CodexProvenance::Catalog { name } => ProvenanceFields {
            kind: "catalog",
            capability_name: None,
            capability_version: None,
            catalog_name: Some(name.as_str()),
        },
        CodexProvenance::Repo => ProvenanceFields {
            kind: "repo",
            capability_name: None,
            capability_version: None,
            catalog_name: None,
        },
    }
}

fn provenance_text(rule: &RuleSummary<'_>) -> String {
    match rule.provenance_kind {
        "capability" => format!(
            "capability {}@v{}",
            rule.capability_name.unwrap_or(""),
            rule.capability_version.unwrap_or(0)
        ),
        "catalog" => format!("catalog {}", rule.catalog_name.unwrap_or("")),
        _ => "repo".into(),
    }
}

fn export_provenance_text(rule: &RuleExport<'_>) -> String {
    match rule.provenance_kind {
        "capability" => format!(
            "capability {}@v{}",
            rule.capability_name.unwrap_or(""),
            rule.capability_version.unwrap_or(0)
        ),
        "catalog" => format!("catalog {}", rule.catalog_name.unwrap_or("")),
        _ => "repo".into(),
    }
}

const fn severity_label(severity: CodexSeverity) -> &'static str {
    match severity {
        CodexSeverity::Critical => "critical",
        CodexSeverity::Important => "important",
        CodexSeverity::Suggestion => "suggestion",
        CodexSeverity::Optional => "optional",
    }
}
