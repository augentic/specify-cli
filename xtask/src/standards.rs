//! Standards-check engine.
//!
//! Each predicate counts a violation per Rust source file. Per-file
//! baselines live in `scripts/standards-allowlist.toml`. A file's live
//! count must not exceed its baseline; the baseline defaults to 0 when
//! omitted (i.e. new files start clean).
//!
//! Predicates:
//!
//! - `inline-dtos` — `#[derive(Serialize)]` declared inside any
//!   `Block` (function bodies, match arms, etc.). AST-based; reliably
//!   sees DTOs defined in match arms that the prior bash regex missed.
//! - `format-match-dispatch` — `match … format { Json => … }`. Should
//!   route through `Render::render_text` + `emit` instead.
//! - `free-form-error-strings` — `Error::(Config|Merge|ToolResolver|
//!   ToolRuntime|CapabilityResolution)(`. Replaced by
//!   `Error::Diag { code, detail }`.
//! - `rfc-numbers-in-code` — `RFC[- ]?\d+` outside `tests/`,
//!   `DECISIONS.md`, and `rfcs/`.
//! - `ritual-doc-paragraphs` — the boilerplate `Returns an error if
//!   the operation fails.` doc paragraph.
//! - `no-op-forwarders` — `let _ = cli.<flag>;` style ignores of
//!   parsed-but-unused flags.
//! - `name-suffix-duplication` — `fn foo_<module>` inside `mod
//!   <module>` (e.g. `fn show_registry` in `commands/registry.rs`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;
use syn::visit::Visit;
use walkdir::WalkDir;

const ALLOWLIST: &str = "scripts/standards-allowlist.toml";

/// Run every predicate against `root` and report. Returns `Ok(true)`
/// when every file is at or below its baseline, `Ok(false)` if any
/// regression, `Err(_)` on I/O / parse failure.
pub fn run(root: &Path) -> std::io::Result<bool> {
    let allowlist = load_allowlist(&root.join(ALLOWLIST))?;
    let files = rust_files(root);

    let mut report = Report::default();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().into_owned();
        let source = fs::read_to_string(path)?;
        let counts = count_one(path, &source);
        let baseline = allowlist.for_file(&rel_str);
        report.merge(&rel_str, &counts, &baseline);
    }

    report.print();
    Ok(report.passed)
}

#[derive(Default, Debug)]
struct Counts {
    inline_dtos: u32,
    format_match_dispatch: u32,
    free_form_error_strings: u32,
    rfc_numbers_in_code: u32,
    ritual_doc_paragraphs: u32,
    no_op_forwarders: u32,
    name_suffix_duplication: u32,
}

impl Counts {
    fn iter(&self) -> impl Iterator<Item = (&'static str, u32)> {
        [
            ("inline-dtos", self.inline_dtos),
            ("format-match-dispatch", self.format_match_dispatch),
            ("free-form-error-strings", self.free_form_error_strings),
            ("rfc-numbers-in-code", self.rfc_numbers_in_code),
            ("ritual-doc-paragraphs", self.ritual_doc_paragraphs),
            ("no-op-forwarders", self.no_op_forwarders),
            ("name-suffix-duplication", self.name_suffix_duplication),
        ]
        .into_iter()
    }
}

fn count_one(path: &Path, source: &str) -> Counts {
    let mut c = Counts::default();
    if let Ok(file) = syn::parse_file(source) {
        let mut visitor = InlineDtoVisitor { hits: 0, depth: 0 };
        visitor.visit_file(&file);
        c.inline_dtos = visitor.hits;
    }
    let stripped = strip_comments(source);
    c.format_match_dispatch = count_regex(&FORMAT_MATCH_RE, &stripped);
    c.free_form_error_strings = count_regex(&FREE_FORM_ERROR_RE, &stripped);
    c.rfc_numbers_in_code = count_regex(&RFC_RE, source);
    c.ritual_doc_paragraphs = count_regex(&RITUAL_DOC_RE, source);
    c.no_op_forwarders = count_regex(&NO_OP_FORWARDER_RE, &stripped);
    c.name_suffix_duplication = count_name_suffix(path, &stripped);
    c
}

// ---------------------------------------------------------------------
// AST: inline-dtos — Serialize derive inside any Block.

struct InlineDtoVisitor {
    hits: u32,
    depth: u32,
}

impl InlineDtoVisitor {
    fn has_serialize(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            if !a.path().is_ident("derive") {
                return false;
            }
            let mut found = false;
            let _ = a.parse_nested_meta(|meta| {
                if meta.path.is_ident("Serialize") {
                    found = true;
                }
                Ok(())
            });
            found
        })
    }
}

impl<'ast> Visit<'ast> for InlineDtoVisitor {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.depth += 1;
        syn::visit::visit_block(self, node);
        self.depth -= 1;
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_enum(self, node);
    }
}

// ---------------------------------------------------------------------
// Regex predicates.

fn count_regex(re: &Regex, text: &str) -> u32 {
    u32::try_from(re.find_iter(text).count()).unwrap_or(u32::MAX)
}

static FORMAT_MATCH_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"match\s+(?:ctx\.|self\.)?format\s*\{").expect("static regex")
});

static FREE_FORM_ERROR_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"Error::(?:Config|Merge|ToolResolver|ToolRuntime|CapabilityResolution)\(")
        .expect("static regex")
});

static RFC_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"RFC[- ]?\d+").expect("static regex"));

static RITUAL_DOC_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"///\s*Returns an error if the operation fails\.").expect("static regex")
});

static NO_OP_FORWARDER_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"let\s+_\s*=\s*cli\.\w+\s*;").expect("static regex"));

// ---------------------------------------------------------------------
// Name-suffix duplication: fn foo_<module> in mod <module>.

fn count_name_suffix(path: &Path, source: &str) -> u32 {
    let Some(module) = module_name(path) else {
        return 0;
    };
    let pattern = format!(r"fn\s+\w+_{module}\b");
    let re = Regex::new(&pattern).expect("dynamic regex");
    count_regex(&re, source)
}

fn module_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    if stem == "mod" || stem == "lib" || stem == "main" {
        path.parent()?.file_name().map(|n| n.to_string_lossy().into_owned())
    } else {
        Some(stem.into_owned())
    }
}

// ---------------------------------------------------------------------
// Comment stripping (for predicates that must ignore prose).

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for nc in chars.by_ref() {
                    if prev == '*' && nc == '/' {
                        break;
                    }
                    if nc == '\n' {
                        out.push('\n');
                    }
                    prev = nc;
                }
            }
            '"' => {
                out.push(c);
                let mut escape = false;
                for nc in chars.by_ref() {
                    out.push(nc);
                    if escape {
                        escape = false;
                    } else if nc == '\\' {
                        escape = true;
                    } else if nc == '"' {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------
// File discovery.

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in ["src", "crates"] {
        let dir = root.join(parent);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&dir).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // Skip integration test dirs and generated/target output.
            let rel = path.strip_prefix(root).unwrap_or(path);
            let rel_str = rel.to_string_lossy();
            if rel_str.starts_with("target/") || rel_str.contains("/target/") {
                continue;
            }
            if rel_str.contains("/tests/") || rel_str.ends_with("/tests.rs") {
                // Tests are exempt from the standards-check (per
                // existing AGENTS.md guidance).
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------
// Allowlist (per-file TOML).

#[derive(Debug, Default, Deserialize)]
struct AllowlistRaw {
    #[serde(default)]
    file: BTreeMap<String, FileBaseline>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
struct FileBaseline {
    #[serde(default)]
    inline_dtos: u32,
    #[serde(default)]
    format_match_dispatch: u32,
    #[serde(default)]
    free_form_error_strings: u32,
    #[serde(default)]
    rfc_numbers_in_code: u32,
    #[serde(default)]
    ritual_doc_paragraphs: u32,
    #[serde(default)]
    no_op_forwarders: u32,
    #[serde(default)]
    name_suffix_duplication: u32,
}

impl FileBaseline {
    fn allowed(&self, key: &str) -> u32 {
        match key {
            "inline-dtos" => self.inline_dtos,
            "format-match-dispatch" => self.format_match_dispatch,
            "free-form-error-strings" => self.free_form_error_strings,
            "rfc-numbers-in-code" => self.rfc_numbers_in_code,
            "ritual-doc-paragraphs" => self.ritual_doc_paragraphs,
            "no-op-forwarders" => self.no_op_forwarders,
            "name-suffix-duplication" => self.name_suffix_duplication,
            _ => 0,
        }
    }
}

struct Allowlist {
    files: BTreeMap<String, FileBaseline>,
}

impl Allowlist {
    fn for_file(&self, rel: &str) -> FileBaseline {
        self.files.get(rel).cloned().unwrap_or_default()
    }
}

fn load_allowlist(path: &Path) -> std::io::Result<Allowlist> {
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

// ---------------------------------------------------------------------
// Reporting.

#[derive(Default)]
struct Report {
    passed: bool,
    failures: Vec<String>,
    totals: BTreeMap<&'static str, u32>,
}

impl Report {
    fn merge(&mut self, rel: &str, counts: &Counts, baseline: &FileBaseline) {
        if self.failures.is_empty() {
            self.passed = true;
        }
        for (key, value) in counts.iter() {
            *self.totals.entry(key).or_insert(0) += value;
            let allowed = baseline.allowed(key);
            if value > allowed {
                self.passed = false;
                self.failures.push(format!("  FAIL {rel}: {key} {value} > baseline {allowed}"));
            }
        }
    }

    fn print(&self) {
        for line in &self.failures {
            println!("{line}");
        }
        println!();
        println!("standards-check totals:");
        for (key, value) in &self.totals {
            println!("  {key}: {value}");
        }
        if self.passed {
            println!("\nstandards-check: PASS");
        } else {
            println!(
                "\nstandards-check: FAIL — reduce the offending counts or, if a hit is justified, raise the per-file baseline in {ALLOWLIST} in the same PR."
            );
        }
    }
}
