//! Built-in framework detectors for the source survey scanner.
//!
//! Shared scanning and import-resolution utilities live here;
//! framework-specific detection logic lives in child modules.

mod bullmq;
mod express;
mod nestjs;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

pub use bullmq::BullMqDetector;
pub use express::ExpressDetector;
pub use nestjs::NestJsDetector;
use regex::Regex;

use super::detector::DetectorError;

// ── Slug & ID helpers ───────────────────────────────────────────────

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// On collision within a single detector, append `-2`, `-3`, …
fn dedup_id(base_id: &str, seen: &mut HashMap<String, usize>) -> String {
    let count = seen.entry(base_id.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 { base_id.to_string() } else { format!("{base_id}-{count}") }
}

// ── package.json helpers ────────────────────────────────────────────

fn read_package_json(source_root: &Path) -> Result<Option<serde_json::Value>, DetectorError> {
    let path = source_root.join("package.json");
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map(Some).map_err(|e| DetectorError::Malformed {
            reason: format!("package.json: {e}"),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DetectorError::Io {
            reason: format!("reading package.json: {e}"),
        }),
    }
}

fn has_dependency(pkg: &serde_json::Value, name: &str) -> bool {
    ["dependencies", "devDependencies"].iter().any(|key| {
        pkg.get(key).and_then(serde_json::Value::as_object).is_some_and(|d| d.contains_key(name))
    })
}

fn has_dependency_prefix(pkg: &serde_json::Value, prefix: &str) -> bool {
    ["dependencies", "devDependencies"].iter().any(|key| {
        pkg.get(key)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|d| d.keys().any(|k| k.starts_with(prefix)))
    })
}

// ── File-system scanning ────────────────────────────────────────────

fn walk_source_files(source_root: &Path) -> Result<Vec<PathBuf>, DetectorError> {
    let mut files = Vec::new();
    collect_ts_js(source_root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_ts_js(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), DetectorError> {
    let entries = fs::read_dir(dir).map_err(|e| DetectorError::Io {
        reason: format!("reading {}: {e}", dir.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| DetectorError::Io {
            reason: format!("entry in {}: {e}", dir.display()),
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_ts_js(&path, out)?;
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("ts" | "js")) {
            out.push(path);
        }
    }
    Ok(())
}

// ── Import resolution ───────────────────────────────────────────────

/// BFS walk from `start_file`, collecting all transitively reachable
/// local source files. Returns sorted relative paths from `source_root`.
fn resolve_touches(source_root: &Path, start_file: &Path) -> Vec<String> {
    let root = source_root.canonicalize().unwrap_or_else(|_| source_root.to_path_buf());
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    if let Ok(c) = start_file.canonicalize() {
        queue.push_back(c);
    }

    while let Some(file) = queue.pop_front() {
        if !visited.insert(file.clone()) {
            continue;
        }
        // v1 import resolution is best-effort; unresolvable imports are skipped
        if let Ok(content) = fs::read_to_string(&file) {
            let parent = file.parent().unwrap_or(&root);
            for resolved in local_imports(&content, parent, &root) {
                if !visited.contains(&resolved) {
                    queue.push_back(resolved);
                }
            }
        }
    }

    let mut result: Vec<String> = visited
        .into_iter()
        .filter_map(|p| p.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
        .collect();
    result.sort();
    result
}

fn local_imports(content: &str, file_dir: &Path, source_root: &Path) -> Vec<PathBuf> {
    let re = Regex::new(r#"(?:from\s+|require\s*\(\s*)['"](\.[^'"]+)['"]"#).expect("constant");
    re.captures_iter(content)
        .filter_map(|cap| {
            let spec = cap.get(1)?.as_str();
            resolve_module(file_dir, spec, source_root)
        })
        .collect()
}

fn resolve_module(dir: &Path, specifier: &str, source_root: &Path) -> Option<PathBuf> {
    let base = dir.join(specifier);
    let root = source_root.canonicalize().ok()?;
    for suffix in ["", ".ts", ".js", "/index.ts", "/index.js"] {
        let mut attempt = base.as_os_str().to_os_string();
        attempt.push(suffix);
        let p = PathBuf::from(attempt);
        if p.is_file() {
            let c = p.canonicalize().ok()?;
            if c.starts_with(&root) {
                return Some(c);
            }
        }
    }
    None
}

// ── Named-import binding resolution ─────────────────────────────────

fn import_bindings(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let named = Regex::new(r#"import\s*\{([^}]+)\}\s*from\s*['"]([^'"]+)['"]"#).expect("constant");
    for cap in named.captures_iter(content) {
        let module = cap.get(2).unwrap().as_str().to_string();
        for name in cap.get(1).unwrap().as_str().split(',') {
            let name = name.split(" as ").next().unwrap_or("").trim();
            if !name.is_empty() {
                out.push((name.to_string(), module.clone()));
            }
        }
    }
    let default = Regex::new(r#"import\s+(\w+)\s+from\s*['"]([^'"]+)['"]"#).expect("constant");
    for cap in default.captures_iter(content) {
        out.push((
            cap.get(1).unwrap().as_str().to_string(),
            cap.get(2).unwrap().as_str().to_string(),
        ));
    }
    out
}

/// Resolve a named handler symbol to `(handler_string, touches)`.
///
/// When the symbol is imported from a relative module, the handler string
/// is `<resolved_file>:<symbol>` and touches walk from the resolved file.
/// Otherwise it falls back to the containing file.
fn resolve_named_handler(
    name: &str, file_content: &str, file: &Path, source_root: &Path,
) -> (String, Vec<String>) {
    let rel = rel_path(source_root, file);
    for (binding, module) in import_bindings(file_content) {
        if binding == name && module.starts_with('.') {
            let dir = file.parent().unwrap_or(source_root);
            let root_c = source_root.canonicalize().unwrap_or_else(|_| source_root.to_path_buf());
            if let Some(resolved) = resolve_module(dir, &module, &root_c) {
                let resolved_rel = rel_path(source_root, &resolved);
                return (format!("{resolved_rel}:{name}"), resolve_touches(source_root, &resolved));
            }
        }
    }
    (format!("{rel}:{name}"), resolve_touches(source_root, file))
}

fn rel_path(source_root: &Path, file: &Path) -> String {
    let root = source_root.canonicalize().unwrap_or_else(|_| source_root.to_path_buf());
    file.canonicalize()
        .ok()
        .and_then(|c| c.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
        .unwrap_or_default()
}

/// Extract a named handler identifier from the arguments after a route
/// string literal. Returns `None` for inline/arrow functions.
fn extract_handler_name(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty()
        || trimmed.contains("=>")
        || trimmed.starts_with("function")
        || trimmed.starts_with("async")
        || trimmed.starts_with('(')
    {
        return None;
    }
    let re = Regex::new(r"\b([a-zA-Z_$]\w*)\b").expect("constant");
    let mut last = None;
    for cap in re.captures_iter(trimmed) {
        let w = cap.get(1).unwrap().as_str();
        if !matches!(
            w,
            "function" | "async" | "await" | "new" | "true" | "false" | "null" | "undefined"
        ) {
            last = Some(w.to_string());
        }
    }
    last
}
