//! `specify context generate` handler.
//!
//! Owns the write-side policy: the fenced AGENTS.md plan, the
//! `.specify/context.lock` write, and the JSON `GenerateBody` envelope.
//! Read-side fingerprint comparison lives in [`super::check`].

use std::io::Write;

use serde::Serialize;
use specify_config::is_workspace_clone;
use specify_error::Result;
use specify_slice::atomic::bytes_write;

use super::{
    context_lock_path, diag, error_from_fence, fences, lock, read_optional, render_document,
};
use crate::context::Ctx;
use crate::output::Render;

const WOULD_UPDATE_MSG: &str =
    "context is out of date; run `specify context generate` to refresh it";

pub(super) fn run(ctx: &Ctx, check: bool, force: bool) -> Result<()> {
    if is_workspace_clone(&ctx.project_dir) {
        return Err(diag(
            "context-workspace-clone-refused",
            format!(
                "specify context generate: refusing to run inside a workspace clone at {}; \
                 run context generation in the owning project instead",
                ctx.project_dir.display()
            ),
        ));
    }

    let body = body(ctx, check, force)?;
    let would_update = check && body.changed;
    ctx.out().write(&body)?;
    if would_update { Err(diag("context-would-update", WOULD_UPDATE_MSG)) } else { Ok(()) }
}

pub(in crate::commands) fn for_init(ctx: &Ctx) -> Result<Outcome> {
    body(ctx, false, false).map(|b| Outcome {
        changed: b.changed,
        disposition: b.disposition,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::commands) struct Outcome {
    pub(in crate::commands) changed: bool,
    pub(in crate::commands) disposition: &'static str,
}

fn body(ctx: &Ctx, check: bool, force: bool) -> Result<GenerateBody> {
    let (generated, context_fingerprint) = render_document(ctx)?;
    let expected_lock = lock::ContextLock::from_fingerprint(&context_fingerprint);
    let lock_path = context_lock_path(ctx);
    let existing_lock = lock::load(&lock_path)?;
    let agents_path = ctx.project_dir.join("AGENTS.md");
    let existing = read_optional(&agents_path)?;
    if !check {
        refuse_modified_fenced_body(existing.as_deref(), existing_lock.as_ref(), force)?;
    }
    let planned = fences::plan_agents_write(existing.as_deref(), generated.as_bytes(), force)
        .map_err(error_from_fence)?;
    let agents_changed = planned.disposition != fences::WriteDisposition::Unchanged;
    let lock_changed = existing_lock.as_ref() != Some(&expected_lock);
    let changed = agents_changed || lock_changed;

    if agents_changed && !check {
        bytes_write(&agents_path, &planned.bytes)?;
    }
    if lock_changed && !check {
        lock::save(&lock_path, &expected_lock)?;
    }

    let status = match (check, changed) {
        (true, true) => "would-update",
        (_, false) => "unchanged",
        (false, true) => "written",
    };
    let disposition = match planned.disposition {
        fences::WriteDisposition::Create => "create",
        fences::WriteDisposition::ForceRewriteUnfenced => "force-rewrite-unfenced",
        fences::WriteDisposition::ReplaceFencedBlock => "replace-fenced-block",
        fences::WriteDisposition::Unchanged => "unchanged",
    };
    Ok(GenerateBody {
        status,
        path: "AGENTS.md",
        check,
        force,
        changed,
        agents_changed,
        lock_changed,
        disposition,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI JSON response mirrors independent check flags and write outcomes."
)]
struct GenerateBody {
    status: &'static str,
    path: &'static str,
    check: bool,
    force: bool,
    changed: bool,
    agents_changed: bool,
    lock_changed: bool,
    disposition: &'static str,
}

impl Render for GenerateBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self.status {
            "would-update" => writeln!(w, "{WOULD_UPDATE_MSG}"),
            "unchanged" => writeln!(w, "AGENTS.md is up to date"),
            "written" if self.agents_changed => writeln!(w, "wrote AGENTS.md"),
            "written" => writeln!(w, "wrote .specify/context.lock"),
            _ => writeln!(w, "context generate finished"),
        }
    }
}

fn refuse_modified_fenced_body(
    agents: Option<&[u8]>, existing_lock: Option<&lock::ContextLock>, force: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    let (Some(agents), Some(existing_lock)) = (agents, existing_lock) else {
        return Ok(());
    };
    let Some(current) = fences::parse_document(agents).map_err(error_from_fence)? else {
        return Ok(());
    };
    let actual_body = super::fingerprint::body_sha256(current.body());
    if actual_body != existing_lock.fences.body_sha256 {
        return Err(diag(
            "context-fenced-content-modified",
            "AGENTS.md drifted from .specify/context.lock",
        ));
    }
    Ok(())
}
