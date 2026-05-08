//! Shallow root-marker detection for generated context guidance.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use serde_json::Value;

const NOT_DETECTED: &str = "not detected";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Detection {
    pub(super) runtimes: Vec<RuntimeDetection>,
    pub(super) tests: Vec<CommandDetection>,
    pub(super) linting: Vec<LintDetection>,
    pub(super) warnings: Vec<DetectionWarning>,
    pub(super) input_paths: Vec<String>,
}

impl Detection {
    pub(super) fn runtime_bullets(&self) -> Vec<String> {
        if self.runtimes.is_empty() {
            return vec![NOT_DETECTED.to_string()];
        }
        self.runtimes.iter().map(RuntimeDetection::bullet).collect()
    }

    pub(super) fn test_bullets(&self) -> Vec<String> {
        if self.tests.is_empty() {
            return vec![NOT_DETECTED.to_string()];
        }
        self.tests.iter().map(CommandDetection::bullet).collect()
    }

    pub(super) fn lint_bullets(&self) -> Vec<String> {
        if self.linting.is_empty() {
            return vec![NOT_DETECTED.to_string()];
        }
        self.linting.iter().map(LintDetection::bullet).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetectionWarning {
    pub(super) path: String,
    pub(super) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDetection {
    id: &'static str,
    label: String,
}

impl RuntimeDetection {
    const fn new(id: &'static str, label: String) -> Self {
        Self { id, label }
    }

    fn bullet(&self) -> String {
        format!("detected: {}.", self.label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandDetection {
    id: &'static str,
    command: &'static str,
}

impl CommandDetection {
    const fn new(id: &'static str, command: &'static str) -> Self {
        Self { id, command }
    }

    fn bullet(&self) -> String {
        format!("detected: `{}`.", self.command)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LintDetection {
    Command(CommandDetection),
    Workflow(String),
}

impl LintDetection {
    const fn id(&self) -> &str {
        match self {
            Self::Command(command) => command.id,
            Self::Workflow(_name) => "github-actions",
        }
    }

    fn bullet(&self) -> String {
        match self {
            Self::Command(command) => command.bullet(),
            Self::Workflow(name) => format!("detected: GitHub Actions workflow `{name}`."),
        }
    }
}

pub(super) fn detect_root_markers(project_dir: &Path) -> Detection {
    let mut detector = Detector::new(project_dir);
    detector.detect_rust();
    detector.detect_node();
    detector.detect_python();
    detector.detect_go();
    detector.detect_deno();
    detector.detect_make();
    detector.detect_github_actions();
    detector.sort();
    detector.detection
}

struct Detector<'a> {
    project_dir: &'a Path,
    detection: Detection,
}

impl<'a> Detector<'a> {
    const fn new(project_dir: &'a Path) -> Self {
        Self {
            project_dir,
            detection: Detection {
                runtimes: Vec::new(),
                tests: Vec::new(),
                linting: Vec::new(),
                warnings: Vec::new(),
                input_paths: Vec::new(),
            },
        }
    }

    fn detect_rust(&mut self) {
        let cargo_path = self.project_dir.join("Cargo.toml");
        if !cargo_path.is_file() {
            return;
        }
        if self.parse_toml_marker(&cargo_path).is_none() {
            return;
        }

        let toolchain = self
            .parse_toml_marker(&self.project_dir.join("rust-toolchain.toml"))
            .and_then(|marker| marker.value(["toolchain"], "channel").map(ToOwned::to_owned));
        let label = match toolchain {
            Some(channel) if !channel.is_empty() => format!("Rust (toolchain `{channel}`)"),
            _ => "Rust".to_string(),
        };
        self.detection.runtimes.push(RuntimeDetection::new("rust", label));
        self.detection.tests.push(CommandDetection::new("rust", "cargo test"));

        let clippy_path = self.project_dir.join("clippy.toml");
        if clippy_path.is_file() && self.parse_toml_marker(&clippy_path).is_some() {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("rust-clippy", "cargo clippy")));
        }
    }

    fn detect_node(&mut self) {
        let package_path = self.project_dir.join("package.json");
        let Some(package) = self.parse_json_marker(&package_path) else {
            self.detect_eslint_without_npm_script();
            return;
        };

        let engine = package
            .get("engines")
            .and_then(|engines| engines.get("node"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let label = engine.map_or_else(
            || "Node.js".to_string(),
            |version| format!("Node.js (engines.node `{version}`)"),
        );
        self.detection.runtimes.push(RuntimeDetection::new("node", label));

        let scripts = package.get("scripts").and_then(Value::as_object);
        if scripts.and_then(|scripts| scripts.get("test")).and_then(Value::as_str).is_some() {
            self.detection.tests.push(CommandDetection::new("node", "npm test"));
        }
        if scripts.and_then(|scripts| scripts.get("lint")).and_then(Value::as_str).is_some() {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("node-lint", "npm run lint")));
        } else {
            self.detect_eslint_without_npm_script();
        }
    }

    fn detect_eslint_without_npm_script(&mut self) {
        if self.eslint_marker_detected() {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("eslint", "eslint")));
        }
    }

    fn detect_python(&mut self) {
        let pyproject_path = self.project_dir.join("pyproject.toml");
        let requirements_path = self.project_dir.join("requirements.txt");
        let pyproject_detected =
            pyproject_path.is_file() && self.parse_toml_marker(&pyproject_path).is_some();
        if pyproject_detected {
            self.detection
                .runtimes
                .push(RuntimeDetection::new("python", "Python (pyproject.toml)".to_string()));
        } else if requirements_path.is_file() {
            if self.read_marker(&requirements_path).is_none() {
                return;
            }
            self.detection
                .runtimes
                .push(RuntimeDetection::new("python", "Python (requirements.txt)".to_string()));
        }

        let ruff_path = self.project_dir.join("ruff.toml");
        if ruff_path.is_file() && self.parse_toml_marker(&ruff_path).is_some() {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("ruff", "ruff check")));
        }
    }

    fn detect_go(&mut self) {
        let go_mod_path = self.project_dir.join("go.mod");
        if !go_mod_path.is_file() {
            return;
        }
        let Some(contents) = self.read_marker(&go_mod_path) else {
            return;
        };
        let version = parse_go_version(&contents);
        let label = version.map_or_else(|| "Go".to_string(), |version| format!("Go {version}"));
        self.detection.runtimes.push(RuntimeDetection::new("go", label));
        self.detection.tests.push(CommandDetection::new("go", "go test ./..."));
    }

    fn detect_deno(&mut self) {
        let deno_path = ["deno.json", "deno.jsonc"]
            .iter()
            .map(|name| self.project_dir.join(name))
            .find(|path| path.is_file());
        let Some(deno_path) = deno_path else {
            return;
        };
        let Some(config) = self.parse_deno_marker(&deno_path) else {
            return;
        };

        self.detection.runtimes.push(RuntimeDetection::new("deno", "Deno".to_string()));
        let tasks = config.get("tasks").and_then(Value::as_object);
        if tasks.and_then(|tasks| tasks.get("test")).and_then(Value::as_str).is_some() {
            self.detection.tests.push(CommandDetection::new("deno", "deno task test"));
        }
        if tasks.and_then(|tasks| tasks.get("lint")).and_then(Value::as_str).is_some() {
            self.detection.linting.push(LintDetection::Command(CommandDetection::new(
                "deno-lint-task",
                "deno task lint",
            )));
        } else if config.get("lint").is_some() {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("deno-lint", "deno lint")));
        }
    }

    fn detect_make(&mut self) {
        let makefile_path = self.project_dir.join("Makefile");
        if !makefile_path.is_file() {
            return;
        }
        let Some(contents) = self.read_marker(&makefile_path) else {
            return;
        };
        let targets = parse_make_targets(&contents);
        if targets.has_test {
            self.detection.tests.push(CommandDetection::new("make-test", "make test"));
        }
        if targets.has_checks {
            self.detection
                .linting
                .push(LintDetection::Command(CommandDetection::new("make-checks", "make checks")));
        }
    }

    fn detect_github_actions(&mut self) {
        let workflows_dir = self.project_dir.join(".github").join("workflows");
        if !workflows_dir.is_dir() {
            return;
        }
        let mut workflows = Vec::new();
        let Ok(entries) = fs::read_dir(workflows_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("yaml")) {
                workflows.push(path);
            }
        }
        workflows.sort();
        let Some(first_workflow) = workflows.first() else {
            return;
        };
        let Some(value) = self.parse_yaml_marker(first_workflow) else {
            return;
        };
        let Some(name) =
            value.get("name").and_then(Value::as_str).filter(|value| !value.is_empty())
        else {
            return;
        };
        self.detection.linting.push(LintDetection::Workflow(name.to_string()));
    }

    fn eslint_marker_detected(&mut self) -> bool {
        let Ok(entries) = fs::read_dir(self.project_dir) else {
            return false;
        };
        let mut candidates = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if file_name.starts_with(".eslintrc") {
                candidates.push(path);
            }
        }
        candidates.sort();
        for candidate in candidates {
            if self.eslint_marker_file_detected(&candidate) {
                return true;
            }
        }
        false
    }

    fn eslint_marker_file_detected(&mut self, path: &Path) -> bool {
        let Some(contents) = self.read_marker(path) else {
            return false;
        };
        let trimmed = contents.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            match serde_json::from_str::<Value>(&contents) {
                Ok(_value) => true,
                Err(err) => {
                    self.warn(path, format!("failed to parse JSON marker: {err}"));
                    false
                }
            }
        } else if path.extension() == Some(OsStr::new("yaml"))
            || path.extension() == Some(OsStr::new("yml"))
        {
            self.parse_yaml_marker(path).is_some()
        } else {
            true
        }
    }

    fn parse_json_marker(&mut self, path: &Path) -> Option<Value> {
        if !path.is_file() {
            return None;
        }
        let contents = self.read_marker(path)?;
        match serde_json::from_str(&contents) {
            Ok(value) => Some(value),
            Err(err) => {
                self.warn(path, format!("failed to parse JSON marker: {err}"));
                None
            }
        }
    }

    fn parse_deno_marker(&mut self, path: &Path) -> Option<Value> {
        let contents = self.read_marker(path)?;
        let json = if path.file_name() == Some(OsStr::new("deno.jsonc")) {
            strip_json_comments(&contents)
        } else {
            contents
        };
        match serde_json::from_str(&json) {
            Ok(value) => Some(value),
            Err(err) => {
                self.warn(path, format!("failed to parse JSON marker: {err}"));
                None
            }
        }
    }

    fn parse_yaml_marker(&mut self, path: &Path) -> Option<Value> {
        let contents = self.read_marker(path)?;
        match serde_saphyr::from_str(&contents) {
            Ok(value) => Some(value),
            Err(err) => {
                self.warn(path, format!("failed to parse YAML marker: {err}"));
                None
            }
        }
    }

    fn parse_toml_marker(&mut self, path: &Path) -> Option<TomlMarker> {
        if !path.is_file() {
            return None;
        }
        let contents = self.read_marker(path)?;
        match TomlMarker::parse(&contents) {
            Ok(marker) => Some(marker),
            Err(err) => {
                self.warn(path, format!("failed to parse TOML marker: {err}"));
                None
            }
        }
    }

    fn read_marker(&mut self, path: &Path) -> Option<String> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                self.detection.input_paths.push(relative_marker_path(self.project_dir, path));
                Some(contents)
            }
            Err(err) => {
                self.warn(path, format!("failed to read marker: {err}"));
                None
            }
        }
    }

    fn warn(&mut self, path: &Path, message: String) {
        self.detection.warnings.push(DetectionWarning {
            path: relative_marker_path(self.project_dir, path),
            message,
        });
    }

    fn sort(&mut self) {
        self.detection.runtimes.sort_by(|left, right| left.id.cmp(right.id));
        self.detection.runtimes.dedup_by(|left, right| left.id == right.id);
        self.detection.tests.sort_by(|left, right| left.id.cmp(right.id));
        self.detection.tests.dedup_by(|left, right| left.id == right.id);
        self.detection.linting.sort_by(|left, right| left.id().cmp(right.id()));
        self.detection.linting.dedup_by(|left, right| left.id() == right.id());
        self.detection.warnings.sort_by(|left, right| left.path.cmp(&right.path));
        self.detection.input_paths.sort();
        self.detection.input_paths.dedup();
    }
}

#[derive(Debug, Clone, Default)]
struct TomlMarker {
    values: Vec<TomlValue>,
}

impl TomlMarker {
    fn parse(contents: &str) -> Result<Self, String> {
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

    fn value<const N: usize>(&self, section: [&str; N], key: &str) -> Option<&str> {
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

fn strip_json_comments(contents: &str) -> String {
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
struct MakeTargets {
    has_test: bool,
    has_checks: bool,
}

fn parse_make_targets(contents: &str) -> MakeTargets {
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

fn parse_go_version(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("go ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn relative_marker_path(project_dir: &Path, path: &Path) -> String {
    path.strip_prefix(project_dir)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn mixed_runtime_order_is_deterministic() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("package.json"), r#"{"engines":{"node":">=20"}}"#)
            .expect("package");
        fs::write(tmp.path().join("go.mod"), "module demo\n\ngo 1.22\n").expect("go");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("cargo");

        let detection = detect_root_markers(tmp.path());

        let labels: Vec<String> =
            detection.runtimes.iter().map(|runtime| runtime.label.clone()).collect();
        assert_eq!(
            labels,
            vec![
                "Go 1.22".to_string(),
                "Node.js (engines.node `>=20`)".to_string(),
                "Rust".to_string(),
            ]
        );
    }

    #[test]
    fn corrupt_markers_warn_and_do_not_detect_that_marker() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "package = [").expect("cargo");
        fs::write(tmp.path().join("package.json"), "{").expect("package");
        fs::create_dir_all(tmp.path().join(".github/workflows")).expect("workflows dir");
        fs::write(tmp.path().join(".github/workflows/ci.yaml"), "name: [").expect("workflow");

        let detection = detect_root_markers(tmp.path());

        assert!(detection.runtimes.is_empty());
        assert_eq!(
            detection.warnings.iter().map(|warning| warning.path.as_str()).collect::<Vec<_>>(),
            vec![".github/workflows/ci.yaml", "Cargo.toml", "package.json"]
        );
    }

    #[test]
    fn makefile_targets_are_detected_shallowly() {
        let targets = parse_make_targets(
            ".PHONY: test checks\nnot-test:\n\ntest:\n\tcargo test\nchecks: lint\n",
        );

        assert!(targets.has_test);
        assert!(targets.has_checks);
    }
}
