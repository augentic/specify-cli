//! `slice journal append | show` — append-only audit log at
//! `<slice_dir>/journal.yaml`.

use std::io::Write;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use specify_capability::Phase;
use specify_error::{Error, Result};
use specify_slice::{
    EntryKind, Journal, JournalEntry, Rfc3339Stamp, SliceMetadata, format_rfc3339,
};

use crate::context::Ctx;
use crate::output::Render;

pub(super) fn append(
    ctx: &Ctx, name: String, phase: Phase, kind: EntryKind, summary: String,
    context: Option<String>,
) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let timestamp = format_rfc3339(Utc::now());
    let entry = JournalEntry {
        timestamp: timestamp.clone(),
        step: phase,
        r#type: kind,
        summary,
        context,
    };

    Journal::append(&slice_dir, entry)?;

    ctx.out().write(&AppendBody {
        slice: name,
        phase: phase.to_string(),
        kind: kind.to_string(),
        timestamp,
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AppendBody {
    slice: String,
    phase: String,
    kind: String,
    timestamp: Rfc3339Stamp,
}

impl Render for AppendBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Appended {} entry to {}/journal.yaml.", self.kind, self.slice)
    }
}

pub(super) fn show(ctx: &Ctx, name: String) -> Result<()> {
    let slice_dir = ctx.slices_dir().join(&name);
    if !slice_dir.is_dir() || !SliceMetadata::path(&slice_dir).exists() {
        return Err(Error::SliceNotFound { name });
    }

    let journal = Journal::load(&slice_dir)?;
    let entries: Vec<EntryRow> = journal.entries.iter().map(EntryRow::from).collect();
    ctx.out().write(&ShowBody { name, entries })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShowBody {
    name: String,
    entries: Vec<EntryRow>,
}

impl Render for ShowBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.entries.is_empty() {
            return writeln!(w, "{}: no journal entries", self.name);
        }

        writeln!(w, "{}:", self.name)?;
        for entry in &self.entries {
            writeln!(
                w,
                "  [{}] {}/{} — {}",
                entry.timestamp, entry.phase, entry.kind, entry.summary,
            )?;
            if let Value::String(context) = &entry.context {
                for line in context.lines() {
                    writeln!(w, "      {line}")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryRow {
    timestamp: Rfc3339Stamp,
    phase: String,
    kind: String,
    summary: String,
    context: Value,
}

impl From<&JournalEntry> for EntryRow {
    fn from(entry: &JournalEntry) -> Self {
        Self {
            timestamp: entry.timestamp.clone(),
            phase: entry.step.to_string(),
            kind: entry.r#type.to_string(),
            summary: entry.summary.clone(),
            context: entry.context.clone().map_or(Value::Null, Value::from),
        }
    }
}
