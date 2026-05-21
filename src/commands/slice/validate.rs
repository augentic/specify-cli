//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and RFC-25 §Requirement block contract validation of
//! `spec.md` provenance metadata.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use specify_domain::change::Plan;
use specify_domain::schema::validate_evidence_dir;
use specify_domain::spec::provenance;
use specify_domain::validate::validate_slice;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

use crate::context::Ctx;

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    // Per RFC-25 §Source adapter contract, any Evidence file under
    // `<slice>/evidence/` must satisfy `schemas/evidence.schema.json`.
    // Failure here short-circuits the rule-based validator below so
    // the operator sees the structural problem first, before adapter
    // rules start complaining about downstream artefacts derived from
    // malformed Evidence.
    validate_evidence_dir(&slice_dir)?;

    // RFC-25 §Requirement block contract — provenance metadata in
    // `<slice>/specs/**/*.md`. Absent `spec.md` (e.g. slice still
    // `refining`) is a valid intermediate state and is silently
    // skipped.
    validate_spec_provenance(ctx, &slice_dir, name)?;

    let pipeline = ctx.load_pipeline()?;
    let report = validate_slice(&slice_dir, &pipeline)?;
    let passed = report.passed;

    ctx.write(&report, |w, _| {
        writeln!(w, "{}", if report.passed { "PASS" } else { "FAIL" })?;
        for (key, results) in &report.brief_results {
            writeln!(w, "{key}:")?;
            for r in results {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        if !report.cross_checks.is_empty() {
            writeln!(w, "cross_checks:")?;
            for r in &report.cross_checks {
                writeln!(w, "  {}", format_result_line(r))?;
            }
        }
        Ok(())
    })?;
    if passed {
        Ok(())
    } else {
        Err(Error::validation_failed(
            "slice-validation-failed",
            "slice must satisfy adapter validation",
            format!("slice `{name}` failed validation"),
        ))
    }
}

/// Walk `<slice>/specs/**/*.md`, parse the RFC-25 §Requirement block
/// contract metadata, and surface every failure as a single
/// [`Error::Validation`] payload so the operator can see all of them in
/// one pass. The plan-level source bindings for the slice (when
/// `plan.yaml` is present) feed the cross-validation rule
/// `spec.requirement-source-key-undefined`; absent plan, the
/// cross-validation is skipped and only structural rules run.
fn validate_spec_provenance(ctx: &Ctx, slice_dir: &Path, name: &str) -> Result<()> {
    let specs_dir = slice_dir.join("specs");
    if !specs_dir.is_dir() {
        return Ok(());
    }
    let spec_files = collect_spec_files(&specs_dir)?;
    if spec_files.is_empty() {
        return Ok(());
    }
    let source_keys = resolve_slice_source_keys(ctx, name)?;

    let mut summaries: Vec<ValidationSummary> = Vec::new();
    for path in spec_files {
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let parsed = provenance::parse_spec_md(&text);
        let path_hint = path_hint(&path, slice_dir);
        if parsed.is_unannotated() {
            // Pre-RFC-25 (or pre-synthesis) state — no provenance
            // lines anywhere in the file. Skip silently per the
            // RFC-25 §Workflow vocabulary `refining` lifecycle.
            continue;
        }
        let validation_findings = provenance::validate(&parsed, &source_keys);
        for f in parsed.findings.into_iter().chain(validation_findings) {
            summaries.push(f.into_summary(&path_hint));
        }
    }

    if summaries.is_empty() { Ok(()) } else { Err(Error::Validation { results: summaries }) }
}

/// Path the operator sees in each finding's detail. Anchored at the
/// slice directory so the printed string is `specs/<group>/spec.md`
/// rather than an absolute tempdir path that varies per test run.
fn path_hint(path: &Path, slice_dir: &Path) -> String {
    let rel = path.strip_prefix(slice_dir).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Recursive walk of `<slice>/specs/` collecting every `*.md` file.
/// Hand-rolled (rather than reaching for `glob`) so the call site
/// stays auditable on the operator path. Sorted for stable error
/// ordering.
fn collect_spec_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| Error::Filesystem {
            op: "readdir",
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| Error::Filesystem {
                op: "readdir-entry",
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Resolve the plan-level source keys declared on the slice's plan
/// entry (`Plan.sources` is the universe of keys; the slice's
/// `sources[]` is the subset the slice actually binds). When no
/// `plan.yaml` exists, returns an empty set so the cross-validation
/// rule no-ops — structural rules still run.
fn resolve_slice_source_keys(ctx: &Ctx, name: &str) -> Result<BTreeSet<String>> {
    let plan_path = ctx.layout().plan_path();
    if !plan_path.exists() {
        return Ok(BTreeSet::new());
    }
    let plan = Plan::load(&plan_path)?;
    let Some(entry) = plan.entries.iter().find(|e| e.name == name) else {
        // Plan exists but the slice is not in it (e.g. operator
        // hand-created the slice). Fall back to the universe of plan
        // sources so the operator still gets useful validation
        // against any key spelt correctly.
        return Ok(plan.sources.keys().cloned().collect());
    };
    Ok(entry.sources.iter().map(|b| b.key().to_string()).collect())
}

fn format_result_line(r: &ValidationSummary) -> String {
    match r.status {
        ValidationStatus::Pass => format!("[ok] {}", r.rule_id),
        ValidationStatus::Fail => {
            format!("[fail] {}: {}", r.rule_id, r.detail.as_deref().unwrap_or(""))
        }
        ValidationStatus::Deferred => {
            format!("[defer] {} ({})", r.rule_id, r.detail.as_deref().unwrap_or(""))
        }
    }
}
