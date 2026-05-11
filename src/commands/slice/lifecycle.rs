//! Slice lifecycle handlers: create / transition / archive / drop.

use std::io::Write;

use chrono::{DateTime, Utc};
use serde::Serialize;
use specify_domain::slice::{CreateIfExists, Created, LifecycleStatus, actions as slice_actions};
use specify_error::{Error, Result};

use crate::context::Ctx;
use crate::output::Render;

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
        slice_actions::create(&slices_dir, name, &capability_value, if_exists, Utc::now())?;

    ctx.write(&CreateBody::from(&outcome))?;
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

impl Render for CreateBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.created {
            writeln!(w, "Created slice {}", self.slice_dir)?;
        } else {
            writeln!(w, "Reusing existing slice {}", self.slice_dir)?;
        }
        if self.restarted {
            writeln!(w, "  (previous directory was removed)")?;
        }
        writeln!(w, "  capability: {}", self.capability)?;
        writeln!(w, "  status: {}", self.status)
    }
}

pub(super) fn transition(ctx: &Ctx, name: String, target: LifecycleStatus) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let metadata = slice_actions::transition(&slice_dir, target, Utc::now())?;
    ctx.write(&TransitionBody {
        name,
        status: metadata.status,
        defined_at: metadata.defined_at,
        build_started_at: metadata.build_started_at,
        completed_at: metadata.completed_at,
        merged_at: metadata.merged_at,
        dropped_at: metadata.dropped_at,
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionBody {
    name: String,
    status: LifecycleStatus,
    #[serde(with = "specify_domain::serde_rfc3339::option")]
    defined_at: Option<DateTime<Utc>>,
    #[serde(with = "specify_domain::serde_rfc3339::option")]
    build_started_at: Option<DateTime<Utc>>,
    #[serde(with = "specify_domain::serde_rfc3339::option")]
    completed_at: Option<DateTime<Utc>>,
    #[serde(with = "specify_domain::serde_rfc3339::option")]
    merged_at: Option<DateTime<Utc>>,
    #[serde(with = "specify_domain::serde_rfc3339::option")]
    dropped_at: Option<DateTime<Utc>>,
}

impl Render for TransitionBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}: status = {}", self.name, self.status)
    }
}

pub(super) fn archive(ctx: &Ctx, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let target = slice_actions::archive(&slice_dir, &archive_dir, Utc::now())?;
    ctx.write(&ArchiveBody {
        name,
        archive_path: target.display().to_string(),
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArchiveBody {
    name: String,
    archive_path: String,
}

impl Render for ArchiveBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}: archived to {}", self.name, self.archive_path)
    }
}

pub(super) fn discard_slice(ctx: &Ctx, name: String, reason: Option<&str>) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    let archive_dir = ctx.archive_dir();
    let (metadata, archive_path) =
        slice_actions::discard(&slice_dir, &archive_dir, reason, Utc::now())?;
    ctx.write(&DropBody {
        name,
        status: metadata.status,
        archive_path: archive_path.display().to_string(),
        drop_reason: metadata.drop_reason,
    })?;
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

impl Render for DropBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{}: dropped and archived to {}", self.name, self.archive_path)?;
        if let Some(r) = &self.drop_reason {
            writeln!(w, "  reason: {r}")?;
        }
        Ok(())
    }
}
