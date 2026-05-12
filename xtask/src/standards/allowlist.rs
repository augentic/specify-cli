//! Per-file allowlist: TOML load, tightenable-diff, and `--tighten`
//! writer. The allowlist lives at `scripts/standards-allowlist.toml`;
//! each `[file."<rel>"]` table caps per-predicate violation counts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::types::{DEFAULT_LINE_CAP, FileBaseline};

pub(super) const ALLOWLIST: &str = "scripts/standards-allowlist.toml";

#[derive(Debug, Default, Deserialize)]
struct AllowlistRaw {
    #[serde(default)]
    file: BTreeMap<String, FileBaseline>,
}

pub(super) struct Allowlist {
    pub(super) files: BTreeMap<String, FileBaseline>,
}

impl Allowlist {
    pub(super) fn for_file(&self, rel: &str) -> FileBaseline {
        self.files.get(rel).cloned().unwrap_or_default()
    }
}

pub(super) fn load(path: &Path) -> std::io::Result<Allowlist> {
    if !path.exists() {
        return Ok(Allowlist {
            files: BTreeMap::new(),
        });
    }
    let text = fs::read_to_string(path)?;
    let raw: AllowlistRaw = toml::from_str(&text)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    Ok(Allowlist { files: raw.file })
}

/// Compute human-readable diff lines for any (file, predicate) where the
/// recorded baseline differs from today's actual count. For
/// `module-line-count` we accept moves in either direction (the baseline
/// is a pure `LoC` snapshot, so growth from a routine edit should
/// re-bake the baseline). For every other predicate we only surface
/// reductions — growth is a violation, not a tightenable diff.
pub(super) fn compute_rewrites(
    allowlist: &Allowlist, current: &BTreeMap<String, FileBaseline>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (rel, baseline) in &allowlist.files {
        seen.insert(rel.as_str());
        let actual = current.get(rel).cloned().unwrap_or_default();
        if &actual == baseline {
            continue;
        }
        for (key, baseline_val) in baseline_iter(baseline) {
            let actual_val = actual.allowed(key);
            if actual_val == baseline_val {
                continue;
            }
            if key == "module-line-count" || actual_val < baseline_val {
                out.push(format!("{rel}: {key} {baseline_val} → {actual_val}"));
            }
        }
    }
    // New files that exceed DEFAULT_LINE_CAP need an explicit module-line-count
    // entry; surface them so `--tighten` stamps a baseline.
    for (rel, actual) in current {
        if seen.contains(rel.as_str()) {
            continue;
        }
        if actual.module_line_count > DEFAULT_LINE_CAP {
            out.push(format!(
                "{rel}: module-line-count {DEFAULT_LINE_CAP} → {} (new file over default cap)",
                actual.module_line_count
            ));
        }
    }
    out
}

fn baseline_iter(b: &FileBaseline) -> impl Iterator<Item = (&'static str, u32)> + '_ {
    [
        ("inline-dtos", b.inline_dtos),
        ("format-match-dispatch", b.format_match_dispatch),
        ("rfc-numbers-in-code", b.rfc_numbers_in_code),
        ("ritual-doc-paragraphs", b.ritual_doc_paragraphs),
        ("no-op-forwarders", b.no_op_forwarders),
        ("error-envelope-inlined", b.error_envelope_inlined),
        ("path-helper-inlined", b.path_helper_inlined),
        ("direct-fs-write", b.direct_fs_write),
        ("stale-cli-vocab", b.stale_cli_vocab),
        ("module-line-count", b.module_line_count),
        ("result-cliresult-default", b.result_cliresult_default),
        ("verbose-doc-paragraphs", b.verbose_doc_paragraphs),
        ("cli-help-shape", b.cli_help_shape),
        ("display-serde-mirror", b.display_serde_mirror),
        ("crate-root-prose", b.crate_root_prose),
        ("unit-test-serde-roundtrip", b.unit_test_serde_roundtrip),
    ]
    .into_iter()
}

/// Serialise `current` back to `path` as TOML, skipping rows where every
/// field equals its zero-default. Output is alphabetised by file path.
pub(super) fn write(path: &Path, current: &BTreeMap<String, FileBaseline>) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str(
        "# Per-file baselines for `cargo run -p xtask -- standards-check`.\n\
         #\n\
         # Each `[file.\"<rel-path>\"]` table caps the number of violations of each\n\
         # predicate for that file. A live count strictly greater than the\n\
         # baseline fails CI; missing predicates default to zero (new files\n\
         # start clean) except `module-line-count`, which defaults to 400.\n\
         # Reductions are encouraged in any PR that touches a file; the CI\n\
         # `--check-tightenable` mode fails when an unrelated PR could lower a\n\
         # baseline without code changes.\n\
         #\n\
         # Predicate definitions live in `xtask/src/standards.rs`. AGENTS.md\n\
         # §Mechanical enforcement explains what each predicate enforces and how\n\
         # to drive its baselines down.\n\n",
    );
    for (rel, baseline) in current {
        if baseline.is_empty() {
            continue;
        }
        let _ = writeln!(out, "[file.\"{rel}\"]");
        for (key, value) in baseline_iter(baseline) {
            if value == 0 {
                continue;
            }
            let _ = writeln!(out, "{key} = {value}");
        }
        out.push('\n');
    }
    fs::write(path, out)
}
