//! `slice journal append | show` — append-only audit log at
//! `<slice_dir>/journal.yaml`.

use std::io::Write;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::capability::Phase;
use specify_domain::slice::{EntryKind, Journal, JournalEntry, SliceMetadata};
use specify_error::{Error, Result};

use crate::context::Ctx;

pub(super) fn append(
    ctx: &Ctx, name: String, phase: Phase, kind: EntryKind, summary: String,
    context: Option<String>,
) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let timestamp = Timestamp::now();
    let entry = JournalEntry {
        timestamp,
        step: phase,
        r#type: kind,
        summary,
        context,
    };

    Journal::append(&slice_dir, entry)?;

    ctx.write(
        &AppendBody {
            slice: name,
            phase: phase.to_string(),
            kind: kind.to_string(),
            timestamp,
        },
        write_append_text,
    )?;
    Ok(())
}

fn write_append_text(w: &mut dyn Write, body: &AppendBody) -> std::io::Result<()> {
    writeln!(w, "Appended {} entry to {}/journal.yaml.", body.kind, body.slice)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppendBody {
    slice: String,
    phase: String,
    kind: String,
    #[serde(with = "specify_error::serde_rfc3339")]
    timestamp: Timestamp,
}

pub(super) fn show(ctx: &Ctx, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let journal = Journal::load(&slice_dir)?;
    ctx.write(
        &ShowBody {
            name,
            entries: journal.entries,
        },
        write_show_text,
    )?;
    Ok(())
}

fn write_show_text(w: &mut dyn Write, body: &ShowBody) -> std::io::Result<()> {
    if body.entries.is_empty() {
        return writeln!(w, "{}: no journal entries", body.name);
    }

    writeln!(w, "{}:", body.name)?;
    for entry in &body.entries {
        writeln!(
            w,
            "  [{}] {}/{} — {}",
            entry.timestamp.strftime("%Y-%m-%dT%H:%M:%SZ"),
            entry.step,
            entry.r#type,
            entry.summary,
        )?;
        if let Some(context) = &entry.context {
            for line in context.lines() {
                writeln!(w, "      {line}")?;
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody {
    name: String,
    entries: Vec<JournalEntry>,
}
