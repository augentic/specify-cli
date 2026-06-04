//! The shared SKILL.md walk, memoised once per [`Context`].
//!
//! The five frontmatter predicates each iterate the same set of skill
//! files. [`load_skill_entries`] performs the walk once and caches the
//! result on the [`Context`] so repeated calls (within one check and
//! across the five checks sharing a context) reuse a single walk.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value as JsonValue;

use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, skill_frontmatter, walk_skill_files};

/// One discovered SKILL.md: its path, framework-relative display path,
/// owning plugin directory, and parsed frontmatter (when present).
pub(super) struct SkillEntry {
    pub(super) path: PathBuf,
    pub(super) rel: String,
    pub(super) plugin_dir: String,
    pub(super) frontmatter: Option<BTreeMap<String, JsonValue>>,
}

/// The memoised walk result. A newtype (rather than a bare
/// `Vec<SkillEntry>`) so the shared `Arc` handle stays a struct rather
/// than `Arc<Vec<…>>`; it derefs to `[SkillEntry]` so callers iterate it
/// like a slice.
pub(super) struct SkillEntries(Vec<SkillEntry>);

impl std::ops::Deref for SkillEntries {
    type Target = [SkillEntry];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Memo key for the skill-file walk on [`Context`].
const SKILL_ENTRIES_KEY: &str = "skill-frontmatter::entries";

/// Walk every SKILL.md under the framework root once, returning a shared
/// handle. Memoised on `ctx` so the five frontmatter predicates share a
/// single walk per [`Context`].
pub(super) fn load_skill_entries(ctx: &Context) -> Result<Arc<SkillEntries>, ToolingError> {
    ctx.memoize(SKILL_ENTRIES_KEY, || walk_entries(ctx).map(SkillEntries))
}

fn walk_entries(ctx: &Context) -> Result<Vec<SkillEntry>, ToolingError> {
    let framework_root = ctx.framework_root();
    let plugins_dir = ctx.plugins_dir();

    walk_skill_files(framework_root)?
        .into_iter()
        .map(|path| {
            let rel = relative_display(framework_root, &path);
            let plugin_dir = path
                .strip_prefix(&plugins_dir)
                .ok()
                .and_then(|rel| rel.components().next())
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            let content = fs::read_to_string(&path)?;
            let frontmatter = skill_frontmatter(&content);
            Ok(SkillEntry {
                path,
                rel,
                plugin_dir,
                frontmatter,
            })
        })
        .collect()
}
