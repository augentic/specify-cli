//! Symlink fact recorder per RFC-32 §"Core entity families (v1)" and
//! §D1.
//!
//! Called by [`super::files`] for each entry whose `file_type()` is a
//! symlink. The walker carries `follow_links(false)`, so the link is
//! visited but the underlying target is never traversed — only the
//! `(path, target, broken)` triple is recorded.

use std::path::{MAIN_SEPARATOR, Path};

use crate::lint::Symlink;

/// Build a [`Symlink`] fact from an on-disk symlink entry.
///
/// Returns `None` when:
/// - the entry sits outside `project_dir` (the strip-prefix fails),
/// - `read_link` fails (the link node disappeared between walk and
///   read), or
/// - either path cannot be rendered as UTF-8.
///
/// `broken` is computed via `Path::exists()`, which dereferences the
/// symlink — a missing or self-referencing target yields `true`.
#[must_use]
pub fn record(path: &Path, project_dir: &Path) -> Option<Symlink> {
    let relative = path.strip_prefix(project_dir).ok()?;
    let path_str = render(relative)?;
    let target = std::fs::read_link(path).ok()?;
    let target_str = render(&target)?;
    let broken = !path.exists();
    Some(Symlink {
        path: path_str,
        target: target_str,
        broken,
    })
}

fn render(p: &Path) -> Option<String> {
    let s = p.to_str()?;
    if MAIN_SEPARATOR == '/' { Some(s.to_owned()) } else { Some(s.replace(MAIN_SEPARATOR, "/")) }
}
