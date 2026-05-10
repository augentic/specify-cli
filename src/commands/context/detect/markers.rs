//! Marker-file parsers used by the per-language detection passes.
//!
//! Lightweight grammars for the formats the [`super::runtimes`] detector
//! reaches for: TOML (Cargo, rust-toolchain, pyproject, ruff, clippy),
//! JSON-with-comments (`deno.json` / `deno.jsonc`), Makefile target
//! lines, and `go.mod` version directives. Lifted out of the detector so
//! per-language policy stays focused on what marker presence means.

use std::path::Path;

#[derive(Debug, Clone, Default)]
pub(super) struct TomlMarker {
    values: Vec<TomlValue>,
}

impl TomlMarker {
    pub(super) fn parse(contents: &str) -> Result<Self, String> {
        let mut marker = Self::default();
        let mut section: Vec<String> = Vec::new();
        let mut multiline_depth = 0_usize;

        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            let line = strip_toml_comment(raw_line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            if multiline_depth > 0 {
                multiline_depth = update_delimiter_depth(multiline_depth, &line)
                    .map_err(|err| format!("line {line_number}: {err}"))?;
                continue;
            }
            if line.starts_with('[') {
                section = parse_toml_section(&line)
                    .map_err(|err| format!("line {line_number}: {err}"))?;
                continue;
            }
            let (key, value) =
                parse_toml_assignment(&line).map_err(|err| format!("line {line_number}: {err}"))?;
            multiline_depth =
                delimiter_depth(value).map_err(|err| format!("line {line_number}: {err}"))?;
            marker.values.push(TomlValue {
                section: section.clone(),
                key: key.to_string(),
                value: parse_toml_scalar(value).unwrap_or_default(),
            });
        }

        if multiline_depth > 0 {
            return Err("unterminated array or inline table".to_string());
        }
        Ok(marker)
    }

    pub(super) fn value<const N: usize>(&self, section: [&str; N], key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|value| {
                value.key == key
                    && value.section.len() == section.len()
                    && value.section.iter().zip(section).all(|(left, right)| left.as_str() == right)
            })
            .map(|value| value.value.as_str())
    }
}

#[derive(Debug, Clone)]
struct TomlValue {
    section: Vec<String>,
    key: String,
    value: String,
}

fn parse_toml_section(line: &str) -> Result<Vec<String>, String> {
    let Some(inner) = line.strip_prefix('[').and_then(|value| value.strip_suffix(']')) else {
        return Err("malformed section header".to_string());
    };
    let inner = inner.strip_prefix('[').and_then(|value| value.strip_suffix(']')).unwrap_or(inner);
    if inner.trim().is_empty() {
        return Err("empty section header".to_string());
    }
    Ok(inner.split('.').map(|part| part.trim().trim_matches('"').to_string()).collect())
}

fn parse_toml_assignment(line: &str) -> Result<(&str, &str), String> {
    let Some((key, value)) = line.split_once('=') else {
        return Err("expected key-value assignment".to_string());
    };
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() {
        return Err("empty key".to_string());
    }
    if value.is_empty() {
        return Err("empty value".to_string());
    }
    Ok((key, value))
}

fn parse_toml_scalar(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(stripped) = value.strip_prefix('"').and_then(|value| value.strip_suffix('"')) {
        return Some(stripped.to_string());
    }
    if let Some(stripped) = value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')) {
        return Some(stripped.to_string());
    }
    if value.starts_with('[') || value.starts_with('{') {
        return None;
    }
    Some(value.to_string())
}

fn strip_toml_comment(line: &str) -> String {
    strip_comment(line, '#')
}

pub(super) fn strip_json_comments(contents: &str) -> String {
    contents.lines().map(|line| strip_comment(line, '/')).collect::<Vec<_>>().join("\n")
}

fn strip_comment(line: &str, comment: char) -> String {
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = line.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if !in_string
            && ch == comment
            && (comment != '/' || chars.peek().is_some_and(|(_next_index, next)| *next == '/'))
        {
            return line[..index].to_string();
        }
    }
    line.to_string()
}

fn delimiter_depth(value: &str) -> Result<usize, String> {
    update_delimiter_depth(0, value)
}

fn update_delimiter_depth(mut depth: usize, value: &str) -> Result<usize, String> {
    let mut in_string = false;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '[' | '{' => depth = depth.saturating_add(1),
            ']' | '}' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "unexpected closing delimiter".to_string())?;
            }
            _ => {}
        }
    }
    if in_string {
        return Err("unterminated string".to_string());
    }
    Ok(depth)
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct MakeTargets {
    pub(super) has_test: bool,
    pub(super) has_checks: bool,
}

pub(super) fn parse_make_targets(contents: &str) -> MakeTargets {
    let mut targets = MakeTargets::default();
    for raw_line in contents.lines() {
        let line = raw_line.trim_end();
        if line.starts_with('\t') || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((target_names, _recipe)) = line.split_once(':') else {
            continue;
        };
        if target_names.trim_start().starts_with('.') {
            continue;
        }
        for target in target_names.split_whitespace() {
            match target {
                "test" => targets.has_test = true,
                "checks" => targets.has_checks = true,
                _ => {}
            }
        }
    }
    targets
}

pub(super) fn parse_go_version(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("go ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

pub(super) fn relative_marker_path(project_dir: &Path, path: &Path) -> String {
    path.strip_prefix(project_dir)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makefile_targets_are_detected_shallowly() {
        let targets = parse_make_targets(
            ".PHONY: test checks\nnot-test:\n\ntest:\n\tcargo test\nchecks: lint\n",
        );

        assert!(targets.has_test);
        assert!(targets.has_checks);
    }
}
