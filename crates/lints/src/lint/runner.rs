//! Shared lint pipeline runner.
//!
//! Both lint surfaces (`specrun lint` and `specdev lint`) compose the
//! identical sequence: resolve the codex, index the workspace, run any
//! imperative producers, evaluate the declarative deterministic hints,
//! dedupe by fingerprint, apply the ignore-directive pass, fold in the
//! reserved-hint summary, and assemble the [`DiagnosticReport`] envelope.
//! This module owns that sequence so the two handlers stay thin and
//! cannot drift.
//!
//! The surfaces differ only in configuration — scan profile, tool
//! runner, resolver-degradation policy, and the producer set — which
//! [`PipelineConfig`] captures. Handler-specific concerns (artifact
//! scope composition, fallback-envelope emission, `lint-completed`
//! journalling, exit-code mapping) stay in the handlers.

use std::collections::HashSet;
use std::fmt;

use specify_error::Result;

use crate::lint::ScanProfile;
use crate::lint::diagnostics::{
    DiagnosticReport, DiagnosticReportVersion, DiagnosticSummary, emit_dump_model, map_index_error,
};
use crate::lint::eval::tool::ToolRunner;
use crate::lint::eval::{evaluate_rules, reserved_hint_summary};
use crate::lint::ignore::apply as apply_directives;
use crate::lint::index::build as build_model;
use crate::lint::producer::DiagnosticProducer;
use crate::rules::{
    Diagnostic, ResolveInputs, ResolvedRules, build_resolved_rules, map_resolve_error,
};

/// How the runner treats a codex-resolution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverDegradation {
    /// Surface the resolver error and abort the run (`specrun lint`).
    Fatal,
    /// Log the resolver error to stderr and continue with an empty
    /// declarative pass (`specdev lint`): the imperative `Check` pass
    /// already surfaces the codex-shape violations the resolver trips
    /// over, so the framework contributor's edit loop stays legible.
    SkipDeclarative,
}

/// Configuration for one [`run`] of the shared lint pipeline.
pub struct PipelineConfig<'a> {
    /// Indexer profile (`Consumer` for `specrun`, `Framework` for
    /// `specdev`).
    pub profile: ScanProfile,
    /// When set, emit the indexed `WorkspaceModel` and stop before the
    /// evaluator pass.
    pub dump_model: bool,
    /// Upgrade the reserved-hint summary severity from `optional` to
    /// `important`.
    pub strict_hints: bool,
    /// Apply the ignore-directive demotion pass. Always `true` for the
    /// lint surfaces; lifecycle gates set this `false` so validation
    /// stays non-silenceable.
    pub apply_ignore_directives: bool,
    /// Operator allow-list of `rule_id`s (empty means no filtering).
    pub rule_filter: &'a [&'a str],
    /// Resolver-failure policy for this surface.
    pub resolver_degradation: ResolverDegradation,
    /// Tool runner backing `kind: tool` hints.
    pub tool_runner: &'a dyn ToolRunner,
    /// Imperative producers composed ahead of the declarative pass.
    pub producers: &'a [&'a dyn DiagnosticProducer],
}

impl fmt::Debug for PipelineConfig<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The trait-object fields (`tool_runner`, `producers`) are not
        // `Debug`; surface their presence without their contents.
        f.debug_struct("PipelineConfig")
            .field("profile", &self.profile)
            .field("dump_model", &self.dump_model)
            .field("strict_hints", &self.strict_hints)
            .field("apply_ignore_directives", &self.apply_ignore_directives)
            .field("rule_filter", &self.rule_filter)
            .field("resolver_degradation", &self.resolver_degradation)
            .field("producer_count", &self.producers.len())
            .finish_non_exhaustive()
    }
}

/// Outcome of [`run`].
#[derive(Debug)]
pub enum RunOutcome {
    /// A fully composed envelope ready to render.
    Report(DiagnosticReport),
    /// `--dump-model` short-circuit; the model has already been
    /// emitted to stdout.
    DumpedModel,
}

/// Resolve, index, evaluate, and assemble one lint envelope.
///
/// # Errors
///
/// - [`map_resolve_error`] when resolution fails under
///   [`ResolverDegradation::Fatal`].
/// - [`map_index_error`] when the indexer walk fails.
/// - The hint-evaluator mapping when a deterministic hint fails.
/// - A render/serialise error when `--dump-model` emit fails.
pub fn run(inputs: &ResolveInputs<'_>, config: &PipelineConfig<'_>) -> Result<RunOutcome> {
    let resolved = match config.resolver_degradation {
        ResolverDegradation::Fatal => build_resolved_rules(inputs).map_err(map_resolve_error)?,
        ResolverDegradation::SkipDeclarative => match build_resolved_rules(inputs) {
            Ok(resolved) => resolved,
            Err(err) => {
                eprintln!("specdev lint: declarative pass skipped: {}", map_resolve_error(err));
                empty_resolved(inputs.target_adapter.to_string(), inputs.source_adapters.to_vec())
            }
        },
    };

    let model =
        build_model(inputs.project_dir, config.profile, inputs.artifact_paths, inputs.languages)
            .map_err(map_index_error)?;

    if config.dump_model {
        emit_dump_model(&model)?;
        return Ok(RunOutcome::DumpedModel);
    }

    let mut combined: Vec<Diagnostic> = Vec::new();
    for producer in config.producers {
        combined.extend(producer.produce(&model, inputs.project_dir));
    }

    // The declarative pass continues the `FIND-NNNN` id sequence past
    // the imperative producers so the two passes never collide on id.
    let start_id = u64::try_from(combined.len()).unwrap_or(u64::MAX).saturating_add(1);
    let (declarative, reserved, mut next_id) = evaluate_rules(
        &resolved.rules,
        &model,
        inputs.project_dir,
        config.tool_runner,
        start_id,
        config.rule_filter,
    )?;
    combined.extend(declarative);

    deduplicate_by_fingerprint(&mut combined);

    if config.apply_ignore_directives {
        let outcome =
            apply_directives(&mut combined, &model.ignore_directives, &resolved.rules, next_id);
        combined.extend(outcome.synthetics);
        next_id = outcome.next_id_counter;
    }

    if let Some(summary) = reserved_hint_summary(&reserved, config.strict_hints) {
        combined.push(summary);
    }
    // `next_id` is intentionally not consumed further in v1; future
    // post-passes (baseline matching, telemetry ids) continue to
    // thread it.
    let _ = next_id;

    let result = DiagnosticReport {
        version: DiagnosticReportVersion,
        summary: DiagnosticSummary::from_diagnostics(&combined),
        findings: combined,
    };
    Ok(RunOutcome::Report(result))
}

/// Deduplicate `findings` by canonical fingerprint, preserving
/// first-occurrence order. During the migration overlap a `CORE-*`
/// rule and its retiring imperative predicate produce byte-identical
/// fingerprints for the same `(rule-id, location)` pair; this collapse
/// keeps a single envelope row. Distinct findings never share a
/// fingerprint, so the pass is a no-op for the producer-free surface.
fn deduplicate_by_fingerprint(findings: &mut Vec<Diagnostic>) {
    let mut seen: HashSet<String> = HashSet::with_capacity(findings.len());
    findings.retain(|f| seen.insert(f.fingerprint.clone()));
}

/// Empty resolved-codex stub used when the declarative pass is skipped
/// because the codex tree itself failed to resolve. Keeps the eval
/// loop's input type uniform.
const fn empty_resolved(target_adapter: String, source_adapters: Vec<String>) -> ResolvedRules {
    ResolvedRules {
        version: 1,
        target_adapter,
        source_adapters,
        rules: Vec::new(),
    }
}
