//! `specify source {resolve}` — RFC-25 source adapter operations.
//!
//! Source adapters carry `axis: source` and the `enumerate` +
//! `extract` capabilities; they are loaded via
//! [`specify_domain::plugin::Plugin::resolve`] with [`Axis::Source`].

pub mod cli;

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_domain::plugin::{Axis, Plugin};
use specify_error::Result;

use crate::cli::Format;
use crate::output;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolveBody {
    axis: &'static str,
    name: String,
    resolved_path: String,
    location: &'static str,
    operations: Vec<String>,
    description: Option<String>,
}

fn write_resolve_text(w: &mut dyn Write, body: &ResolveBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.resolved_path)?;
    writeln!(w, "  axis: {}", body.axis)?;
    writeln!(w, "  name: {}", body.name)?;
    writeln!(w, "  location: {}", body.location)?;
    writeln!(w, "  operations: {}", body.operations.join(", "))?;
    if let Some(desc) = &body.description {
        writeln!(w, "  description: {desc}")?;
    }
    Ok(())
}

/// Resolve a source-adapter manifest by kebab name.
///
/// Probe order matches [`Plugin::resolve`]: agent-populated cache at
/// `<project_dir>/.specify/.cache/sources/<name>/` first, then the
/// in-repo `<project_dir>/sources/<name>/`. The resolved manifest's
/// kebab discriminants land in JSON envelopes via [`ResolveBody`].
pub fn resolve(format: Format, name: &str, project_dir: &Path) -> Result<()> {
    let resolved = Plugin::resolve(Axis::Source, name, project_dir)?;
    let body = ResolveBody {
        axis: Axis::Source.dir_segment(),
        name: resolved.manifest.name.clone(),
        resolved_path: resolved.root_dir.display().to_string(),
        location: resolved.location.label(),
        operations: resolved.manifest.operations.clone(),
        description: resolved.manifest.description.clone(),
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_resolve_text)?;
    Ok(())
}
