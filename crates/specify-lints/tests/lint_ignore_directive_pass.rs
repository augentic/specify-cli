//! End-to-end acceptance goldens for the RFC-33a directive pass.
//!
//! Drives the full pipeline — `lint::index::build` → `lint::eval::evaluate`
//! → `lint::ignore::apply` → JSON envelope render — against tiny Rust
//! fixtures and pins each scenario from RFC-33a §"Implementation
//! plan" step 7 and the RFC-33a plan §"C8 Acceptance golden tests"
//! eight-scenario list:
//!
//! 1. No directives present → all findings `status: open` with no
//!    `disposition` on the wire, and the fingerprint matches a value
//!    that would be computed by the pre-RFC-33a algorithm (the
//!    fingerprint excludes `status` / `disposition` per RFC-28).
//! 2. Directive matches a finding → finding flips to `status:
//!    ignored`, `disposition.directive` populated.
//! 3. `false-positive:` rationale → finding flips to `status:
//!    false-positive`, `disposition.directive.rationale` carries the
//!    `false-positive:` prefix verbatim.
//! 4. Unrationaled directive → synthetic `UNI-022` emitted with the
//!    severity authored on the resolved rule (`important`).
//! 5. Too-short rationale (< 16 chars) → synthetic `UNI-022` emitted
//!    same as (4).
//! 6. Orphan directive (rule id matches no finding on the target
//!    line) → synthetic `UNI-023` emitted.
//! 7. Graceful degradation — `UNI-022` / `UNI-023` absent from the
//!    resolved codex → no synthetic findings emitted; status
//!    stamping for matched directives still works.
//!
//! Golden JSON envelopes live under
//! `tests/fixtures/lint/ignore_directive_<scenario>.json`. Pretty-
//! formatter goldens live under
//! `tests/goldens/ignore_directive_<scenario>_pretty.txt` for the
//! scenarios whose pretty output materially differs from the JSON
//! one (matched dispositions and synthetic findings).
//!
//! Regenerate via `REGENERATE_GOLDENS=1 cargo nextest run -p
//! specify-lints --test lint_ignore_directive_pass` and review every
//! diff before committing per `docs/standards/testing.md`.
//!
//! The JSON goldens are normalised to strip `id` (the producer-side
//! `FIND-NNNN` counter) and `fingerprint` (sha256 over inputs that
//! include the producer fixture path — kept stable but not asserted
//! byte-for-byte so future evidence-cap drift surfaces as a tiny
//! diff rather than every golden going red). The fingerprint
//! algorithm itself is pinned by
//! `crates/specify-lints/src/rules/fingerprint.rs` (the
//! `golden_fingerprint_pins_algorithm` and
//! `excluded_producer_fields_do_not_change_fingerprint` tests
//! together cover RFC-33a §"Implementation plan" step 1 from the
//! algorithm side); the pretty goldens leave fingerprints implicit
//! because the pretty formatter does not render them.

mod eval_support;

use std::fs;
use std::path::PathBuf;

use eval_support::{NoToolRunner, hint, make_rule};
use serde_json::Value;
use specify_lints::lint::ScanProfile;
use specify_lints::lint::diagnostics::{
    Format, LintResult, LintResultVersion, LintSummary, render,
};
use specify_lints::lint::eval::{ToolRunner, evaluate};
use specify_lints::lint::ignore::apply as apply_directives;
use specify_lints::lint::index::build;
use specify_lints::rules::fingerprint::fingerprint as compute_fingerprint;
use specify_lints::rules::{HintKind, LintFinding, Origin, PathRoot, ResolvedRule, Severity};

/// Pre-RFC-33a fingerprint snapshot for the URL fixture below.
///
/// Algorithmically equivalent to the value `compute_fingerprint`
/// returns on the URL finding the scanner mints from
/// [`URL_FIXTURE_BODY`] — `status` and `disposition` are excluded
/// from the fingerprint per RFC-28 §"Fingerprint algorithm", so the
/// snapshot holds whether the finding has been stamped by the
/// directive pass or not. If this constant ever needs updating the
/// algorithm has drifted; bump the version, do not edit the
/// constant. Aligned with the algorithm canary at
/// `crates/specify-lints/src/rules/fingerprint.rs::golden_fingerprint_pins_algorithm`.
const PRE_RFC_33A_URL_FINGERPRINT: &str =
    "sha256:28b4fbaa698f563815a938785e7bf618e512dd6e09bc2bfb9e55881f80c7c76d";

/// Rust file with one `https://` literal — the scanner's only
/// finding source across the matched / `false-positive` / orphan /
/// graceful-degradation scenarios.
const URL_FIXTURE_BODY: &str = "const BASE_URL: &str = \"https://api.example.com\";\n";

/// Same line of code as [`URL_FIXTURE_BODY`] with a long-enough
/// `specify-ignore: UNI-014` directive on the line above. Pairs the
/// regex-emitted finding with a matching directive on its target
/// line so the post-pass stamps `status: ignored`.
const URL_WITH_DIRECTIVE_FIXTURE_BODY: &str = "// specify-ignore: UNI-014 — internal endpoint pinned per ops policy\nconst BASE_URL: &str = \"https://api.example.com\";\n";

/// Same shape as [`URL_WITH_DIRECTIVE_FIXTURE_BODY`] but the
/// rationale opens with the `false-positive:` prefix that demotes
/// the finding to `status: false-positive` per RFC-33a §"Finding
/// status taxonomy".
const URL_FALSE_POSITIVE_FIXTURE_BODY: &str = "// specify-ignore: UNI-014 — false-positive: scanner pattern misfires on the demo stub URL\nconst BASE_URL: &str = \"https://api.example.com\";\n";

/// Same line of code as [`URL_FIXTURE_BODY`] with an unrationaled
/// `specify-ignore: UNI-014` directive on the line above. Drives
/// scenario 4 (no rationale → `UNI-022`).
const URL_WITH_UNRATIONED_DIRECTIVE_FIXTURE_BODY: &str =
    "// specify-ignore: UNI-014\nconst BASE_URL: &str = \"https://api.example.com\";\n";

/// Same shape as [`URL_WITH_DIRECTIVE_FIXTURE_BODY`] but the
/// rationale is below the 16-character floor; the directive pass
/// still matches the finding and stamps `status: ignored` (the
/// rationale is captured verbatim), and additionally mints a
/// synthetic `UNI-022` per RFC-33a §"Implementation plan" step 4.
const URL_WITH_SHORT_DIRECTIVE_FIXTURE_BODY: &str =
    "// specify-ignore: UNI-014 — too short\nconst BASE_URL: &str = \"https://api.example.com\";\n";

/// Same line of code as [`URL_FIXTURE_BODY`] with a long-enough
/// directive whose `<RULE-ID>` does not match any finding on the
/// target line. Drives scenario 6 (orphan → `UNI-023`).
const URL_WITH_ORPHAN_DIRECTIVE_FIXTURE_BODY: &str = "// specify-ignore: UNI-999 — bogus rule id with rationale long enough to clear the floor\nconst BASE_URL: &str = \"https://api.example.com\";\n";

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn json_goldens_dir() -> PathBuf {
    crate_root().join("tests").join("fixtures").join("lint")
}

fn pretty_goldens_dir() -> PathBuf {
    crate_root().join("tests").join("goldens")
}

fn validation_rule(rule_id: &str) -> ResolvedRule {
    ResolvedRule {
        rule_id: rule_id.into(),
        title: format!("{rule_id} title"),
        severity: Severity::Important,
        trigger: format!("Trigger for {rule_id}"),
        lint_mode: None,
        applicability: None,
        deterministic_hints: None,
        references: None,
        origin: Origin::Shared,
        path_root: PathRoot::RulesRoot,
        path: format!("adapters/shared/rules/universal/{rule_id}.md"),
        body: String::new(),
        deprecated: None,
    }
}

/// Inputs every scenario shares.
struct Scenario {
    /// Bytes the harness writes to `app.rs` inside the tempdir.
    fixture_body: &'static str,
    /// `(rule-id, regex)` pair used to mint the underlying finding.
    /// The pattern is the same `https?://` URL trigger every fixture
    /// uses.
    primary_rule_id: &'static str,
    /// Whether the resolved-codex slice carries `UNI-022`. Scenario
    /// 7 sets this to `false` to exercise graceful degradation.
    resolve_uni_022: bool,
    /// Whether the resolved-codex slice carries `UNI-023`. Scenario
    /// 7 sets this to `false` to exercise graceful degradation.
    resolve_uni_023: bool,
}

/// Drive the full pipeline and return the rendered envelope plus
/// the in-memory finding set so individual scenarios can layer
/// extra structural assertions on top of the golden comparison.
fn run_scenario(scenario: &Scenario) -> (LintResult, Vec<LintFinding>) {
    let tmp = tempfile::tempdir().expect("tmp");
    fs::write(tmp.path().join("app.rs"), scenario.fixture_body).expect("write fixture");

    let model = build(tmp.path(), ScanProfile::Consumer, &[], &[]).expect("build model");

    let url_rule = make_rule(scenario.primary_rule_id, vec![hint(HintKind::Regex, "https?://")]);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &url_rule,
        url_rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    )
    .expect("evaluate ok");

    let mut findings: Vec<LintFinding> = outcome.findings;

    let mut resolved_rules = vec![url_rule];
    if scenario.resolve_uni_022 {
        resolved_rules.push(validation_rule("UNI-022"));
    }
    if scenario.resolve_uni_023 {
        resolved_rules.push(validation_rule("UNI-023"));
    }
    let ignore_outcome = apply_directives(
        &mut findings,
        &model.ignore_directives,
        &resolved_rules,
        outcome.next_id_counter,
    );
    findings.extend(ignore_outcome.synthetics);

    let result = LintResult {
        version: LintResultVersion,
        summary: LintSummary::from_findings(&findings),
        findings: findings.clone(),
    };
    (result, findings)
}

/// Strip producer-local volatile fields so JSON goldens survive
/// `FIND-NNNN` reshuffles. The fingerprint is also stripped because
/// the snippet evidence is verbatim-equal across runs but
/// fingerprint determinism is pinned separately by
/// `crates/specify-lints/src/rules/fingerprint.rs`.
fn normalise(value: Value) -> Value {
    let mut value = value;
    if let Value::Object(map) = &mut value
        && let Some(Value::Array(findings)) = map.get_mut("findings")
    {
        for f in findings {
            if let Value::Object(obj) = f {
                obj.remove("id");
                obj.remove("fingerprint");
            }
        }
    }
    value
}

#[track_caller]
fn assert_json_golden(result: &LintResult, golden_name: &str) {
    let rendered = render(Format::Json, result).expect("render json");
    let value: Value = serde_json::from_str(&rendered).expect("parse rendered");
    let normalised = normalise(value);
    let pretty = serde_json::to_string_pretty(&normalised).expect("pretty");

    let golden = json_goldens_dir().join(golden_name);
    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        if let Some(parent) = golden.parent() {
            fs::create_dir_all(parent).expect("mk golden parent");
        }
        fs::write(&golden, format!("{pretty}\n")).expect("write golden");
        return;
    }

    let expected = fs::read_to_string(&golden).unwrap_or_else(|err| {
        panic!("missing golden {}: {err}; regenerate with REGENERATE_GOLDENS=1", golden.display())
    });
    let expected_value: Value = serde_json::from_str(&expected).expect("parse golden");
    assert_eq!(
        normalised,
        expected_value,
        "JSON envelope diverged from golden {}. Actual:\n{pretty}",
        golden.display()
    );
}

#[track_caller]
fn assert_pretty_golden(result: &LintResult, golden_name: &str) {
    let rendered = render(Format::Pretty, result).expect("render pretty");

    let golden = pretty_goldens_dir().join(golden_name);
    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        if let Some(parent) = golden.parent() {
            fs::create_dir_all(parent).expect("mk golden parent");
        }
        fs::write(&golden, &rendered).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(&golden).unwrap_or_else(|err| {
        panic!("missing golden {}: {err}; regenerate with REGENERATE_GOLDENS=1", golden.display())
    });
    assert_eq!(
        rendered,
        expected,
        "pretty golden drift at {}; rerender with REGENERATE_GOLDENS=1 if intentional",
        golden.display()
    );
}

/// Helper that runs the pretty formatter with `NO_COLOR=1` so the
/// golden is ANSI-free across CI environments. C7 may add a
/// `[status]` token to the pretty body — the goldens absorb that
/// when present and are deliberately checked into a separate file
/// per scenario so a single human review of `git diff` after C7
/// lands flips all of them at once.
#[expect(
    unsafe_code,
    reason = "env::set_var / env::remove_var are unsafe under Rust 2024; this binary's pretty assertions live in dedicated #[test] functions and no other thread reads NO_COLOR in parallel."
)]
#[track_caller]
fn with_no_color<R>(f: impl FnOnce() -> R) -> R {
    // SAFETY: nextest grants this binary a fresh process per test
    // and no other thread reads `NO_COLOR` while the closure runs.
    let () = unsafe { std::env::set_var("NO_COLOR", "1") };
    let out = f();
    // SAFETY: same single-test sequencing argument as above.
    let () = unsafe { std::env::remove_var("NO_COLOR") };
    out
}

/// Scenario 1 — no directives present.
///
/// Drives the URL fixture without any `specify-ignore` directive in
/// source. The matched finding stays `status: open` (omitted on
/// the wire by `skip_serializing_if` per RFC-28 §"Schema"); no
/// `disposition` field appears, and the fingerprint matches the
/// pre-RFC-33a snapshot. Plus an inline guard that mutating
/// `status` / `disposition` post-emission does not change the
/// fingerprint — RFC-33a §"Schema changes" §"Backwards
/// compatibility" promises additivity, and this assertion is the
/// end-to-end witness.
#[test]
fn scenario_1_no_directives_stable_fp() {
    let scenario = Scenario {
        fixture_body: URL_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 1, "exactly one finding for the URL match");
    let finding = &findings[0];
    assert!(finding.status.is_none(), "scanner output is `open` (status omitted on wire)");
    assert!(finding.disposition.is_none(), "no directive ⇒ no disposition");

    assert_eq!(
        finding.fingerprint, PRE_RFC_33A_URL_FINGERPRINT,
        "fingerprint must match the pre-RFC-33a snapshot — RFC-28 excludes status/disposition",
    );

    // End-to-end witness that the additive RFC-33a fields are
    // outside the fingerprint preimage: stamp them on a clone and
    // assert the recomputed fingerprint is byte-equal.
    let mut stamped = finding.clone();
    stamped.status = Some(specify_lints::rules::FindingStatus::Ignored);
    stamped.disposition = Some(specify_lints::rules::FindingDisposition {
        source: specify_lints::rules::DispositionSource::Directive,
        directive: Some(specify_lints::rules::DirectiveDisposition {
            path: "app.rs".into(),
            line: 1,
            rationale: "fingerprint must survive disposition stamping per RFC-28".into(),
        }),
        since: None,
    });
    assert_eq!(
        compute_fingerprint(&stamped),
        PRE_RFC_33A_URL_FINGERPRINT,
        "RFC-33a status/disposition must not enter the fingerprint preimage",
    );

    assert_json_golden(&result, "ignore_directive_scenario_1_no_directives.json");
}

/// Scenario 2 — directive matches a finding (happy path).
///
/// The seed C6 happy-path: a long-enough rationale flips the URL
/// finding to `status: ignored` and populates `disposition.directive`.
/// Renders both the JSON envelope and the pretty formatter so the
/// pinned wire shape is reviewed alongside the human-readable
/// output. C7's optional `[status]` token surfaces in the pretty
/// golden when it lands; the assertion is the golden file itself,
/// not a substring check, so the snapshot reviewer is the source of
/// truth.
#[test]
fn scenario_2_directive_match_flips_finding_to_ignored() {
    let scenario = Scenario {
        fixture_body: URL_WITH_DIRECTIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 1, "single URL finding; no synthetics");
    let f = &findings[0];
    assert_eq!(f.status, Some(specify_lints::rules::FindingStatus::Ignored));
    let disp = f.disposition.as_ref().expect("disposition stamped");
    assert_eq!(disp.source, specify_lints::rules::DispositionSource::Directive);
    let directive = disp.directive.as_ref().expect("directive payload populated");
    assert_eq!(directive.path, "app.rs");
    assert_eq!(directive.line, 1);
    assert_eq!(directive.rationale, "internal endpoint pinned per ops policy");

    assert_json_golden(&result, "ignore_directive_scenario_2_match.json");
    with_no_color(|| {
        assert_pretty_golden(&result, "ignore_directive_scenario_2_match_pretty.txt");
    });
}

/// Scenario 3 — `false-positive:` rationale.
///
/// Rationale opens with the literal `false-positive:` prefix so the
/// directive pass demotes the finding to `status: false-positive`
/// instead of `ignored`. The full rationale (prefix and all) is
/// captured on the wire in `disposition.directive.rationale`.
#[test]
fn scenario_3_false_positive_prefix() {
    let scenario = Scenario {
        fixture_body: URL_FALSE_POSITIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 1, "single URL finding; no synthetics");
    let f = &findings[0];
    assert_eq!(f.status, Some(specify_lints::rules::FindingStatus::FalsePositive));
    let rationale = &f
        .disposition
        .as_ref()
        .expect("disposition stamped")
        .directive
        .as_ref()
        .expect("directive payload populated")
        .rationale;
    assert!(
        rationale.starts_with("false-positive:"),
        "rationale must carry the verbatim false-positive: prefix; got `{rationale}`",
    );

    assert_json_golden(&result, "ignore_directive_scenario_3_false_positive.json");
}

/// Scenario 4 — unrationaled directive.
///
/// `// specify-ignore: UNI-014` with no rationale at all. The
/// directive still matches the URL finding (stamping `status:
/// ignored` with an empty rationale verbatim) and synthesises a
/// `UNI-022` per RFC-33a §"Directive-without-rationale is a
/// finding" (D4). The synthetic carries the rule's authored
/// severity (`important`) per C2.
#[test]
fn scenario_4_missing_rationale_mints_uni_022_synthetic() {
    let scenario = Scenario {
        fixture_body: URL_WITH_UNRATIONED_DIRECTIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 2, "matched URL + UNI-022 synthetic; got {findings:?}");
    let uni_022 = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("UNI-022"))
        .expect("UNI-022 synthetic present");
    assert_eq!(uni_022.severity, Severity::Important, "severity comes from the resolved rule");
    assert_eq!(uni_022.status, Some(specify_lints::rules::FindingStatus::Open));
    assert!(uni_022.disposition.is_none(), "synthetic findings carry no disposition");

    assert_json_golden(&result, "ignore_directive_scenario_4_missing_rationale.json");
    with_no_color(|| {
        assert_pretty_golden(&result, "ignore_directive_scenario_4_missing_rationale_pretty.txt");
    });
}

/// Scenario 5 — rationale shorter than 16 chars.
///
/// Same shape as scenario 4 but the rationale is present and
/// captured verbatim; the validation pass still mints `UNI-022`
/// because the rationale is below the 16-character floor pinned by
/// RFC-33a D12.
#[test]
fn scenario_5_short_rationale_mints_uni_022_synthetic() {
    let scenario = Scenario {
        fixture_body: URL_WITH_SHORT_DIRECTIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 2, "matched URL + UNI-022 synthetic");
    let url_finding = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("UNI-014"))
        .expect("URL finding present");
    assert_eq!(
        url_finding.status,
        Some(specify_lints::rules::FindingStatus::Ignored),
        "short rationale still flips the matched finding",
    );
    assert_eq!(
        url_finding
            .disposition
            .as_ref()
            .expect("disposition stamped")
            .directive
            .as_ref()
            .expect("directive payload populated")
            .rationale,
        "too short",
        "captured rationale is verbatim — the 16-char floor only affects UNI-022 emission",
    );

    let uni_022 = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("UNI-022"))
        .expect("UNI-022 synthetic present");
    assert_eq!(uni_022.severity, Severity::Important);

    assert_json_golden(&result, "ignore_directive_scenario_5_short_rationale.json");
}

/// Scenario 6 — orphan directive.
///
/// The directive references `UNI-999`, a rule id no finding on the
/// target line ever fires for. The URL finding stays `status: open`
/// (no match) and the directive pass mints a `UNI-023` synthetic
/// per RFC-33a §"Ignore directives" scope rules.
#[test]
fn scenario_6_orphan_directive_mints_uni_023_synthetic() {
    let scenario = Scenario {
        fixture_body: URL_WITH_ORPHAN_DIRECTIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: true,
        resolve_uni_023: true,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(findings.len(), 2, "open URL finding + UNI-023 synthetic");
    let url_finding = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("UNI-014"))
        .expect("URL finding present");
    assert!(url_finding.status.is_none(), "URL finding stays open — directive id did not match");
    assert!(url_finding.disposition.is_none());

    let uni_023 = findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("UNI-023"))
        .expect("UNI-023 synthetic present");
    assert_eq!(uni_023.severity, Severity::Important);

    assert_json_golden(&result, "ignore_directive_scenario_6_orphan.json");
    with_no_color(|| {
        assert_pretty_golden(&result, "ignore_directive_scenario_6_orphan_pretty.txt");
    });
}

/// Scenario 7 — graceful degradation.
///
/// Run the unrationaled-directive fixture from scenario 4 with the
/// resolved-codex slice missing both `UNI-022` and `UNI-023`. Per
/// RFC-33a §"Graceful degradation when the universal codex tree is
/// absent": match-and-stamp logic still runs (the URL finding flips
/// to `status: ignored`), but no synthetic findings are emitted and
/// the scanner does not error out. The on-wire envelope is exactly
/// what scenario 2 would produce if its rationale were missing — a
/// single matched finding with an empty rationale verbatim — and no
/// synthetic noise.
#[test]
fn scenario_7_graceful_degradation() {
    let scenario = Scenario {
        fixture_body: URL_WITH_UNRATIONED_DIRECTIVE_FIXTURE_BODY,
        primary_rule_id: "UNI-014",
        resolve_uni_022: false,
        resolve_uni_023: false,
    };
    let (result, findings) = run_scenario(&scenario);

    assert_eq!(
        findings.len(),
        1,
        "graceful degradation: matched URL finding only; no synthetics; got {findings:?}",
    );
    let f = &findings[0];
    assert_eq!(
        f.rule_id.as_deref(),
        Some("UNI-014"),
        "the matched URL finding survives; no UNI-022/UNI-023 noise",
    );
    assert_eq!(f.status, Some(specify_lints::rules::FindingStatus::Ignored));

    assert_json_golden(&result, "ignore_directive_scenario_7_graceful_degradation.json");
}
