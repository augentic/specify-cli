//! Real-rules smoke test for the CH-11 codex frontmatter parser.
//!
//! Walks every `*.md` rule under `adapters/shared/rules/universal/`
//! and `adapters/sources/documentation/rules/` in the sibling
//! `augentic/specify` plugin checkout and asserts that
//! [`specify_lints::parse_rule_file`] returns a
//! [`specify_lints::Rule`] for each one. Skips
//! cleanly when the plugin checkout is not available so the test
//! is safe to run from any environment.
//!
//! The conventional sibling layout is:
//!
//! ```text
//! github.com/augentic/specify/        <- plugin repo (rule sources)
//! github.com/augentic/specify-cli/    <- this repo
//! ```

use std::path::{Path, PathBuf};

use specify_lints::parse_rule_file;

/// Conventional sibling path to the plugin repo's adapter tree.
fn plugin_adapters_dir() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidate = manifest_dir.ancestors().nth(3)?.join("specify").join("adapters");
    candidate.is_dir().then_some(candidate)
}

fn collect_rule_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.eq_ignore_ascii_case("readme.md"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn parses_every_universal_codex_rule() {
    let Some(adapters) = plugin_adapters_dir() else {
        eprintln!("skip: sibling augentic/specify plugin checkout not found");
        return;
    };
    let rules_dir = adapters.join("shared").join("rules").join("universal");
    let files = collect_rule_files(&rules_dir);
    if files.is_empty() {
        eprintln!("skip: no rules under {}", rules_dir.display());
        return;
    }
    for path in files {
        let rule = parse_rule_file(&path)
            .unwrap_or_else(|err| panic!("parse {} failed: {err}", path.display()));
        assert!(!rule.id.is_empty(), "{}: parsed rule has empty id", path.display());
        assert!(
            rule.body.starts_with("## Rule\n") || rule.body.contains("\n## Rule\n"),
            "{}: body must contain '## Rule' heading",
            path.display()
        );
    }
}

#[test]
fn parses_every_documentation_source_codex_rule() {
    let Some(adapters) = plugin_adapters_dir() else {
        eprintln!("skip: sibling augentic/specify plugin checkout not found");
        return;
    };
    let rules_dir = adapters.join("sources").join("documentation").join("rules");
    let files = collect_rule_files(&rules_dir);
    if files.is_empty() {
        eprintln!("skip: no rules under {}", rules_dir.display());
        return;
    }
    for path in files {
        let rule = parse_rule_file(&path)
            .unwrap_or_else(|err| panic!("parse {} failed: {err}", path.display()));
        assert!(
            rule.id.starts_with("SRC-"),
            "{}: expected SRC-* id, got {}",
            path.display(),
            rule.id
        );
        assert!(
            rule.body.starts_with("## Rule\n") || rule.body.contains("\n## Rule\n"),
            "{}: body must contain '## Rule' heading",
            path.display()
        );
    }
}
