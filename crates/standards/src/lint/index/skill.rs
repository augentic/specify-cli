//! `plugins/**/SKILL.md` extractor per the standards-layer contract
//! §"Module additions".
//!
//! Emits one [`Skill`] fact per file whose project-relative path
//! matches `plugins/<plugin>/skills/<skill>/SKILL.md`. The plugin
//! slug is the directory immediately under `plugins/`; non-matching
//! paths are skipped silently. The frontmatter is parsed by the
//! shared [`super::frontmatter`] extractor; this extractor only adds
//! the structural facts (name, plugin slug, body line count, and the
//! back-reference into the frontmatter table). Files missing a `name:`
//! frontmatter field collapse to a per-file skip.

use serde_json::Value;

use super::files::DiscoveredFile;
use super::frontmatter;
use crate::lint::Skill;

/// Extract a [`Skill`] fact from a discovered file.
///
/// Returns `None` when the file does not live under
/// `plugins/<plugin>/skills/.../SKILL.md`, when the frontmatter is
/// missing or unparseable, or when the frontmatter lacks a non-empty
/// `name:` field.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Option<Skill> {
    let (plugin, _skill_slug) = parse_skill_path(&file.relative)?;
    let frontmatter = frontmatter::extract(file)?;
    let name = frontmatter.fields.get("name").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(Skill {
        name: name.to_owned(),
        path: file.relative.clone(),
        plugin: plugin.to_owned(),
        frontmatter_ref: format!("{}#frontmatter", file.relative),
        body_line_count: Some(body_line_count(file)),
    })
}

/// Split `plugins/<plugin>/skills/<skill>/SKILL.md` into the
/// `(plugin, skill)` slug pair. Returns `None` for any other shape.
fn parse_skill_path(relative: &str) -> Option<(&str, &str)> {
    let rest = relative.strip_prefix("plugins/")?;
    let (plugin, rest) = rest.split_once('/')?;
    if plugin.is_empty() {
        return None;
    }
    let rest = rest.strip_prefix("skills/")?;
    let (skill, tail) = rest.split_once('/')?;
    if skill.is_empty() || tail != "SKILL.md" {
        return None;
    }
    Some((plugin, skill))
}

/// Count non-frontmatter body lines. Strips a leading
/// `---\n…\n---\n` block before counting.
fn body_line_count(file: &DiscoveredFile) -> u32 {
    let text = file.text();
    let body = frontmatter::split(&text).map_or(text.as_str(), |(_, body)| body);
    u32::try_from(body.lines().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::FileKind;

    fn skill_file(relative: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some("markdown".into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    #[test]
    fn extracts_skill_from_well_formed_path() {
        let file = skill_file(
            "plugins/spec/skills/init/SKILL.md",
            "---\nname: specify-init\ndescription: Init skill.\n---\n# Body\n\nLine two.\n",
        );
        let skill = extract(&file).expect("skill extracted");
        assert_eq!(skill.name, "specify-init");
        assert_eq!(skill.plugin, "spec");
        assert_eq!(skill.path, "plugins/spec/skills/init/SKILL.md");
        assert_eq!(skill.frontmatter_ref, "plugins/spec/skills/init/SKILL.md#frontmatter");
        assert!(skill.body_line_count.unwrap() >= 1);
    }

    #[test]
    fn rejects_non_skill_paths() {
        let file = skill_file("plugins/spec/SKILL.md", "---\nname: nope\n---\n");
        assert!(extract(&file).is_none());
    }

    #[test]
    fn rejects_skill_without_name() {
        let file = skill_file(
            "plugins/spec/skills/init/SKILL.md",
            "---\ndescription: missing name\n---\n",
        );
        assert!(extract(&file).is_none());
    }
}
