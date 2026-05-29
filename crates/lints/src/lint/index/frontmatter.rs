//! Markdown frontmatter extractor per the `WorkspaceModel` entity families.
//!
//! Splits a markdown file at its leading `---\n…\n---\n` block and
//! parses the YAML body via `serde_saphyr`. Non-markdown files and
//! markdown files without a frontmatter block return `None`; YAML
//! parse failures also collapse to `None` — reserved-hint diagnostics reserves the
//! `index.warning` finding for S7's hint runner.
//!
//! `schema_id` is left unset in v1: the `WorkspaceModel` entity families types it as `Option<String>` but v1 has no shape-inference
//! pass to populate it. Consumers that need to consult the declared
//! `schema_id` should read the fields map.

use serde_json::{Map, Value};

use super::files::DiscoveredFile;
use crate::lint::Frontmatter;

/// Extract a [`Frontmatter`] fact from a discovered file.
///
/// Returns `None` for non-markdown files, files without a leading
/// `---\n` block, files whose closing delimiter is missing, and
/// files whose YAML body fails to parse as a JSON object.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Option<Frontmatter> {
    if file.language.as_deref() != Some("markdown") {
        return None;
    }
    // Rule frontmatter is surfaced via the `rule_index` family
    // per the doc-comment on `super::discover`; the two surfaces would
    // double-count if the generic extractor also fired here.
    if file.relative.starts_with(".specify/cache/rules/") {
        return None;
    }
    let text = file.text();
    let frontmatter_body = split(&text)?;
    let value: Value = serde_saphyr::from_str(frontmatter_body).ok()?;
    let fields = match value {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => return None,
    };
    Some(Frontmatter {
        path: file.relative.clone(),
        schema_id: None,
        fields,
    })
}

/// Slice `content` to the bytes between an opening `---\n` (or
/// `---\r\n`) line and the matching closing `---` line. Mirrors the
/// codex parser's split rules so authoring conventions agree across
/// surfaces.
fn split(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let mut search_from = 0;
    while let Some(rel) = rest[search_from..].find("\n---") {
        let pos = search_from + rel;
        let after = pos + "\n---".len();
        let tail = &rest[after..];
        if tail.is_empty() || tail.starts_with('\n') || tail.starts_with("\r\n") {
            return Some(&rest[..pos]);
        }
        search_from = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn markdown(text: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: "doc.md".into(),
            kind: FileKind::Text,
            language: Some("markdown".into()),
            bytes: Some(text.as_bytes().to_vec()),
        }
    }

    #[test]
    fn parses_simple_frontmatter() {
        let f = markdown("---\ntitle: Demo\nschema_id: rule\n---\n# Body\n");
        let fm = extract(&f).expect("frontmatter present");
        assert_eq!(fm.path, "doc.md");
        assert_eq!(fm.schema_id, None);
        assert_eq!(fm.fields.get("title").and_then(Value::as_str), Some("Demo"));
        assert_eq!(fm.fields.get("schema_id").and_then(Value::as_str), Some("rule"));
    }

    #[test]
    fn non_markdown_returns_none() {
        let f = DiscoveredFile {
            relative: "src/lib.rs".into(),
            kind: FileKind::Text,
            language: Some("rust".into()),
            bytes: Some(b"// some code".to_vec()),
        };
        assert!(extract(&f).is_none());
    }

    #[test]
    fn missing_frontmatter_returns_none() {
        let f = markdown("# heading only\n");
        assert!(extract(&f).is_none());
    }

    #[test]
    fn invalid_yaml_returns_none() {
        let f = markdown("---\n  : : :\n---\nbody\n");
        assert!(extract(&f).is_none());
    }
}
