#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to these command handlers."
)]

use serde::Serialize;
use specify::{
    CodexApplicability, CodexDeprecation, CodexDeterministicHint, CodexHintKind, CodexProvenance,
    CodexReference, CodexReviewMode, CodexSeverity, Error, ResolvedCodex, ResolvedCodexRule,
    ValidationSummary,
};

use crate::cli::{CodexAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, absolute_string, emit_response};

/// Dispatch `specify codex *`.
pub fn run(ctx: &CommandContext, action: CodexAction) -> Result<CliResult, Error> {
    match action {
        CodexAction::List => list(ctx),
        CodexAction::Show { rule_id } => show(ctx, &rule_id),
        CodexAction::Validate => validate(ctx),
        CodexAction::Export => export(ctx),
    }
}

fn resolve(ctx: &CommandContext) -> Result<ResolvedCodex, Error> {
    ResolvedCodex::resolve(&ctx.project_dir, ctx.config.capability.as_deref(), ctx.config.hub)
}

fn list(ctx: &CommandContext) -> Result<CliResult, Error> {
    let codex = resolve(ctx)?;
    match ctx.format {
        OutputFormat::Json => {
            let rules: Vec<_> = codex.rules.iter().map(RuleSummary::from_resolved).collect();
            emit_response(ListBody {
                rule_count: rules.len(),
                rules,
            })?;
        }
        OutputFormat::Text => print_list_text(&codex),
    }
    Ok(CliResult::Success)
}

fn show(ctx: &CommandContext, rule_id: &str) -> Result<CliResult, Error> {
    let codex = resolve(ctx)?;
    let normalized = rule_id.to_ascii_uppercase();
    let resolved = codex
        .rules
        .iter()
        .find(|candidate| candidate.rule.normalized_id == normalized)
        .ok_or_else(|| {
            Error::Config(format!("codex-rule-not-found: rule `{rule_id}` not found"))
        })?;

    match ctx.format {
        OutputFormat::Json => emit_response(ShowBody {
            rule: RuleExport::from_resolved(resolved),
        })?,
        OutputFormat::Text => print_show_text(resolved),
    }
    Ok(CliResult::Success)
}

fn validate(ctx: &CommandContext) -> Result<CliResult, Error> {
    match resolve(ctx) {
        Ok(codex) => {
            emit_validate_ok(ctx.format, codex.rules.len())?;
            Ok(CliResult::Success)
        }
        Err(Error::Validation { count, results }) => {
            emit_validate_fail(ctx.format, count, &results)?;
            Ok(CliResult::ValidationFailed)
        }
        Err(err) => Err(err),
    }
}

fn export(ctx: &CommandContext) -> Result<CliResult, Error> {
    let codex = resolve(ctx)?;
    match ctx.format {
        OutputFormat::Json => {
            let rules: Vec<_> = codex.rules.iter().map(RuleExport::from_resolved).collect();
            emit_response(ExportBody {
                rule_count: rules.len(),
                rules,
            })?;
        }
        OutputFormat::Text => {
            println!(
                "Codex export is a JSON contract; rerun with `specify codex export --format json`."
            );
        }
    }
    Ok(CliResult::Success)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ListBody<'a> {
    rule_count: usize,
    rules: Vec<RuleSummary<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody<'a> {
    rule: RuleExport<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExportBody<'a> {
    rule_count: usize,
    rules: Vec<RuleExport<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidateBody<'a> {
    ok: bool,
    rule_count: Option<usize>,
    error_count: usize,
    results: Vec<ValidationRow<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ValidationRow<'a> {
    status: String,
    rule_id: &'a str,
    rule: &'a str,
    detail: Option<&'a str>,
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
    applicability: Option<&'a CodexApplicability>,
    review_mode: Option<&'static str>,
    deterministic_hints: Vec<HintExport<'a>>,
    references: &'a [CodexReference],
    deprecated: Option<DeprecationExport<'a>>,
    body: &'a str,
    source_path: String,
    provenance_kind: &'static str,
    capability_name: Option<&'a str>,
    capability_version: Option<u32>,
    catalog_name: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct HintExport<'a> {
    kind: &'static str,
    value: &'a str,
    description: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DeprecationExport<'a> {
    reason: &'a str,
    replaced_by: Option<&'a str>,
}

impl<'a> RuleSummary<'a> {
    fn from_resolved(resolved: &'a ResolvedCodexRule) -> Self {
        let provenance = provenance_fields(&resolved.provenance);
        Self {
            id: &resolved.rule.frontmatter.id,
            title: &resolved.rule.frontmatter.title,
            severity: severity_label(resolved.rule.frontmatter.severity),
            source_path: absolute_string(&resolved.rule.path),
            provenance_kind: provenance.kind,
            capability_name: provenance.capability_name,
            capability_version: provenance.capability_version,
            catalog_name: provenance.catalog_name,
        }
    }
}

impl<'a> RuleExport<'a> {
    fn from_resolved(resolved: &'a ResolvedCodexRule) -> Self {
        let rule = &resolved.rule;
        let frontmatter = &rule.frontmatter;
        let provenance = provenance_fields(&resolved.provenance);
        Self {
            id: &frontmatter.id,
            title: &frontmatter.title,
            severity: severity_label(frontmatter.severity),
            trigger: &frontmatter.trigger,
            applicability: frontmatter.applicability.as_ref(),
            review_mode: frontmatter.review_mode.map(review_mode_label),
            deterministic_hints: frontmatter
                .deterministic_hints
                .iter()
                .map(HintExport::from_hint)
                .collect(),
            references: &frontmatter.references,
            deprecated: frontmatter.deprecated.as_ref().map(DeprecationExport::from_deprecation),
            body: &rule.body,
            source_path: absolute_string(&rule.path),
            provenance_kind: provenance.kind,
            capability_name: provenance.capability_name,
            capability_version: provenance.capability_version,
            catalog_name: provenance.catalog_name,
        }
    }
}

impl<'a> DeprecationExport<'a> {
    fn from_deprecation(deprecation: &'a CodexDeprecation) -> Self {
        Self {
            reason: &deprecation.reason,
            replaced_by: deprecation.replaced_by.as_deref(),
        }
    }
}

impl<'a> HintExport<'a> {
    fn from_hint(hint: &'a CodexDeterministicHint) -> Self {
        Self {
            kind: hint_kind_label(hint.kind),
            value: &hint.value,
            description: hint.description.as_deref(),
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

fn emit_validate_ok(format: OutputFormat, rule_count: usize) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(ValidateBody {
            ok: true,
            rule_count: Some(rule_count),
            error_count: 0,
            results: Vec::new(),
        })?,
        OutputFormat::Text => println!("Codex OK: {rule_count} rule(s)"),
    }
    Ok(())
}

fn emit_validate_fail(
    format: OutputFormat, error_count: usize, results: &[ValidationSummary],
) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(ValidateBody {
            ok: false,
            rule_count: None,
            error_count,
            results: results.iter().map(validation_row).collect(),
        })?,
        OutputFormat::Text => {
            println!("Codex invalid: {error_count} error(s)");
            for result in results {
                let detail = result.detail.as_deref().unwrap_or(&result.rule);
                println!("  [fail] {}: {detail}", result.rule_id);
            }
        }
    }
    Ok(())
}

fn validation_row(summary: &ValidationSummary) -> ValidationRow<'_> {
    ValidationRow {
        status: summary.status.to_string(),
        rule_id: &summary.rule_id,
        rule: &summary.rule,
        detail: summary.detail.as_deref(),
    }
}

fn print_list_text(codex: &ResolvedCodex) {
    for resolved in &codex.rules {
        let rule = &resolved.rule.frontmatter;
        println!(
            "{}\t{}\t{}\t{}",
            rule.id,
            severity_label(rule.severity),
            resolved.provenance,
            rule.title
        );
    }
}

fn print_show_text(resolved: &ResolvedCodexRule) {
    let rule = &resolved.rule;
    let frontmatter = &rule.frontmatter;
    println!("id: {}", frontmatter.id);
    println!("title: {}", frontmatter.title);
    println!("severity: {}", severity_label(frontmatter.severity));
    println!("trigger: {}", frontmatter.trigger);
    if let Some(review_mode) = frontmatter.review_mode {
        println!("review-mode: {}", review_mode_label(review_mode));
    }
    println!("source: {}", rule.path.display());
    println!("provenance: {}", resolved.provenance);
    println!();
    print!("{}", rule.body);
}

const fn severity_label(severity: CodexSeverity) -> &'static str {
    match severity {
        CodexSeverity::Critical => "critical",
        CodexSeverity::Important => "important",
        CodexSeverity::Suggestion => "suggestion",
        CodexSeverity::Optional => "optional",
    }
}

const fn review_mode_label(mode: CodexReviewMode) -> &'static str {
    match mode {
        CodexReviewMode::Deterministic => "deterministic",
        CodexReviewMode::ModelAssisted => "model-assisted",
        CodexReviewMode::Hybrid => "hybrid",
    }
}

const fn hint_kind_label(kind: CodexHintKind) -> &'static str {
    match kind {
        CodexHintKind::PathPattern => "path-pattern",
        CodexHintKind::Regex => "regex",
        CodexHintKind::Schema => "schema",
        CodexHintKind::Tool => "tool",
    }
}
