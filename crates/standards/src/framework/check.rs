//! Framework authoring predicates.
//! No imperative `Check` predicate runs as a `specify lint framework`
//! producer, and the `kind: authoring-predicate` bridge is gone — every
//! framework rule resolves through declarative hints or referenced WASI
//! tools. The only surviving predicates are the Rust-quality
//! predicates ([`RustTestNaming`], [`RustSourceQuality`]), run through
//! [`run_rust_quality`] for this repo's own `cargo test --test
//! rust_quality` gate.

pub mod brief;
pub mod rust_source;
pub mod rust_test_naming;

use std::path::Path;

pub use rust_source::RustSourceQuality;
pub use rust_test_naming::RustTestNaming;
use specify_diagnostics::{Diagnostic, fingerprint};

use crate::framework::context::Context;

/// A check predicate that scans the framework repo and returns
/// [`Diagnostic`]s. The predicates need a `&Context` (framework root +
/// schema cache), which the declarative
/// [`crate::lint::producer::DiagnosticProducer`] contract does not
/// provide, so this trait survives the finding-type unification — only
/// its return type changed from the deleted lightweight `Finding`.
pub trait Check {
    /// Scan `ctx` and return this predicate's findings. Locations are
    /// absolute (anchored at the canonicalised framework root) and
    /// `id` / `fingerprint` are left unset for the `finalize` pass.
    fn run(&self, ctx: &Context) -> Vec<Diagnostic>;
}

/// Rust-quality predicates for the specify-cli repo (`RustTestNaming`,
/// `RustSourceQuality`). No-op on plugin framework roots.
pub fn run_rust_quality(ctx: &Context) -> Vec<Diagnostic> {
    let checks: [&dyn Check; 2] = [&RustTestNaming, &RustSourceQuality];
    let mut findings = Vec::new();
    for check in checks {
        findings.extend(check.run(ctx));
    }
    finalize(&mut findings, ctx.framework_root());
    findings
}

/// Finalise a batch of predicate findings into ready-to-render
/// [`Diagnostic`]s: rebase each `location.path` to project-relative
/// form, sort deterministically, then compute fingerprints and assign
/// sequential `FIND-NNNN` ids.
///
/// The fingerprint preimage excludes `id`, so hashing before assigning
/// ids is safe. Rebasing before hashing is required because the
/// imperative predicates emit absolute paths anchored at the
/// canonicalised framework root, while `diagnostic.schema.json`
/// constrains `location.path` to project-relative forward-slash
/// strings.
fn finalize(findings: &mut [Diagnostic], framework_root: &Path) {
    let prefix = framework_root.to_string_lossy().replace('\\', "/");
    for finding in findings.iter_mut() {
        if let Some(location) = finding.location.as_mut() {
            let normalised = location.path.replace('\\', "/");
            if let Some(rest) = normalised.strip_prefix(&prefix) {
                location.path = rest.trim_start_matches('/').to_string();
            } else {
                location.path = normalised;
            }
        }
    }

    findings.sort_by(|a, b| {
        let a_path = a.location.as_ref().map(|l| l.path.as_str());
        let b_path = b.location.as_ref().map(|l| l.path.as_str());
        let a_line = a.location.as_ref().and_then(|l| l.line);
        let b_line = b.location.as_ref().and_then(|l| l.line);
        (&a.rule_id, a_path, a_line, &a.title).cmp(&(&b.rule_id, b_path, b_line, &b.title))
    });

    for (index, finding) in findings.iter_mut().enumerate() {
        finding.fingerprint = fingerprint(finding);
        finding.id = format!("FIND-{:04}", index + 1);
    }
}
