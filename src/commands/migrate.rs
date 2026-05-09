//! Hidden one-shot migrators kept for one minor release after a
//! breaking on-disk rename, then removed.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify::Error;

use crate::cli::{MigrateAction, OutputFormat};
use crate::output::{CliResult, Render, emit};

pub fn run(format: OutputFormat, action: MigrateAction) -> Result<CliResult, Error> {
    match action {
        MigrateAction::CapabilityNoun { dry_run } => capability_noun(format, dry_run),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CapabilityNounBody {
    dry_run: bool,
    rewritten: Vec<String>,
}

impl Render for CapabilityNounBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.rewritten.is_empty() {
            return writeln!(w, "No legacy `schema:` keys found.");
        }
        let verb = if self.dry_run { "Would rewrite" } else { "Rewrote" };
        for p in &self.rewritten {
            writeln!(w, "{verb} {p}")?;
        }
        Ok(())
    }
}

/// Rewrite legacy `schema:` keys to `capability:` in `registry.yaml`,
/// `plan.yaml`, and archived `plan-*.yaml` under `.specify/archive/plans/`.
fn capability_noun(format: OutputFormat, dry_run: bool) -> Result<CliResult, Error> {
    let root = std::env::current_dir().map_err(Error::Io)?;
    let mut rewritten = Vec::new();

    for path in candidates(&root) {
        if rewrite_one(&path, dry_run)? {
            rewritten
                .push(path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().into_owned());
        }
    }
    rewritten.sort();

    emit(format, &CapabilityNounBody { dry_run, rewritten })?;
    Ok(CliResult::Success)
}

fn candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.join("registry.yaml"), root.join("plan.yaml")];
    let archive = root.join(".specify").join("archive").join("plans");
    if let Ok(entries) = fs::read_dir(&archive) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "yaml") {
                out.push(path);
            }
        }
    }
    out
}

/// Rewrite one file in place. Returns `true` when the file was (or would
/// be) changed. Idempotent — already-migrated files report `false`.
fn rewrite_one(path: &Path, dry_run: bool) -> Result<bool, Error> {
    let Ok(original) = fs::read_to_string(path) else {
        return Ok(false);
    };
    let updated = rewrite(&original);
    if updated == original {
        return Ok(false);
    }
    if !dry_run {
        fs::write(path, updated).map_err(Error::Io)?;
    }
    Ok(true)
}

/// Replace `schema:` with `capability:` only when it appears as a YAML
/// mapping key (start of line or after whitespace, followed by `:`).
fn rewrite(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("schema:") {
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str("capability:");
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_top_level_and_nested_keys() {
        let input = "version: 1\nprojects:\n  - name: a\n    schema: omnia@v1\nschema: hub\n";
        let expected =
            "version: 1\nprojects:\n  - name: a\n    capability: omnia@v1\ncapability: hub\n";
        assert_eq!(rewrite(input), expected);
    }

    #[test]
    fn leaves_capability_lines_untouched() {
        let input = "    capability: omnia@v1\n";
        assert_eq!(rewrite(input), input);
    }

    #[test]
    fn does_not_rewrite_substring_in_value() {
        let input = "description: schemas folder\n";
        assert_eq!(rewrite(input), input);
    }
}
