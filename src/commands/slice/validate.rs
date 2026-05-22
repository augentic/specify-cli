//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and RFC-25 §Requirement block contract validation of
//! `spec.md` provenance metadata.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use specify_domain::change::{Plan, authority_override_orphan_source_keys};
use specify_domain::discovery::Discovery;
use specify_domain::schema::validate_evidence_dir;
use specify_domain::slice::fusion::{self, FusionIndex};
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

    // RFC-27 §D4 — when `fusion.yaml` exists, cross-check it against
    // `spec.md` REQ ids and per-source evidence claim ids. Absence
    // of `fusion.yaml` is *not* drift: older slices and pre-refine
    // slices skip the check silently. The slice-fusion-drift error
    // body bundles every finding so the operator sees the full
    // re-refine surface in one pass.
    //
    // RFC-27 §D3 — refuse a per-slice `authority-override` map that
    // names a source key absent from the slice's own `sources[]`
    // list. Runs before `validate_slice` so the operator sees the
    // structural issue before adapter rules surface downstream
    // breakage. Both checks share an error envelope when they
    // both fire so the operator can see every issue in one pass.
    validate_pre_adapter_gates(ctx, &slice_dir, name)?;

    let report = validate_slice(&slice_dir)?;
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

/// Bundle the three pre-adapter gates that fire on a single slice:
///
/// 1. RFC-27 §D4 — fusion-drift detection between `spec.md`, the
///    per-slice `fusion.yaml`, and per-source `evidence/<key>.yaml`.
/// 2. RFC-27 §D3 — orphan source keys on the slice's
///    `plan.yaml.slices[].authority-override` map.
/// 3. RFC-27 §D6 — candidate `id` ↔ `aliases[]` collisions in
///    `<project_dir>/discovery.md`. A discovery-level check (not
///    per-slice) but evaluated here because `specify slice validate`
///    is the single CLI surface skills shell out to between
///    `/spec:refine` and `/spec:build`.
///
/// All three checks can fail independently; we collect every finding
/// into a single [`Error::Validation`] so the operator sees the
/// full surface in one pass instead of one error per re-run.
fn validate_pre_adapter_gates(ctx: &Ctx, slice_dir: &Path, name: &str) -> Result<()> {
    let mut findings: Vec<ValidationSummary> = Vec::new();
    findings.extend(collect_fusion_drift_findings(slice_dir)?);
    findings.extend(collect_authority_override_orphan_findings(ctx, name)?);
    findings.extend(collect_discovery_alias_collision_findings(ctx)?);
    if findings.is_empty() { Ok(()) } else { Err(Error::Validation { results: findings }) }
}

/// RFC-27 §D6 alias-collision gate. Loads
/// `<project_dir>/discovery.md` when present and emits one
/// `discovery-alias-collision` finding per name that resolves to
/// more than one candidate. Absent `discovery.md` skips the check
/// silently — older slices and projects without an authored
/// inventory remain valid (this is the read-only counterpart to
/// the per-amend gate in `specify plan amend --add-alias`).
fn collect_discovery_alias_collision_findings(ctx: &Ctx) -> Result<Vec<ValidationSummary>> {
    let path = ctx.layout().discovery_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let discovery = Discovery::load(&path)?;
    Ok(discovery
        .check_alias_collisions()
        .iter()
        .map(specify_domain::discovery::DiscoveryAliasCollision::to_summary)
        .collect())
}

/// RFC-27 §D4 drift gate. Loads `<slice>/fusion.yaml` when present,
/// gathers `REQ-*` ids from every `<slice>/specs/**/*.md` file and
/// claim ids from every `<slice>/evidence/<source>.yaml` file, and
/// emits one `slice-fusion-drift` finding per drift entry. The
/// fusion file's own schema validation runs first
/// ([`FusionIndex::load`]) so structural errors surface as
/// `fusion-schema` failures rather than bare drift noise.
///
/// Absence of `fusion.yaml` is a legal state — older slices and
/// `refining` slices that haven't yet been driven through
/// `/spec:refine` skip the check silently.
fn collect_fusion_drift_findings(slice_dir: &Path) -> Result<Vec<ValidationSummary>> {
    let fusion_path = fusion::fusion_path(slice_dir);
    if !fusion_path.is_file() {
        return Ok(Vec::new());
    }
    let fusion_index = FusionIndex::load(&fusion_path)?;
    let spec_req_ids = collect_spec_req_ids(slice_dir)?;
    let evidence = fusion::collect_evidence_claim_ids(slice_dir)?;
    let drift = fusion_index.detect_drift(&spec_req_ids, &evidence);
    Ok(drift.into_iter().map(fusion::FusionDrift::into_summary).collect())
}

/// RFC-27 §D3 orphan-source-key gate. Loads `plan.yaml` (when
/// present) and reports one finding per `(slice, kind)` pair
/// whose source-key value is not in the slice's own `sources[]`
/// list. Absent `plan.yaml` (e.g. ad-hoc slice without a plan)
/// skips the check silently; the structural issue would already
/// have surfaced earlier in workflow.
fn collect_authority_override_orphan_findings(
    ctx: &Ctx, name: &str,
) -> Result<Vec<ValidationSummary>> {
    let plan_path = ctx.layout().plan_path();
    if !plan_path.exists() {
        return Ok(Vec::new());
    }
    let plan = Plan::load(&plan_path)?;
    // Filter to the named slice only — `specify slice validate` is
    // per-slice by definition, and surfacing findings from other
    // slices would confuse the operator.
    let slice_entries: Vec<_> = plan.entries.iter().filter(|e| e.name == name).cloned().collect();
    let findings = authority_override_orphan_source_keys(&slice_entries);
    Ok(findings
        .into_iter()
        .map(|f| ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: f.code.to_string(),
            rule: "Per-slice `authority-override` source key must appear in the slice's \
                   `sources[]` list"
                .to_string(),
            detail: Some(f.message),
        })
        .collect())
}

/// Gather every `REQ-NNN` id that appears in any `*.md` file under
/// `<slice>/specs/`. Multiple spec files contribute to one fusion
/// index per slice, so the drift gate joins all of them.
fn collect_spec_req_ids(slice_dir: &Path) -> Result<BTreeSet<String>> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let specs_dir = slice_dir.join("specs");
    if !specs_dir.is_dir() {
        return Ok(ids);
    }
    for path in collect_spec_files(&specs_dir)? {
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let parsed = provenance::parse_spec_md(&text);
        for req in parsed.requirements {
            if !req.id.is_empty() {
                ids.insert(req.id);
            }
        }
    }
    Ok(ids)
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
