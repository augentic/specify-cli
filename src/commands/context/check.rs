//! `specify context check` handler.
//!
//! Owns the read-side drift detection: input fingerprints and the fenced
//! AGENTS.md body hash are compared against `.specify/context.lock`.
//! Write-side policy lives in [`super::generate`].

use std::io::Write;

use serde::Serialize;
use specify_error::Result;

use super::{context_lock_path, fences, fingerprint, lock, read_optional, render_document};
use crate::context::CommandContext;
use crate::output::{CliResult, Render, emit};

pub(super) fn run(ctx: &CommandContext) -> Result<CliResult> {
    let body = body(ctx)?;
    emit(ctx.format, &body)?;
    Ok(if body.status == "up-to-date" { CliResult::Success } else { CliResult::GenericFailure })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckBody {
    status: &'static str,
    fingerprint: CheckFingerprint,
    inputs_changed: Vec<String>,
    inputs_added: Vec<String>,
    inputs_removed: Vec<String>,
    fences_modified: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckFingerprint {
    expected: Option<String>,
    actual: Option<String>,
}

impl Render for CheckBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self.status {
            "up-to-date" => writeln!(w, "context up to date"),
            "context-not-generated" => writeln!(w, "context-not-generated: AGENTS.md is missing"),
            "context-lock-missing" => {
                writeln!(w, "context-lock-missing: .specify/context.lock is missing")
            }
            "drift" => {
                writeln!(w, "context drift detected")?;
                write_drift_list(w, "inputs changed", &self.inputs_changed)?;
                write_drift_list(w, "inputs added", &self.inputs_added)?;
                write_drift_list(w, "inputs removed", &self.inputs_removed)?;
                if self.fences_modified {
                    writeln!(w, "fences modified: true")?;
                }
                Ok(())
            }
            _ => writeln!(w, "context check finished"),
        }
    }
}

fn body(ctx: &CommandContext) -> Result<CheckBody> {
    let agents_path = ctx.project_dir.join("AGENTS.md");
    let agents = read_optional(&agents_path)?;
    let existing_lock = lock::load(&context_lock_path(ctx))?;
    let (_generated, actual_fingerprint) = render_document(ctx)?;
    let actual_lock = lock::ContextLock::from_fingerprint(&actual_fingerprint);

    if agents.is_none() {
        return Ok(CheckBody {
            status: "context-not-generated",
            fingerprint: check_fingerprint(existing_lock.as_ref(), Some(&actual_lock)),
            inputs_changed: Vec::new(),
            inputs_added: Vec::new(),
            inputs_removed: Vec::new(),
            fences_modified: false,
        });
    }

    let Some(expected_lock) = existing_lock else {
        return Ok(CheckBody {
            status: "context-lock-missing",
            fingerprint: check_fingerprint(None, Some(&actual_lock)),
            inputs_changed: Vec::new(),
            inputs_added: Vec::new(),
            inputs_removed: Vec::new(),
            fences_modified: false,
        });
    };

    let diff = lock::diff_inputs(&expected_lock.inputs, &actual_lock.inputs);
    let fences_modified = fences_modified(
        agents
            .as_deref()
            .expect("agents bytes are present because missing AGENTS.md returned above"),
        &expected_lock,
    );
    let has_input_drift =
        !diff.changed.is_empty() || !diff.added.is_empty() || !diff.removed.is_empty();
    let has_fingerprint_drift = expected_lock.fingerprint != actual_lock.fingerprint;
    let status = if has_fingerprint_drift || has_input_drift || fences_modified {
        "drift"
    } else {
        "up-to-date"
    };

    Ok(CheckBody {
        status,
        fingerprint: check_fingerprint(Some(&expected_lock), Some(&actual_lock)),
        inputs_changed: diff.changed,
        inputs_added: diff.added,
        inputs_removed: diff.removed,
        fences_modified,
    })
}

fn write_drift_list(w: &mut dyn Write, label: &str, paths: &[String]) -> std::io::Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    writeln!(w, "{label}: {}", paths.join(", "))
}

fn check_fingerprint(
    expected: Option<&lock::ContextLock>, actual: Option<&lock::ContextLock>,
) -> CheckFingerprint {
    CheckFingerprint {
        expected: expected.map(|lock| lock.fingerprint.clone()),
        actual: actual.map(|lock| lock.fingerprint.clone()),
    }
}

fn fences_modified(agents: &[u8], expected_lock: &lock::ContextLock) -> bool {
    match fences::parse_document(agents) {
        Ok(Some(current)) => {
            fingerprint::body_sha256(current.body()) != expected_lock.fences.body_sha256
        }
        Ok(None) | Err(_) => true,
    }
}
