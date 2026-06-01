//! `slice validate` — coherence check against the adapter validation
//! rules plus first-use schema validation of per-source `Evidence`
//! files and workflow §Requirement block contract validation of
//! `spec.md` provenance metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value as JsonValue;
use specify_diagnostics::{
    Artifact, Diagnostic, DiagnosticReport, DiagnosticReportVersion, DiagnosticSummary,
    blocking_present, renumber,
};
use specify_error::{Error, Result};
use specify_model::discovery::Discovery;
use specify_model::evidence::ClaimKind;
use specify_model::spec::provenance::{self, ParsedSpec, RequirementStatus, RequirementTag};
use specify_validate::validate_slice;
use specify_workflow::change::{Plan, orphan_authority_override_keys};
use specify_workflow::design_system::{ComponentStatus, ComponentsCatalog};
use specify_workflow::journal::{Event, EventKind, append_batch};
use specify_workflow::schema::{evidence_yaml_paths, validate_evidence_dir};
use specify_workflow::slice::model::validate_model_doc;
use specify_workflow::slice::{SliceModel, expected_provenance_lines};

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

/// Bundle the pre-adapter gates that fire on a single slice:
///
/// 1. RFC-35 D8 — spec file-location check: root `spec.md` exists
///    but no canonical `specs/<unit>/spec.md` files found. Fires
///    first so the operator sees the structural cause before
///    downstream drift noise.
/// 2. per-slice authority override — orphan source keys on the slice's
///    `plan.yaml.slices[].authority-override` map.
/// 3. discovery alias contract — candidate `id` ↔ `aliases[]` collisions in
///    `<project_dir>/discovery.md`. A discovery-level check (not
///    per-slice) but evaluated here because `specrun slice validate`
///    is the single CLI surface skills shell out to between
///    `/spec:refine` and `/spec:build`.
/// 4. component catalog contract — catalog drift between Evidence `component:`
///    directives and `.specify/design-system/components.yaml`.
/// 5. typed-model drift — the seven RFC-29c §"Drift validation"
///    findings over `<slice>/model.yaml` (skipped when absent).
///
/// Provenance no longer has a file-drift gate: it is carried inline in
/// `model.yaml` and projected on demand (`specrun slice provenance`),
/// so there is no second representation to drift against. Spec-level
/// `Sources:` / `Status:` coherence still runs in [`scan_slice_specs`].
///
/// All checks can fail independently; we collect every finding
/// into one [`Diagnostic`] vector so the caller can render the full
/// surface in one pass instead of one error per re-run.
fn collect_pre_adapter_gates(ctx: &Ctx, slice_dir: &Path, name: &str) -> Result<Vec<Diagnostic>> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    findings.extend(collect_spec_file_location_findings(slice_dir));
    findings.extend(override_orphans(ctx, name)?);
    findings.extend(alias_collisions(ctx)?);
    findings.extend(collect_catalog_drift_findings(ctx, slice_dir)?);
    findings.extend(collect_model_drift_findings(ctx, slice_dir, name)?);
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

/// RFC-35 D8 file-location gate. Emits a `specs.file-location`
/// finding when the slice has no spec files under the canonical
/// `specs/<unit>/spec.md` layout but does have a root-level
/// `spec.md`. This fires first among the pre-adapter gates so the
/// operator sees the structural cause before downstream drift noise.
fn collect_spec_file_location_findings(slice_dir: &Path) -> Vec<Diagnostic> {
    let specs_dir = slice_dir.join("specs");
    let has_canonical_specs =
        specs_dir.is_dir() && collect_spec_files(&specs_dir).is_ok_and(|files| !files.is_empty());
    if has_canonical_specs {
        return Vec::new();
    }
    let root_spec = slice_dir.join("spec.md");
    if !root_spec.is_file() {
        return Vec::new();
    }
    vec![Diagnostic::violation(
        "specs.file-location",
        "Spec files live under specs/<unit>/spec.md, not at the slice root",
        "No spec files found under `specs/`. Found `spec.md` at the slice root — \
         move it to `specs/<unit>/spec.md` (one file per `proposal.md ## Units` entry). \
         The Specify workflow requires spec files under `specs/` for every target.",
        Artifact::Specs,
        None,
    )]
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

// ---------------------------------------------------------------------------
// RFC-29c §"Drift validation" — typed-model drift gate
// ---------------------------------------------------------------------------

/// Emit the seven RFC-29c §"Drift validation" findings over the slice's
/// `model.yaml`. Skipped silently when the file is absent — every
/// synthesized slice carries it, but `slice validate` runs on
/// pre-synthesis (`refining`) slices too, so absence is not a defect
/// here (it is enforced at synthesize time and by
/// `slice provenance` / `slice model show`).
///
/// The schema gate (`slice-model-schema`) and the typed model-derived
/// gates are evaluated independently from the same raw document: the
/// embedded `model.schema.json` overlaps several of the structural
/// checks (e.g. it pins the `REQ` / `TASK` id patterns), so collecting
/// both surfaces every disagreement in one pass. The model-derived
/// checks short-circuit only when the document cannot deserialise into
/// the typed view at all — then the schema finding already explains
/// why, and there is nothing typed left to inspect (mirroring the
/// Evidence-schema short-circuit at the top of [`run`]).
fn collect_model_drift_findings(
    ctx: &Ctx, slice_dir: &Path, name: &str,
) -> Result<Vec<Diagnostic>> {
    let model_path = slice_dir.join("model.yaml");
    if !model_path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&model_path).map_err(|source| Error::Filesystem {
        op: "read",
        path: model_path.clone(),
        source,
    })?;
    let value: JsonValue = serde_saphyr::from_str(&raw)?;

    let mut findings = Vec::new();
    if let Err(Error::Validation { detail, .. }) = validate_model_doc(&value) {
        findings.push(model_schema_finding(detail));
    }
    let Ok(model) = serde_saphyr::from_str::<SliceModel>(&raw) else {
        return Ok(findings);
    };

    let evidence = EvidenceFacts::read(slice_dir)?;
    findings.extend(provenance_stale_findings(slice_dir, &model)?);
    findings.extend(target_drift_findings(ctx, &model, name)?);
    findings.extend(source_orphan_findings(&model, &evidence));
    findings.extend(cross_ref_orphan_findings(&model));
    findings.extend(claim_kind_mismatch_findings(&model, &evidence));
    findings.extend(id_grammar_findings(&model));
    Ok(findings)
}

fn model_drift(code: &'static str, rule: &'static str, detail: String) -> Diagnostic {
    Diagnostic::violation(code, rule, detail, Artifact::Specs, None)
}

fn model_schema_finding(detail: String) -> Diagnostic {
    model_drift(
        "slice-model-schema",
        "model.yaml conforms to schemas/slice/model.schema.json",
        detail,
    )
}

/// `slice-spec-provenance-stale` — compare each model requirement's
/// kernel-owned `id` / `sources` / `status` against the matching
/// requirement parsed from the on-disk `specs/<unit>/spec.md`. A
/// disagreement (or an absent rendered requirement) means an operator
/// hand-edited a kernel-rendered provenance line without
/// re-synthesising.
fn provenance_stale_findings(slice_dir: &Path, model: &SliceModel) -> Result<Vec<Diagnostic>> {
    const RULE: &str = "spec.md provenance lines agree with model.yaml";
    let mut parsed_units: BTreeMap<String, Option<ParsedSpec>> = BTreeMap::new();
    let mut findings = Vec::new();
    for exp in expected_provenance_lines(model) {
        if exp.id.is_empty() {
            continue;
        }
        if !parsed_units.contains_key(&exp.unit) {
            let path = slice_dir.join("specs").join(&exp.unit).join("spec.md");
            let parsed = match std::fs::read_to_string(&path) {
                Ok(text) => Some(provenance::parse_spec_md(&text)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(Error::Filesystem {
                        op: "read",
                        path,
                        source,
                    });
                }
            };
            parsed_units.insert(exp.unit.clone(), parsed);
        }
        let Some(parsed) = parsed_units.get(&exp.unit).and_then(Option::as_ref) else {
            findings.push(model_drift(
                "slice-spec-provenance-stale",
                RULE,
                format!(
                    "model requirement `{}` has no rendered `specs/{}/spec.md`",
                    exp.id, exp.unit
                ),
            ));
            continue;
        };
        let Some(req) = parsed.requirements.iter().find(|r| r.id == exp.id) else {
            findings.push(model_drift(
                "slice-spec-provenance-stale",
                RULE,
                format!(
                    "model requirement `{}` is absent from `specs/{}/spec.md`",
                    exp.id, exp.unit
                ),
            ));
            continue;
        };
        if req.sources != exp.sources {
            findings.push(model_drift(
                "slice-spec-provenance-stale",
                RULE,
                format!(
                    "requirement `{}` `Sources:` in `specs/{}/spec.md` ({}) disagrees with \
                     model.yaml ({})",
                    exp.id,
                    exp.unit,
                    render_sources(&req.sources),
                    render_sources(&exp.sources),
                ),
            ));
        }
        if req.status != exp.status {
            findings.push(model_drift(
                "slice-spec-provenance-stale",
                RULE,
                format!(
                    "requirement `{}` `Status:` in `specs/{}/spec.md` ({}) disagrees with \
                     model.yaml ({})",
                    exp.id,
                    exp.unit,
                    render_status(req.status),
                    render_status(exp.status),
                ),
            ));
        }
    }
    Ok(findings)
}

fn render_sources(sources: &[String]) -> String {
    if sources.is_empty() { "<none>".to_string() } else { sources.join(", ") }
}

fn render_status(status: Option<RequirementStatus>) -> String {
    status.map_or_else(|| "<none>".to_string(), |s| s.to_string())
}

/// `slice-model-target-drift` — the persisted `model.yaml.project` must
/// agree with the slice's `plan.yaml` entry `project`. Only flagged
/// when both carry an explicit value and they differ; an omitted plan
/// `project` resolves to the sole topology project, so it cannot
/// disagree. Skipped when no `plan.yaml` exists or the slice has no
/// matching entry. `target` is never persisted, so there is no
/// target-vs-resolved-target half.
fn target_drift_findings(ctx: &Ctx, model: &SliceModel, name: &str) -> Result<Vec<Diagnostic>> {
    let plan_path = ctx.layout().plan_path();
    if !plan_path.exists() {
        return Ok(Vec::new());
    }
    let plan = Plan::load(&plan_path)?;
    let Some(entry) = plan.entries.iter().find(|e| e.name == name) else {
        return Ok(Vec::new());
    };
    match (model.project.as_deref(), entry.project.as_deref()) {
        (Some(model_project), Some(plan_project)) if model_project != plan_project => {
            Ok(vec![Diagnostic::violation(
                "slice-model-target-drift",
                "model.yaml `project` agrees with the slice's plan entry",
                format!(
                    "model.yaml `project: {model_project}` disagrees with plan.yaml slice \
                     `{name}` `project: {plan_project}`"
                ),
                Artifact::Plan,
                None,
            )])
        }
        _ => Ok(Vec::new()),
    }
}

/// `slice-model-source-orphan` — every contributing claim must trace to
/// a real `(source, id)` in the slice's Evidence: the `source` key must
/// own an `evidence/<source>.yaml`, and that file must carry a claim
/// with the cited `id`.
fn source_orphan_findings(model: &SliceModel, evidence: &EvidenceFacts) -> Vec<Diagnostic> {
    const RULE: &str = "every claim traces to a real Evidence `(source, id)`";
    let mut findings = Vec::new();
    for claim in model.requirements.iter().flat_map(|req| &req.claims) {
        if !evidence.sources.contains(&claim.source) {
            findings.push(model_drift(
                "slice-model-source-orphan",
                RULE,
                format!(
                    "claim `{}:{}` references source key `{}`, which has no `evidence/{}.yaml`",
                    claim.source, claim.id, claim.source, claim.source
                ),
            ));
        } else if !evidence.claim_kinds.contains_key(&(claim.source.clone(), claim.id.clone())) {
            findings.push(model_drift(
                "slice-model-source-orphan",
                RULE,
                format!(
                    "claim `{}:{}` references an Evidence claim id absent from `evidence/{}.yaml`",
                    claim.source, claim.id, claim.source
                ),
            ));
        }
    }
    findings
}

/// `slice-model-cross-ref-orphan` — every `tasks[].satisfies[]`
/// reference must name an existing `requirements[].id`.
fn cross_ref_orphan_findings(model: &SliceModel) -> Vec<Diagnostic> {
    const RULE: &str = "every `satisfies[]` reference names an existing requirement";
    let req_ids: BTreeSet<&str> =
        model.requirements.iter().filter_map(|req| req.id.as_deref()).collect();
    let mut findings = Vec::new();
    for task in &model.tasks {
        for req_ref in &task.satisfies {
            if !req_ids.contains(req_ref.as_str()) {
                findings.push(model_drift(
                    "slice-model-cross-ref-orphan",
                    RULE,
                    format!(
                        "task `{}` `satisfies` references `{}`, which is not a `requirements[].id`",
                        task.id, req_ref
                    ),
                ));
            }
        }
    }
    findings
}

/// `slice-model-claim-kind-mismatch` (D13) — a claim's `kind` in
/// `model.yaml` must equal the `kind` recorded on the matching Evidence
/// claim. Claims with no matching Evidence `(source, id)` are left to
/// [`source_orphan_findings`].
fn claim_kind_mismatch_findings(model: &SliceModel, evidence: &EvidenceFacts) -> Vec<Diagnostic> {
    const RULE: &str = "claim `kind` agrees with the Evidence claim it traces to";
    let mut findings = Vec::new();
    for claim in model.requirements.iter().flat_map(|req| &req.claims) {
        let key = (claim.source.clone(), claim.id.clone());
        if let Some(evidence_kind) = evidence.claim_kinds.get(&key)
            && *evidence_kind != claim.kind
        {
            findings.push(model_drift(
                "slice-model-claim-kind-mismatch",
                RULE,
                format!(
                    "claim `{}:{}` has `kind: {}` in model.yaml but `kind: {}` in \
                     `evidence/{}.yaml`",
                    claim.source, claim.id, claim.kind, evidence_kind, claim.source
                ),
            ));
        }
    }
    findings
}

/// `slice-model-id-grammar` — `requirements[].id` matches `^REQ-[0-9]{3}$`,
/// `tasks[].id` and `depends-on[]` match `^TASK-[0-9]{3}$`, and
/// `satisfies[]` references match `^REQ-[0-9]{3}$` (RFC-29c §"ID grammar").
fn id_grammar_findings(model: &SliceModel) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    for id in model.requirements.iter().filter_map(|req| req.id.as_deref()) {
        if !is_req_id(id) {
            findings.push(id_grammar_finding(format!(
                "requirement id `{id}` does not match `^REQ-[0-9]{{3}}$`"
            )));
        }
    }
    for task in &model.tasks {
        if !is_task_id(&task.id) {
            findings.push(id_grammar_finding(format!(
                "task id `{}` does not match `^TASK-[0-9]{{3}}$`",
                task.id
            )));
        }
        for dep in &task.depends_on {
            if !is_task_id(dep) {
                findings.push(id_grammar_finding(format!(
                    "task `{}` `depends-on` entry `{}` does not match `^TASK-[0-9]{{3}}$`",
                    task.id, dep
                )));
            }
        }
        for req_ref in &task.satisfies {
            if !is_req_id(req_ref) {
                findings.push(id_grammar_finding(format!(
                    "task `{}` `satisfies` entry `{}` does not match `^REQ-[0-9]{{3}}$`",
                    task.id, req_ref
                )));
            }
        }
    }
    findings
}

fn id_grammar_finding(detail: String) -> Diagnostic {
    model_drift(
        "slice-model-id-grammar",
        "`REQ` / `TASK` ids match their closed three-digit grammar",
        detail,
    )
}

fn is_req_id(id: &str) -> bool {
    is_three_digit_id(id, "REQ-")
}

fn is_task_id(id: &str) -> bool {
    is_three_digit_id(id, "TASK-")
}

fn is_three_digit_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .is_some_and(|tail| tail.len() == 3 && tail.bytes().all(|b| b.is_ascii_digit()))
}

/// Per-slice Evidence facts the model-drift checks read: the set of
/// source keys (one per `evidence/*.yaml`) and the `(source, id)` →
/// [`ClaimKind`] map for the source-orphan and kind-mismatch checks.
/// Mirrors the local Evidence read in [`collect_catalog_drift_findings`].
struct EvidenceFacts {
    sources: BTreeSet<String>,
    claim_kinds: BTreeMap<(String, String), ClaimKind>,
}

impl EvidenceFacts {
    fn read(slice_dir: &Path) -> Result<Self> {
        let mut sources = BTreeSet::new();
        let mut claim_kinds = BTreeMap::new();
        for path in evidence_yaml_paths(slice_dir)? {
            let raw = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
                op: "read",
                path: path.clone(),
                source,
            })?;
            let doc: JsonValue = serde_saphyr::from_str(&raw)?;
            let source = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            sources.insert(source.clone());
            let Some(claims) = doc.get("claims").and_then(JsonValue::as_array) else {
                continue;
            };
            for claim in claims {
                let Some(id) = claim.get("id").and_then(JsonValue::as_str) else {
                    continue;
                };
                let Some(kind) = claim
                    .get("kind")
                    .and_then(JsonValue::as_str)
                    .and_then(|raw| raw.parse::<ClaimKind>().ok())
                else {
                    continue;
                };
                claim_kinds.insert((source.clone(), id.to_string()), kind);
            }
        }
        Ok(Self { sources, claim_kinds })
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
