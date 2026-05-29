//! `specrun source preview` handler — workflow-free source adapter
//! execution scaffolding (`specrun source preview` contract).
//!
//! Resolves the adapter, validates `--source`, scaffolds the output
//! directory with an `evidence/` subtree, and emits a summary of
//! adapter info, brief paths, and the source binding. The agent then
//! executes the briefs against the prepared environment.
//!
//! No `.specify/` writes, no journal events. Output lives entirely
//! under `--out` (default `.specify-preview/`).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_error::{Error, Result};
use specify_workflow::adapter::SourceAdapter;

use crate::runtime::cli::Format;
use crate::runtime::output;

const DEFAULT_OUT_DIR: &str = ".specify-preview";

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct BriefEntry {
    operation: String,
    path: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PreviewBody {
    adapter: String,
    version: u32,
    source: PathBuf,
    out: PathBuf,
    evidence_dir: PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    leads: Vec<String>,
    briefs: Vec<BriefEntry>,
}

pub fn preview(
    format: Format, adapter_name: &str, source: &Path, leads: &[String], out: Option<&Path>,
    project_dir: &Path,
) -> Result<()> {
    if !source.exists() {
        return Err(Error::Argument {
            flag: "--source",
            detail: format!("path does not exist: {}", source.display()),
        });
    }

    let resolved = SourceAdapter::resolve(adapter_name, project_dir)?;
    let adapter_dir = resolved.location.path().clone();

    let out_dir = out.map_or_else(|| PathBuf::from(DEFAULT_OUT_DIR), Path::to_path_buf);
    let evidence_dir = out_dir.join("evidence");
    std::fs::create_dir_all(&evidence_dir).map_err(Error::Io)?;

    let briefs: Vec<BriefEntry> = resolved
        .manifest
        .briefs
        .iter()
        .map(|(op, relative)| BriefEntry {
            operation: op.to_string(),
            path: adapter_dir.join(relative),
        })
        .collect();

    let body = PreviewBody {
        adapter: resolved.manifest.name,
        version: resolved.manifest.version,
        source: source.to_path_buf(),
        out: out_dir,
        evidence_dir,
        leads: leads.to_vec(),
        briefs,
    };

    output::emit(&mut std::io::stdout().lock(), format, &body, write_preview_text)?;
    Ok(())
}

fn write_preview_text(w: &mut dyn Write, body: &PreviewBody) -> std::io::Result<()> {
    writeln!(w, "adapter: {} v{}", body.adapter, body.version)?;
    writeln!(w, "source: {}", body.source.display())?;
    writeln!(w, "out: {}", body.out.display())?;
    writeln!(w, "evidence: {}", body.evidence_dir.display())?;
    if !body.leads.is_empty() {
        writeln!(w, "leads: {}", body.leads.join(", "))?;
    }
    writeln!(w, "briefs:")?;
    for brief in &body.briefs {
        writeln!(w, "  {}: {}", brief.operation, brief.path.display())?;
    }
    Ok(())
}
