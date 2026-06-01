//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and workflow §Requirement block contract validation of
//! `spec.md` provenance metadata.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value as JsonValue;
use specify_diagnostics::{
    Artifact, Diagnostic, DiagnosticReport, DiagnosticReportVersion, DiagnosticSummary,
    blocking_present, renumber,
};
use specify_error::{Error, Result};
use specify_model::discovery::Discovery;
use specify_model::spec::provenance::{self, ParsedSpec, RequirementTag};
use specify_validate::validate_slice;
use specify_workflow::change::{Plan, orphan_authority_override_keys};
use specify_workflow::design_system::{ComponentStatus, ComponentsCatalog};
use specify_workflow::journal::{Event, EventKind, append_batch};
use specify_workflow::schema::{evidence_yaml_paths, validate_evidence_dir};

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
    // validation, provenance-drift REQ-id gathering, and post-pass
    // synthesis journal emission.
    let source_keys = resolve_slice_source_keys(ctx, name)?;
    let (_spec_req_ids, synthesis_tags, provenance_findings) =
        scan_slice_specs(&slice_dir, &source_keys)?;
    if !provenance_findings.is_empty() {
        return fail_with(ctx, "slice-provenance-invalid", provenance_findings);
    }

    // per-slice authority override — refuse a per-slice `authority-override` map that
    // names a source key absent from the slice's own `sources[]`
    // list. Runs before `validate_slice` so the operator sees the
    // structural issue before adapter rules surface downstream
    // breakage. Both checks share an error envelope when they
    // both fire so the operator can see every issue in one pass.
    let gate_findings = collect_pre_adapter_gates(ctx, &slice_dir, name)?;
    if !gate_findings.is_empty() {
        return fail_with(ctx, "slice-pre-adapter-gate", gate_findings);
    }

    // Adapter validation findings — `validate_slice` returns one
    // `violation` diagnostic per structural Fail and one `review`
    // diagnostic per deferred semantic rule. The report is rendered
    // on stdout either way; only a blocking diagnostic gates exit.
    //
    // The `discovery-lead-synopsis-thin` content-floor advisory
    // (RFC-29b-signal D2.1) rides this non-blocking surface — never
    // the pre-adapter gate above, which hard-fails on any finding —
    // so a thin `synopsis` nudges without ever parking the slice.
    let mut findings = validate_slice(&slice_dir)?;
    findings.extend(synopsis_thin(ctx)?);
    let blocking = blocking_present(&findings);
    render_report(ctx, findings)?;

    if blocking {
        Err(Error::validation_failed(
            "slice-validation-failed",
            "slice must satisfy adapter validation",
            format!("slice `{name}` failed validation"),
        ))
    } else {
        // DECISIONS.md — `slice.synthesis.{conflict,divergence,unknown}`
        // emit once per tagged requirement after a successful validate
        // (same posture as `slice.transition.refined` on transition).
        append_synthesis_journal(ctx, name, synthesis_tags)?;
        Ok(())
    }
}

/// Render `findings` as a [`DiagnosticReport`] on stdout in the active
/// `Ctx` format. JSON serialises the wire envelope; text renders a
/// PASS/FAIL banner plus one line per diagnostic. Ids are assigned
/// sequentially at render time.
fn render_report(ctx: &Ctx, mut findings: Vec<Diagnostic>) -> Result<()> {
    renumber(&mut findings);
    let blocking = blocking_present(&findings);
    let report = DiagnosticReport {
        version: DiagnosticReportVersion,
        summary: DiagnosticSummary::from_diagnostics(&findings),
        findings,
    };
    ctx.write(&report, move |w, report| {
        writeln!(w, "{}", if blocking { "FAIL" } else { "PASS" })?;
        for finding in &report.findings {
            writeln!(w, "  {}", format_finding_line(finding))?;
        }
        Ok(())
    })
}

/// Render `findings` on stdout and return the payload-free
/// [`Error::Validation`] keyed on `code`. Used by every pre-adapter
/// gate so the operator sees the full diagnostic surface before the
/// gate fails the command.
fn fail_with(ctx: &Ctx, code: &'static str, findings: Vec<Diagnostic>) -> Result<()> {
    let count = findings.len();
    render_report(ctx, findings)?;
    Err(Error::validation_failed(
        code,
        "slice must satisfy structural invariants",
        format!("{count} blocking finding(s)"),
    ))
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

type ScanSliceSpecsResult = (BTreeSet<String>, Vec<(String, RequirementTag)>, Vec<Diagnostic>);

/// Walk `<slice>/specs/**/*.md` once, parse each file, and fan out
/// REQ ids (all files), synthesis tags (annotated files only), and
/// provenance diagnostics (annotated files only).
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
    let mut provenance_findings = Vec::new();

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
            provenance_findings.push(f.into_diagnostic(&path_hint));
        }
    }

    Ok((req_ids, synthesis_tags, provenance_findings))
}

/// Bundle the three pre-adapter gates that fire on a single slice:
///
/// 1. per-slice authority override — orphan source keys on the slice's
///    `plan.yaml.slices[].authority-override` map.
/// 2. discovery alias contract — candidate `id` ↔ `aliases[]` collisions in
///    `<project_dir>/discovery.md`. A discovery-level check (not
///    per-slice) but evaluated here because `specrun slice validate`
///    is the single CLI surface skills shell out to between
///    `/spec:refine` and `/spec:build`.
/// 3. component catalog contract — catalog drift between Evidence `component:`
///    directives and `.specify/design-system/components.yaml`.
///
/// Provenance no longer has a file-drift gate: it is carried inline in
/// `model.yaml` and projected on demand (`specrun slice provenance`),
/// so there is no second representation to drift against. Spec-level
/// `Sources:` / `Status:` coherence still runs in [`scan_slice_specs`].
///
/// All three checks can fail independently; we collect every finding
/// into one [`Diagnostic`] vector so the caller can render the full
/// surface in one pass instead of one error per re-run.
fn collect_pre_adapter_gates(ctx: &Ctx, slice_dir: &Path, name: &str) -> Result<Vec<Diagnostic>> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    findings.extend(override_orphans(ctx, name)?);
    findings.extend(alias_collisions(ctx)?);
    findings.extend(collect_catalog_drift_findings(ctx, slice_dir)?);
    Ok(findings)
}

/// discovery alias contract alias-collision gate. Loads
/// `<project_dir>/discovery.md` when present and emits one
/// `discovery-alias-collision` finding per name that resolves to
/// more than one candidate. Absent `discovery.md` skips the check
/// silently — older slices and projects without an authored
/// inventory remain valid (this is the read-only counterpart to
/// the per-amend gate in `specrun plan amend --add-alias`).
fn alias_collisions(ctx: &Ctx) -> Result<Vec<Diagnostic>> {
    let path = ctx.layout().discovery_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let discovery = Discovery::load(&path)?;
    Ok(discovery
        .check_alias_collisions()
        .iter()
        .map(specify_model::discovery::DiscoveryAliasCollision::to_diagnostic)
        .collect())
}

/// Synopsis content-floor advisory (RFC-29b-signal D2.1). Loads
/// `<project_dir>/discovery.md` when present and emits one
/// non-blocking `discovery-lead-synopsis-thin` finding per lead whose
/// `synopsis` falls below a contentfulness heuristic. Absent
/// `discovery.md` skips the check silently.
///
/// **Non-blocking by design** — surfaced at `suggestion` severity
/// (`Diagnostic::review`), it never parks planning or transitions a
/// plan. A thin synopsis is a nudge to improve the source adapter's
/// `survey` brief output, not a gate: cross-source reconciliation is
/// only ever as good as the discriminating power of each lead's
/// `synopsis`, so the floor catches synopses the agent cannot match
/// or split on at `propose` time.
fn synopsis_thin(ctx: &Ctx) -> Result<Vec<Diagnostic>> {
    let path = ctx.layout().discovery_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let discovery = Discovery::load(&path)?;
    Ok(discovery
        .leads()
        .iter()
        .filter(|lead| synopsis_is_thin(&lead.synopsis))
        .map(|lead| {
            Diagnostic::review(
                "discovery-lead-synopsis-thin",
                "lead synopses should name behaviour distinctly enough to match or split on \
                 content, not just the slug",
                format!(
                    "lead `{}:{}` has a thin synopsis (`{}`); name the operation/surface and its \
                     salient constraint so a same-slug lead from another source can be \
                     reconciled on content",
                    lead.source,
                    lead.lead,
                    lead.synopsis.trim()
                ),
                Artifact::Plan,
                None,
            )
        })
        .collect())
}

/// Contentfulness heuristic for a lead `synopsis` (RFC-29b-signal
/// D2.1). A synopsis is "thin" when it carries fewer than
/// [`SYNOPSIS_MIN_WORDS`] whitespace-delimited words OR fewer than
/// [`SYNOPSIS_MIN_CHARS`] non-whitespace characters once trimmed — too
/// little for the agent to distinguish it from a same-slug lead in
/// another source. Deliberately coarse: the finding is advisory, so a
/// false positive costs the operator nothing.
fn synopsis_is_thin(synopsis: &str) -> bool {
    let trimmed = synopsis.trim();
    let words = trimmed.split_whitespace().filter(|word| !word.is_empty()).count();
    let chars = trimmed.chars().filter(|character| !character.is_whitespace()).count();
    words < SYNOPSIS_MIN_WORDS || chars < SYNOPSIS_MIN_CHARS
}

/// Minimum whitespace-delimited word count below which a `synopsis` is
/// flagged as thin.
const SYNOPSIS_MIN_WORDS: usize = 4;

/// Minimum non-whitespace character count below which a `synopsis` is
/// flagged as thin.
const SYNOPSIS_MIN_CHARS: usize = 20;

/// per-slice authority override orphan-source gate. Loads `plan.yaml` (when
/// present) and reports one finding per `(slice, kind)` pair
/// whose source value is not in the slice's own `sources[]`
/// list. Absent `plan.yaml` (e.g. ad-hoc slice without a plan)
/// skips the check silently; the structural issue would already
/// have surfaced earlier in workflow.
fn override_orphans(ctx: &Ctx, name: &str) -> Result<Vec<Diagnostic>> {
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
        .map(|f| {
            Diagnostic::violation(
                f.code,
                "Per-slice `authority-override` source key must appear in the slice's \
                 `sources[]` list",
                f.message,
                Artifact::Plan,
                None,
            )
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
fn collect_catalog_drift_findings(ctx: &Ctx, slice_dir: &Path) -> Result<Vec<Diagnostic>> {
    let Some(catalog) = ComponentsCatalog::load(&ctx.project_dir)? else {
        return Ok(Vec::new());
    };

    let paths = evidence_yaml_paths(slice_dir)?;
    let mut findings: Vec<Diagnostic> = Vec::new();

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

    findings.sort_by(|a, b| a.impact.cmp(&b.impact));
    Ok(findings)
}

fn catalog_drift_summary(detail: &str) -> Diagnostic {
    Diagnostic::violation(
        "slice-catalog-drift",
        "Evidence `component:` directives resolve to confirmed catalog entries",
        detail,
        Artifact::Specs,
        None,
    )
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
    Ok(entry.sources.iter().map(|b| b.source().to_string()).collect())
}

/// One-line text rendering of a diagnostic for the PASS/FAIL banner.
/// `violation` findings are blocking defects (`[fail]`); `review`
/// findings are deferred requests for judgment (`[review]`).
fn format_finding_line(d: &Diagnostic) -> String {
    let rule = d.rule_id.as_deref().unwrap_or("<unknown>");
    match d.kind {
        specify_diagnostics::DiagnosticKind::Violation => {
            format!("[fail] {}: {}", rule, d.impact)
        }
        specify_diagnostics::DiagnosticKind::Review => {
            format!("[review] {} ({})", rule, d.impact)
        }
    }
}
