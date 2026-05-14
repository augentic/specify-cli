//! Slice lifecycle handlers: create / transition / archive / drop.

use std::io::Write;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::slice::{CreateIfExists, Created, LifecycleStatus, actions as slice_actions};
use specify_error::{Error, Result};

use crate::context::Ctx;

pub(super) fn create(
    ctx: &Ctx, name: &str, capability: Option<String>, if_exists: CreateIfExists,
) -> Result<()> {
    let capability_value = capability.map_or_else(
        || {
            ctx.config.capability.clone().ok_or_else(|| Error::Diag {
                code: "slice-create-capability-missing",
                detail: "no project capability declared; pass `--capability <id>` explicitly or \
                         run `specify init <capability>` first (hub projects cannot create \
                         changes)"
                    .to_string(),
            })
        },
        Ok,
    )?;
    let slices_dir = ctx.slices_dir();
    std::fs::create_dir_all(&slices_dir)?;

    let outcome =
        slice_actions::create(&slices_dir, name, &capability_value, if_exists, Timestamp::now())?;

    ctx.write(&CreateBody::from(&outcome), write_create_text)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    slice_dir: String,
    status: LifecycleStatus,
    capability: String,
    created: bool,
    restarted: bool,
}

impl From<&Created> for CreateBody {
    fn from(outcome: &Created) -> Self {
        Self {
            name: outcome.dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            slice_dir: outcome.dir.display().to_string(),
            status: outcome.metadata.status,
            capability: outcome.metadata.capability.clone(),
            created: outcome.created,
            restarted: outcome.restarted,
        }
    }
}

fn write_create_text(w: &mut dyn Write, body: &CreateBody) -> std::io::Result<()> {
    if body.created {
        writeln!(w, "Created slice {}", body.slice_dir)?;
    } else {
        writeln!(w, "Reusing existing slice {}", body.slice_dir)?;
    }
    if body.restarted {
        writeln!(w, "  (previous directory was removed)")?;
    }
    writeln!(w, "  capability: {}", body.capability)?;
    writeln!(w, "  status: {}", body.status)
}

pub(super) fn transition(ctx: &Ctx, name: String, target: LifecycleStatus) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = slice_actions::transition(&slice_dir, target, Timestamp::now())?;
    ctx.write(
        &TransitionBody {
            name,
            status: metadata.status,
            defined_at: metadata.defined_at,
            build_started_at: metadata.build_started_at,
            completed_at: metadata.completed_at,
            merged_at: metadata.merged_at,
            dropped_at: metadata.dropped_at,
        },
        write_transition_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionBody {
    name: String,
    status: LifecycleStatus,
    #[serde(with = "specify_error::serde_rfc3339")]
    defined_at: Option<Timestamp>,
    #[serde(with = "specify_error::serde_rfc3339")]
    build_started_at: Option<Timestamp>,
    #[serde(with = "specify_error::serde_rfc3339")]
    completed_at: Option<Timestamp>,
    #[serde(with = "specify_error::serde_rfc3339")]
    merged_at: Option<Timestamp>,
    #[serde(with = "specify_error::serde_rfc3339")]
    dropped_at: Option<Timestamp>,
}

fn write_transition_text(w: &mut dyn Write, body: &TransitionBody) -> std::io::Result<()> {
    writeln!(w, "{}: status = {}", body.name, body.status)
}

pub(super) fn discard_slice(ctx: &Ctx, name: String, reason: Option<&str>) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let (metadata, archive_path) =
        slice_actions::discard(&slice_dir, &archive_dir, reason, Timestamp::now())?;
    ctx.write(
        &DropBody {
            name,
            status: metadata.status,
            archive_path: archive_path.display().to_string(),
            drop_reason: metadata.drop_reason,
        },
        write_drop_text,
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct DropBody {
    name: String,
    status: LifecycleStatus,
    archive_path: String,
    drop_reason: Option<String>,
}

fn write_drop_text(w: &mut dyn Write, body: &DropBody) -> std::io::Result<()> {
    writeln!(w, "{}: dropped and archived to {}", body.name, body.archive_path)?;
    if let Some(r) = &body.drop_reason {
        writeln!(w, "  reason: {r}")?;
    }
    Ok(())
}
