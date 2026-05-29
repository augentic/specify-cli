//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and workflow §Requirement block contract validation of
//! `spec.md` provenance metadata.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value as JsonValue;
use specify_domain::change::{Plan, orphan_authority_override_keys};
use specify_domain::design_system::{ComponentStatus, ComponentsCatalog};
use specify_domain::discovery::Discovery;
use specify_domain::journal::{Event, EventKind, append_batch};
use specify_domain::schema::{evidence_yaml_paths, validate_evidence_dir};
use specify_domain::slice::reconciliation::{self, ReconciliationIndex};
use specify_domain::spec::provenance::{self, ParsedSpec, RequirementTag};
use specify_domain::validate::validate_slice;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

use crate::runtime::context::Ctx;

pub(super) fn run(ctx: &Ctx, name: &str) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(name);
    // Per workflow §Source adapter contract, any Evidence file under
    // `<slice>/evidence/` must satisfy `schemas/evidence.schema.json`.
    // Failure here short-circuits the rule-based validator below so
    // the operator sees the structural problem first, before adapter
    // rules start complaining about downstream artefacts derived from
    // malformed Evidence.
    validate_evidence_dir(&slice_dir)?;

    // Single walk of `<slice>/specs/**/*.md` feeds provenance
    // validation, reconciliation-drift REQ-id gathering, and post-pass
    // synthesis journal emission.
    let source_keys = resolve_slice_source_keys(ctx, name)?;
    let (spec_req_ids, synthesis_tags, provenance_summaries) =
        scan_slice_specs(&slice_dir, &source_keys)?;
    if !provenance_summaries.is_empty() {
        return Err(Error::Validation {
            results: provenance_summaries,
        });
    }

    // `reconciliation.yaml` audit index — when `reconciliation.yaml` exists, cross-check it against
    // `spec.md` REQ ids and per-source evidence claim ids. Absence
    // of `reconciliation.yaml` is *not* drift: older slices and pre-refine
    // slices skip the check silently. The slice-reconciliation-drift error
    // body bundles every finding so the operator sees the full
    // re-refine surface in one pass.
    //
    // per-slice authority override — refuse a per-slice `authority-override` map that
    // names a source key absent from the slice's own `sources[]`
    // list. Runs before `validate_slice` so the operator sees the
    // structural issue before adapter rules surface downstream
    // breakage. Both checks share an error envelope when they
    // both fire so the operator can see every issue in one pass.
    validate_pre_adapter_gates(ctx, &slice_dir, name, &spec_req_ids)?;

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
        // DECISIONS.md — `slice.synthesis.{conflict,divergence,unknown}`
        // emit once per tagged requirement after a successful validate
        // (same posture as `slice.transition.refined` on transition).
        append_synthesis_journal(ctx, name, synthesis_tags)?;
        Ok(())
    } else {
        Err(Error::validation_failed(
            "slice-validation-failed",
            "slice must satisfy adapter validation",
            format!("slice `{name}` failed validation"),
        ))
    }
}

/// Append one `slice.synthesis.*` journal line per `(requirement-id,
/// tag)` pair gathered during the spec scan. Skipped when the slice
/// has no tagged requirements.
fn append_synthesis_journal(
    ctx: &Ctx, slice_name: &str, tags: Vec<(String, RequirementTag)>,
) -> Result<()> {
    if tags.is_empty() {
        return Ok(());
    }
    let now = Timestamp::now();
    let events: Vec<Event> = tags
        .into_iter()
        .map(|(requirement_id, tag)| {
            let kind = match tag {
                RequirementTag::Unknown => EventKind::SliceSynthesisUnknown {
                    slice_name: slice_name.to_string(),
                    requirement_id,
                },
                RequirementTag::Conflict => EventKind::SliceSynthesisConflict {
                    slice_name: slice_name.to_string(),
                    requirement_id,
                },
                RequirementTag::Divergence => EventKind::SliceSynthesisDivergence {
                    slice_name: slice_name.to_string(),
                    requirement_id,
                },
            };
            Event::new(now, kind)
        })
        .collect();
    append_batch(ctx.layout(), &events)
}

/// One parsed `spec.md` from the slice specs walk.
struct ScannedSpec {
    path: PathBuf,
    parsed: ParsedSpec,
}

type ScanSliceSpecsResult =
    (BTreeSet<String>, Vec<(String, RequirementTag)>, Vec<ValidationSummary>);

/// Walk `<slice>/specs/**/*.md` once, parse each file, and fan out
/// REQ ids (all files), synthesis tags (annotated files only), and
/// provenance validation summaries (annotated files only).
fn scan_slice_specs(
    slice_dir: &Path, source_keys: &BTreeSet<String>,
) -> Result<ScanSliceSpecsResult> {
    let specs_dir = slice_dir.join("specs");
    if !specs_dir.is_dir() {
        return Ok((BTreeSet::new(), Vec::new(), Vec::new()));
    }
    let spec_files = collect_spec_files(&specs_dir)?;
    if spec_files.is_empty() {
        return Ok((BTreeSet::new(), Vec::new(), Vec::new()));
    }

    let mut req_ids = BTreeSet::new();
    let mut synthesis_tags = Vec::new();
    let mut provenance_summaries = Vec::new();

    for path in spec_files {
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let scanned = ScannedSpec {
            path,
            parsed: provenance::parse_spec_md(&text),
        };

        for req in &scanned.parsed.requirements {
            if !req.id.is_empty() {
                req_ids.insert(req.id.clone());
            }
        }
        if scanned.parsed.is_unannotated() {
            continue;
        }
        for (id, tag) in scanned.parsed.synthesis_tags() {
            synthesis_tags.push((id.to_string(), tag));
        }
        let path_hint = path_hint(&scanned.path, slice_dir);
        let validation_findings = provenance::validate(&scanned.parsed, source_keys);
        for f in scanned.parsed.findings.into_iter().chain(validation_findings) {
            provenance_summaries.push(f.into_summary(&path_hint));
        }
    }

    Ok((req_ids, synthesis_tags, provenance_summaries))
}

/// Bundle the four pre-adapter gates that fire on a single slice:
///
/// 1. `reconciliation.yaml` audit index — reconciliation-drift detection between `spec.md`, the
///    per-slice `reconciliation.yaml`, and per-source `evidence/<key>.yaml`.
/// 2. per-slice authority override — orphan source keys on the slice's
///    `plan.yaml.slices[].authority-override` map.
/// 3. discovery alias contract — candidate `id` ↔ `aliases[]` collisions in
///    `<project_dir>/discovery.md`. A discovery-level check (not
///    per-slice) but evaluated here because `specrun slice validate`
///    is the single CLI surface skills shell out to between
///    `/spec:refine` and `/spec:build`.
/// 4. component catalog contract — catalog drift between Evidence `component:`
///    directives and `.specify/design-system/components.yaml`.
///
/// All four checks can fail independently; we collect every finding
/// into a single [`Error::Validation`] so the operator sees the
/// full surface in one pass instead of one error per re-run.
fn validate_pre_adapter_gates(
    ctx: &Ctx, slice_dir: &Path, name: &str, spec_req_ids: &BTreeSet<String>,
) -> Result<()> {
    let mut findings: Vec<ValidationSummary> = Vec::new();
    findings.extend(collect_reconciliation_drift_findings(slice_dir, spec_req_ids)?);
    findings.extend(override_orphans(ctx, name)?);
    findings.extend(alias_collisions(ctx)?);
    findings.extend(collect_catalog_drift_findings(ctx, slice_dir)?);
    if findings.is_empty() { Ok(()) } else { Err(Error::Validation { results: findings }) }
}

/// discovery alias contract alias-collision gate. Loads
/// `<project_dir>/discovery.md` when present and emits one
/// `discovery-alias-collision` finding per name that resolves to
/// more than one candidate. Absent `discovery.md` skips the check
/// silently — older slices and projects without an authored
/// inventory remain valid (this is the read-only counterpart to
/// the per-amend gate in `specrun plan amend --add-alias`).
fn alias_collisions(ctx: &Ctx) -> Result<Vec<ValidationSummary>> {
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

/// `reconciliation.yaml` audit index drift gate. Loads `<slice>/reconciliation.yaml` when present,
/// gathers `REQ-*` ids from every `<slice>/specs/**/*.md` file and
/// claim ids from every `<slice>/evidence/<source>.yaml` file, and
/// emits one `slice-reconciliation-drift` finding per drift entry. The
/// reconciliation file's own schema validation runs first
/// ([`ReconciliationIndex::load`]) so structural errors surface as
/// `reconciliation-schema` failures rather than bare drift noise.
///
/// Absence of `reconciliation.yaml` is a legal state — older slices and
/// `refining` slices that haven't yet been driven through
/// `/spec:refine` skip the check silently.
fn collect_reconciliation_drift_findings(
    slice_dir: &Path, spec_req_ids: &BTreeSet<String>,
) -> Result<Vec<ValidationSummary>> {
    let index_path = slice_dir.join("reconciliation.yaml");
    if !index_path.is_file() {
        return Ok(Vec::new());
    }
    let reconciliation_index = ReconciliationIndex::load(&index_path)?;
    let evidence = reconciliation::collect_evidence_claim_ids(slice_dir)?;
    let drift = reconciliation_index.detect_drift(spec_req_ids, &evidence);
    Ok(drift.into_iter().map(reconciliation::ReconciliationDrift::into_summary).collect())
}

/// per-slice authority override orphan-source-key gate. Loads `plan.yaml` (when
/// present) and reports one finding per `(slice, kind)` pair
/// whose source-key value is not in the slice's own `sources[]`
/// list. Absent `plan.yaml` (e.g. ad-hoc slice without a plan)
/// skips the check silently; the structural issue would already
/// have surfaced earlier in workflow.
fn override_orphans(ctx: &Ctx, name: &str) -> Result<Vec<ValidationSummary>> {
    let plan_path = ctx.layout().plan_path();
    if !plan_path.exists() {
        return Ok(Vec::new());
    }
    let plan = Plan::load(&plan_path)?;
    // Filter to the named slice only — `specrun slice validate` is
    // per-slice by definition, and surfacing findings from other
    // slices would confuse the operator.
    let slice_entries: Vec<_> = plan.entries.iter().filter(|e| e.name == name).cloned().collect();
    let findings = orphan_authority_override_keys(&slice_entries);
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

/// component catalog contract catalog-drift gate. Loads the project-level component
/// catalog (`.specify/design-system/components.yaml`) when present
/// and cross-references every `component: <slug>` directive on
/// Evidence claims in `<slice>/evidence/*.yaml` against it:
///
/// - Slug absent from catalog → `slice-catalog-drift` finding.
/// - Slug has `status: rejected` → `slice-catalog-drift` finding.
///
/// Claims carrying a slug in `notes.candidate_component` are exempt
/// when the catalog entry is `rejected` — the operator has
/// intentionally declined the promotion, and the note is purely
/// informational.
///
/// When no catalog exists the check returns empty — the catalog is
/// opt-in.
fn collect_catalog_drift_findings(ctx: &Ctx, slice_dir: &Path) -> Result<Vec<ValidationSummary>> {
    let Some(catalog) = ComponentsCatalog::load(&ctx.project_dir)? else {
        return Ok(Vec::new());
    };

    let paths = evidence_yaml_paths(slice_dir)?;
    let mut findings: Vec<ValidationSummary> = Vec::new();

    for path in &paths {
        let raw = std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let doc: JsonValue = serde_saphyr::from_str(&raw)?;
        let source_key =
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("<unknown>").to_string();

        let Some(claims) = doc.get("claims").and_then(JsonValue::as_array) else {
            continue;
        };

        for claim in claims {
            if let Some(slug) = claim.get("component").and_then(JsonValue::as_str) {
                match catalog.status_of(slug) {
                    None => {
                        findings.push(catalog_drift_summary(&format!(
                            "evidence/{source_key}.yaml: claim carries `component: {slug}` \
                             but no entry exists in the component catalog"
                        )));
                    }
                    Some(ComponentStatus::Rejected) => {
                        findings.push(catalog_drift_summary(&format!(
                            "evidence/{source_key}.yaml: claim carries `component: {slug}` \
                             but the catalog entry has `status: rejected`"
                        )));
                    }
                    Some(ComponentStatus::Confirmed) => {}
                }
            }

            // `notes.candidate_component` is purely informational
            // (a hint from the adapter's stage-6 detection). It never
            // triggers `slice-catalog-drift` regardless of whether
            // the slug is in the catalog, absent, or rejected. Only
            // hard `component:` directives above are checked.
        }
    }

    findings.sort_by(|a, b| a.detail.cmp(&b.detail));
    Ok(findings)
}

fn catalog_drift_summary(detail: &str) -> ValidationSummary {
    ValidationSummary {
        status: ValidationStatus::Fail,
        rule_id: "slice-catalog-drift".into(),
        rule: "Evidence `component:` directives resolve to confirmed catalog entries".into(),
        detail: Some(detail.to_string()),
    }
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
