//! Handler for the `source resolve --explain` fingerprint-chain reader
//! (extraction cache fingerprint contract).

use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::adapter as adapter_mod;
use specify_domain::adapter::{CacheLayout, SourceOperation};
use specify_error::Result;

use crate::runtime::cli::Format;
use crate::runtime::output;

/// One row emitted for `specrun source resolve --explain`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ExplainRow {
    #[serde(with = "specify_error::serde_rfc3339")]
    timestamp: Timestamp,
    fingerprint: String,
    slice: String,
    source_key: String,
    operation: SourceOperation,
}

/// Envelope returned by `--explain`.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct ExplainBody {
    adapter: String,
    index_path: PathBuf,
    entries: Vec<ExplainRow>,
}

/// Dispatch for `specrun source resolve --explain`.
pub fn explain(format: Format, adapter: &str, project_dir: &Path) -> Result<()> {
    let layout = CacheLayout::new(project_dir, adapter);
    let entries = adapter_mod::cache_read_index(layout)?;
    let body = ExplainBody {
        adapter: adapter.to_string(),
        index_path: layout.index_path(),
        entries: entries
            .into_iter()
            .map(|e| ExplainRow {
                timestamp: e.timestamp,
                fingerprint: e.fingerprint,
                slice: e.slice,
                source_key: e.source_key,
                operation: e.operation,
            })
            .collect(),
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_explain_text)?;
    Ok(())
}

fn write_explain_text(w: &mut dyn Write, body: &ExplainBody) -> std::io::Result<()> {
    writeln!(w, "adapter: {}", body.adapter)?;
    writeln!(w, "index: {}", body.index_path.display())?;
    if body.entries.is_empty() {
        writeln!(w, "  (no cache writes recorded yet)")?;
        return Ok(());
    }
    for entry in &body.entries {
        writeln!(
            w,
            "  {ts} {op} {slice}/{key} {fp}",
            ts = entry.timestamp,
            op = entry.operation,
            slice = entry.slice,
            key = entry.source_key,
            fp = entry.fingerprint
        )?;
    }
    Ok(())
}
