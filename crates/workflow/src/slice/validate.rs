//! Slice-validation kernel for `specrun slice validate`.
//!
//! Holds the pure, `Ctx`-free gate logic the handler orchestrates: the
//! pre-adapter gates ([`pre_adapter_gates`]) — provenance scan, spec
//! file-location, per-slice authority-override orphans, component-catalog
//! drift, typed-model drift (RFC-29c §"Drift validation"), and Decision
//! Record gates — plus the non-blocking synopsis advisory and the
//! synthesis journal emission. Every entry point takes a [`Layout`] or
//! plain paths rather than the CLI `Ctx`, so the gates are unit-testable
//! without standing up a binary. Adapter validation (`validate_slice`,
//! from the sibling `specify-validate` crate) and report rendering stay
//! in the handler, which cannot live here without a forbidden
//! `specify-workflow → specify-validate` dependency.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value as JsonValue;
use specify_diagnostics::{Artifact, Diagnostic, FindingLocation};
use specify_error::{Error, Result};
use specify_model::decision::{DecisionRecord, parse_decision};
use specify_model::discovery::Discovery;
use specify_model::evidence::ClaimKind;
use specify_model::spec::provenance::{self, ParsedSpec, RequirementStatus, RequirementTag};

use crate::change::{Plan, orphan_authority_override_keys};
use crate::config::Layout;
use crate::decisions::{is_dec_ref, read_baseline};
use crate::design_system::{ComponentStatus, ComponentsCatalog};
use crate::journal::{Event, EventKind, append_batch};
use crate::schema::{evidence_yaml_paths, validate_evidence_dir};
use crate::slice::expected_provenance_lines;
use crate::slice::model::{SliceModel, validate_model_doc};

/// Outcome of the pre-adapter gate sweep ([`pre_adapter_gates`]).
///
/// `slice validate` runs structural gates before invoking the target
/// adapter's rules; a firing gate short-circuits adapter validation so
/// the operator sees the structural cause first.
#[derive(Debug)]
pub enum PreAdapter {
    /// A gate fired. `code` is the error discriminant the handler raises
    /// after rendering `findings`; `findings` are the blocking diagnostics
    /// for that gate.
    Gate {
        /// Stable `Error::Validation` discriminant for the failing gate.
        code: &'static str,
        /// Blocking diagnostics to render before failing.
        findings: Vec<Diagnostic>,
    },
    /// Every gate passed. The handler proceeds to adapter validation,
    /// folds `advisories` into the adapter findings, and — on overall
    /// success — journals `synthesis_tags`.
    Proceed {
        /// `(requirement-id, tag)` pairs to journal on overall success.
        synthesis_tags: Vec<(String, RequirementTag)>,
        /// Non-blocking advisories (synopsis content-floor) to render
        /// alongside the adapter findings.
        advisories: Vec<Diagnostic>,
    },
}

/// Run the pre-adapter gate sweep for slice `name`.
///
/// First-use schema validation of per-source `Evidence` files runs first
/// (per workflow §Source adapter contract); a structural Evidence problem
/// short-circuits with [`Error`] before any gate so the operator sees it
/// before downstream artefact noise. Then the provenance scan and the
/// pre-adapter gates fire in order, each able to return
/// [`PreAdapter::Gate`]. When all gates pass, returns
/// [`PreAdapter::Proceed`] carrying the synthesis tags and the synopsis
/// advisory surface.
///
/// # Errors
///
/// Returns [`Error`] when Evidence schema validation fails, or when a
/// plan, spec, model, discovery, decision, or Evidence file cannot be
/// read or parsed.
pub fn pre_adapter_gates(layout: Layout<'_>, name: &str) -> Result<PreAdapter> {
    let slice_dir = layout.slices_dir().join(name);
    validate_evidence_dir(&slice_dir)?;

    let source_keys = resolve_slice_source_keys(layout, name)?;
    let (_spec_req_ids, synthesis_tags, provenance_findings) =
        scan_slice_specs(&slice_dir, &source_keys)?;
    if !provenance_findings.is_empty() {
        return Ok(PreAdapter::Gate {
            code: "slice-provenance-invalid",
            findings: provenance_findings,
        });
    }

    let gate_findings = collect_pre_adapter_gates(layout, &slice_dir, name)?;
    if !gate_findings.is_empty() {
        return Ok(PreAdapter::Gate {
            code: "slice-pre-adapter-gate",
            findings: gate_findings,
        });
    }

    let advisories = synopsis_thin(layout)?;
    Ok(PreAdapter::Proceed {
        synthesis_tags,
        advisories,
    })
}

/// Append one `slice.synthesis.*` journal line per `(requirement-id,
/// tag)` pair gathered during the spec scan. Skipped when the slice has
/// no tagged requirements.
///
/// # Errors
///
/// Propagates the journal write error from [`append_batch`].
pub fn append_synthesis_journal(
    layout: Layout<'_>, slice_name: &str, tags: Vec<(String, RequirementTag)>,
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
                    slice_name: slice_name.into(),
                    requirement_id,
                },
                RequirementTag::Conflict => EventKind::SliceSynthesisConflict {
                    slice_name: slice_name.into(),
                    requirement_id,
                },
                RequirementTag::Divergence => EventKind::SliceSynthesisDivergence {
                    slice_name: slice_name.into(),
                    requirement_id,
                },
            };
            Event::new(now, kind)
        })
        .collect();
    append_batch(layout, &events)
}

/// Emit the RFC-29c §"Drift validation" findings over the slice's
/// `model.yaml`.
///
/// Skipped silently when the file is absent — every
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
/// why, and there is nothing typed left to inspect.
///
/// `plan_path` resolves the target-drift gate; when it does not exist
/// that gate no-ops.
///
/// # Errors
///
/// Returns [`Error::Filesystem`] when the model or a spec file cannot be
/// read, or a YAML parse error when `model.yaml` / an Evidence file is
/// malformed.
pub fn model_drift_findings(
    slice_dir: &Path, plan_path: &Path, slice_name: &str,
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
    findings.extend(target_drift_findings(plan_path, &model, slice_name)?);
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
fn target_drift_findings(
    plan_path: &Path, model: &SliceModel, name: &str,
) -> Result<Vec<Diagnostic>> {
    if !plan_path.exists() {
        return Ok(Vec::new());
    }
    let plan = Plan::load(plan_path)?;
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
/// 1. Spec file-location check — root `spec.md` exists
///    but no canonical `specs/<unit>/spec.md` files found. Fires
///    first so the operator sees the structural cause before
///    downstream drift noise.
/// 2. per-slice authority override — orphan source keys on the slice's
///    `plan.yaml.slices[].authority-override` map.
/// 3. component catalog contract — catalog drift between Evidence `component:`
///    directives and `.specify/design-system/components.yaml`.
/// 4. typed-model drift — the seven RFC-29c §"Drift validation"
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
fn collect_pre_adapter_gates(
    layout: Layout<'_>, slice_dir: &Path, name: &str,
) -> Result<Vec<Diagnostic>> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    findings.extend(collect_spec_file_location_findings(slice_dir));
    findings.extend(override_orphans(layout, name)?);
    findings.extend(collect_catalog_drift_findings(layout, slice_dir)?);
    findings.extend(model_drift_findings(slice_dir, &layout.plan_path(), name)?);
    findings.extend(collect_decision_gates(layout, slice_dir)?);
    Ok(findings)
}

/// Decision Record gate (RFC-36 §"Validation findings"). Over
/// `<slice>/decisions/*.md` it raises the per-file findings owned by the
/// `specify-model` parser — `decision-record-schema`,
/// `decision-record-section-missing`, `decision-slug-grammar` (the same
/// parser-drives-findings posture as the `spec.md` provenance parser, so
/// no JSON schema runs here) — plus the two cross-file checks the parser
/// cannot make alone:
///
/// - `decision-slug-collision` — two records in the slice share a `slug`.
/// - `decision-supersede-orphan` — a `supersedes:` target resolves to
///   neither the live baseline catalogue nor a sibling slice record.
///   Re-checked against the live baseline at merge (the baseline may move
///   between refine and merge).
///
/// Absent `decisions/` skips the gate silently — Decision Records are
/// opt-in.
fn collect_decision_gates(layout: Layout<'_>, slice_dir: &Path) -> Result<Vec<Diagnostic>> {
    let decisions_dir = slice_dir.join("decisions");
    if !decisions_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&decisions_dir).map_err(|source| Error::Filesystem {
        op: "readdir",
        path: decisions_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "readdir-entry",
            path: decisions_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();

    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut slug_files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut records: Vec<(String, DecisionRecord)> = Vec::new();

    for path in &files {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let hint = path_hint(path, slice_dir);
        let parsed = parse_decision(&text);
        for finding in parsed.findings {
            findings.push(finding.into_diagnostic(&hint));
        }
        if let Some(record) = parsed.record {
            slug_files.entry(record.slug.clone()).or_default().push(hint.clone());
            records.push((hint, record));
        }
    }

    for (slug, hints) in &slug_files {
        if hints.len() > 1 {
            findings.push(Diagnostic::violation(
                "decision-slug-collision",
                "Each Decision Record in the slice carries a distinct `slug`",
                format!("slug `{slug}` is shared by {} records: {}", hints.len(), hints.join(", ")),
                Artifact::Decisions,
                None,
            ));
        }
    }

    findings.extend(decision_supersede_orphans(layout, &records, &slug_files)?);
    Ok(findings)
}

/// `decision-supersede-orphan` — every `supersedes:` target must resolve
/// to a baseline `DEC-NNNN` (for a DEC reference) or to a baseline slug
/// or sibling slice record (for a slug reference).
fn decision_supersede_orphans(
    layout: Layout<'_>, records: &[(String, DecisionRecord)],
    slug_files: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<Diagnostic>> {
    let baseline = read_baseline(&layout.decisions_dir())?;
    let baseline_ids: BTreeSet<String> = baseline.iter().map(|b| b.id().to_string()).collect();
    let baseline_slugs: BTreeSet<String> = baseline.iter().map(|b| b.record.slug.clone()).collect();

    let mut findings: Vec<Diagnostic> = Vec::new();
    for (hint, record) in records {
        for target in &record.supersedes {
            let resolved = if is_dec_ref(target) {
                baseline_ids.contains(target)
            } else {
                baseline_slugs.contains(target) || slug_files.contains_key(target)
            };
            if !resolved {
                findings.push(Diagnostic::violation(
                    "decision-supersede-orphan",
                    "every `supersedes:` target resolves to a baseline DEC or a sibling record",
                    format!(
                        "decision `{}` (slug `{}`) supersedes `{target}`, which resolves to \
                         neither the baseline catalogue nor a sibling slice record",
                        record.slug, record.slug
                    ),
                    Artifact::Decisions,
                    Some(FindingLocation {
                        path: hint.clone(),
                        line: None,
                        column: None,
                        end_line: None,
                        end_column: None,
                    }),
                ));
            }
        }
    }
    Ok(findings)
}

/// Spec file-location gate. Emits a `specs.file-location`
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

/// Synopsis content-floor advisory (DECISIONS §Lead reconciliation D2.1). Loads
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
fn synopsis_thin(layout: Layout<'_>) -> Result<Vec<Diagnostic>> {
    let path = layout.discovery_path();
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

/// Contentfulness heuristic for a lead `synopsis` (DECISIONS §Lead reconciliation
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
fn override_orphans(layout: Layout<'_>, name: &str) -> Result<Vec<Diagnostic>> {
    let plan_path = layout.plan_path();
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
                f.rule_id.clone().unwrap_or_default(),
                "Per-slice `authority-override` source key must appear in the slice's \
                 `sources[]` list",
                f.impact,
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
fn collect_catalog_drift_findings(layout: Layout<'_>, slice_dir: &Path) -> Result<Vec<Diagnostic>> {
    let Some(catalog) = ComponentsCatalog::load(layout.project_dir())? else {
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
fn resolve_slice_source_keys(layout: Layout<'_>, name: &str) -> Result<BTreeSet<String>> {
    let plan_path = layout.plan_path();
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        collect_spec_file_location_findings, collect_spec_files, path_hint, synopsis_is_thin,
    };

    #[test]
    fn synopsis_thin_below_word_floor() {
        // Three contentful words clear the char floor but trip the word
        // floor (which sits at four).
        assert!(synopsis_is_thin("validates everything carefully"));
    }

    #[test]
    fn synopsis_thin_below_char_floor() {
        // Four short words clear the word floor but trip the char floor.
        assert!(synopsis_is_thin("a b c de"));
    }

    #[test]
    fn synopsis_contentful_clears_both_floors() {
        assert!(!synopsis_is_thin("register a new user account and enforce a unique email"));
    }

    #[test]
    fn synopsis_blank_is_thin() {
        assert!(synopsis_is_thin("   "));
    }

    #[test]
    fn path_hint_relativises_under_slice() {
        let slice = TempDir::new().unwrap();
        let spec = slice.path().join("specs").join("auth").join("spec.md");
        assert_eq!(path_hint(&spec, slice.path()), "specs/auth/spec.md");
    }

    #[test]
    fn path_hint_keeps_filename_outside() {
        let slice = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let stray = other.path().join("spec.md");
        // A path outside the slice dir cannot be stripped; the hint still
        // names the file rather than dropping it.
        assert!(path_hint(&stray, slice.path()).ends_with("spec.md"));
    }

    #[test]
    fn spec_files_walked_recursively_and_sorted() {
        let root = TempDir::new().unwrap();
        let unit_b = root.path().join("b");
        let unit_a = root.path().join("a");
        fs::create_dir_all(&unit_b).unwrap();
        fs::create_dir_all(&unit_a).unwrap();
        fs::write(unit_b.join("spec.md"), "b").unwrap();
        fs::write(unit_a.join("spec.md"), "a").unwrap();
        fs::write(root.path().join("notes.txt"), "ignored").unwrap();

        let found = collect_spec_files(root.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], unit_a.join("spec.md"));
        assert_eq!(found[1], unit_b.join("spec.md"));
    }

    #[test]
    fn file_location_flags_root_no_canonical() {
        let slice = TempDir::new().unwrap();
        fs::write(slice.path().join("spec.md"), "# misplaced").unwrap();
        let findings = collect_spec_file_location_findings(slice.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id.as_deref(), Some("specs.file-location"));
    }

    #[test]
    fn file_location_silent_with_canonical() {
        let slice = TempDir::new().unwrap();
        let unit = slice.path().join("specs").join("auth");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("spec.md"), "# canonical").unwrap();
        // Even a stray root spec.md does not fire once canonical specs exist.
        fs::write(slice.path().join("spec.md"), "# stray").unwrap();
        assert!(collect_spec_file_location_findings(slice.path()).is_empty());
    }

    #[test]
    fn file_location_silent_no_specs() {
        let slice = TempDir::new().unwrap();
        assert!(collect_spec_file_location_findings(slice.path()).is_empty());
    }
}
