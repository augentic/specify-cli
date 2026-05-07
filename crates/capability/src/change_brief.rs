//! Change brief parser — operator-authored brief
//! (RFC-3a §*The Initiative Brief*; renamed to *change brief* by
//! RFC-13 chunk 3.4 and migrated on-disk from `initiative.md` to
//! `change.md` by RFC-13 chunk 3.7).
//!
//! `change.md` (at the repo root) is a markdown document with a
//! `---`-delimited YAML frontmatter block at the top. The frontmatter
//! shape is enforced here (`#[serde(deny_unknown_fields)]` +
//! [`ChangeBrief::parse_str`] invariants); the body is captured
//! verbatim and **not** parsed in v1. A future RFC may land structured
//! body parsing, but today's consumers treat the body as prose.
//!
//! Pre-Phase-3.7 projects still carry the brief as `initiative.md`.
//! `specify migrate change-noun` is the operator path that renames it
//! into the post-RFC `change.md` location at the repo root.
//! [`ChangeBrief::path`] returns the post-rename filename;
//! [`ChangeBrief::legacy_path`] returns the pre-rename filename so
//! migrators and the "found legacy file" diagnostic
//! ([`Error::ChangeBriefBecameChangeMd`]) have one place to ask for
//! either name.
//!
//! No JSON schema file ships for v1 per the RFC — the shape is enforced
//! directly in code.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::brief::split_on_closing_delimiter;

/// Filename of the post-RFC-13 operator brief at the repo root.
///
/// Written by `specify change create <name>` and by
/// `specify migrate change-noun` (the chunk 3.7 migrator that renames
/// pre-Phase-3.7 [`LEGACY_CHANGE_BRIEF_FILENAME`] in place).
pub const CHANGE_BRIEF_FILENAME: &str = "change.md";

/// Pre-Phase-3.7 filename of the operator brief at the repo root.
///
/// Loaded only by the `specify migrate change-noun` migrator when
/// renaming the file in place; the post-RFC CLI surface
/// (`specify change {create, show, finalize}` and `specify change
/// plan archive`) refuses to read this filename and emits
/// [`Error::ChangeBriefBecameChangeMd`] pointing the operator at the
/// migration verb.
pub const LEGACY_CHANGE_BRIEF_FILENAME: &str = "initiative.md";

/// Kebab-case predicate. Duplicates the helper that lives in
/// `specify_registry::validate::is_kebab_case` because RFC-13 chunk 2.1
/// extracted registry parsing into its own crate; depending on
/// `specify-registry` from here would re-introduce the
/// "platform components are not capabilities" cycle (RFC-13 §Migration
/// invariant 4). Kept private and inlined.
fn is_kebab_case(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    if s.contains("--") {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// In-memory representation of `change.md` (at the repo root).
///
/// Structured frontmatter (YAML) + free-form body (markdown). The
/// body is preserved byte-for-byte so round-tripping is faithful;
/// structured body interpretation is explicitly deferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeBrief {
    /// Parsed YAML frontmatter.
    pub frontmatter: ChangeFrontmatter,
    /// Markdown body captured verbatim. Not parsed further in v1.
    pub body: String,
}

/// Parsed frontmatter of `change.md`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeFrontmatter {
    /// Kebab-case change name.
    pub name: String,
    /// Seed inputs. Optional — may be absent or empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<ChangeInput>,
}

/// One entry in [`ChangeFrontmatter::inputs`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeInput {
    /// Relative or absolute path. Stored verbatim; resolution happens
    /// downstream in `/spec:analyze` (not in this crate).
    pub path: String,
    /// Closed enum — see [`InputKind`].
    pub kind: InputKind,
}

/// Closed enum over the kinds of seed input a brief can declare.
///
/// Unknown values are a hard parse error (serde-driven) so typos
/// like `kind: documenttation` fail fast.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputKind {
    /// Existing source tree that will be migrated/extracted.
    LegacyCode,
    /// Human-authored references (runbooks, PDFs, design docs).
    Documentation,
}

impl ChangeBrief {
    /// Absolute path to `<project_dir>/change.md`. The post-RFC-13
    /// operator brief lives at the repo root.
    #[must_use]
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(CHANGE_BRIEF_FILENAME)
    }

    /// Absolute path to `<project_dir>/initiative.md` — the
    /// pre-Phase-3.7 filename. Used by `specify migrate change-noun`
    /// to detect the legacy file before renaming, and by every
    /// `specify change *` verb that wants to surface the
    /// [`Error::ChangeBriefBecameChangeMd`] diagnostic when a project
    /// has not run the migration yet.
    #[must_use]
    pub fn legacy_path(project_dir: &Path) -> PathBuf {
        project_dir.join(LEGACY_CHANGE_BRIEF_FILENAME)
    }

    /// Refuse to load when only the pre-Phase-3.7 filename is on disk.
    ///
    /// Returns `Err(Error::ChangeBriefBecameChangeMd { path })` when
    /// `<project_dir>/initiative.md` exists and `<project_dir>/change.md`
    /// does not — the caller (`specify change show` and
    /// `specify change finalize`) surfaces the diagnostic and points the
    /// operator at `specify migrate change-noun`. Returns `Ok(())` when
    /// the project is on the post-Phase-3.7 layout, when the brief is
    /// absent altogether, or when both filenames are present (the
    /// migration verb resolves that case via
    /// [`Error::ChangeNounMigrationTargetExists`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ChangeBriefBecameChangeMd`] when only the
    /// legacy file is present.
    pub fn refuse_legacy(project_dir: &Path) -> Result<(), Error> {
        let modern = Self::path(project_dir);
        let legacy = Self::legacy_path(project_dir);
        if !modern.exists() && legacy.is_file() {
            return Err(Error::ChangeBriefBecameChangeMd { path: legacy });
        }
        Ok(())
    }

    /// Load + shape-validate the change brief.
    ///
    /// - `Ok(None)` — the file is absent. The brief is optional and a
    ///   missing file is *not* an error.
    /// - `Ok(Some(_))` — parsed and shape-validated.
    /// - `Err(_)` — malformed YAML, unknown keys, kebab-case / required-
    ///   field / empty-path violations.
    ///
    /// Reads `change.md` only — the post-Phase-3.7 filename. Callers
    /// that want the loud-diagnostic fall-back for projects still on
    /// `initiative.md` must run [`ChangeBrief::refuse_legacy`] first.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(project_dir: &Path) -> Result<Option<Self>, Error> {
        let path = Self::path(project_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|err| Error::Config(format!("failed to read {}: {err}", path.display())))?;
        Self::parse_str(&content).map(Some)
    }

    /// Parse an in-memory brief: YAML frontmatter between `---`
    /// delimiters followed by a verbatim markdown body.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn parse_str(content: &str) -> Result<Self, Error> {
        let after_open =
            content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")).ok_or_else(
                || Error::Config(format!("{CHANGE_BRIEF_FILENAME}: missing YAML frontmatter")),
            )?;
        let (frontmatter_text, body) = split_on_closing_delimiter(after_open).ok_or_else(|| {
            Error::Config(format!(
                "{CHANGE_BRIEF_FILENAME}: opening `---` has no closing `---` delimiter"
            ))
        })?;

        let frontmatter: ChangeFrontmatter =
            serde_saphyr::from_str(frontmatter_text).map_err(|err| {
                Error::Config(format!("{CHANGE_BRIEF_FILENAME}: invalid frontmatter: {err}"))
            })?;

        let brief = Self {
            frontmatter,
            body: body.to_string(),
        };
        brief.validate_shape()?;
        Ok(brief)
    }

    /// Invariants serde cannot express: kebab-case name, non-empty
    /// input paths.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn validate_shape(&self) -> Result<(), Error> {
        if self.frontmatter.name.is_empty() {
            return Err(Error::Config(format!("{CHANGE_BRIEF_FILENAME}: name is empty")));
        }
        if !is_kebab_case(&self.frontmatter.name) {
            return Err(Error::Config(format!(
                "{CHANGE_BRIEF_FILENAME}: name `{}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)",
                self.frontmatter.name
            )));
        }
        for (idx, input) in self.frontmatter.inputs.iter().enumerate() {
            if input.path.is_empty() {
                return Err(Error::Config(format!(
                    "{CHANGE_BRIEF_FILENAME}: inputs[{idx}].path is empty"
                )));
            }
        }
        Ok(())
    }

    /// Render the canonical `change.md` template for the given
    /// kebab-case change name. Byte-stable — the `change create` CLI
    /// verb compares against a golden fixture.
    #[must_use]
    #[allow(clippy::literal_string_with_formatting_args)]
    pub fn template(name: &str) -> String {
        CHANGE_TEMPLATE.replace("{name}", name).replace("{title}", &title_case(name))
    }
}

/// Canonical template shipped by `specify change create`. The
/// golden-fixture test pins this byte-for-byte; any edit here must be
/// mirrored in the test constant.
///
/// RFC-13 chunk 3.7 refreshed the prose to name the artefact a
/// "change" (matching the new filename and the surface verbs); the
/// frontmatter shape is unchanged.
const CHANGE_TEMPLATE: &str = "\
---
name: {name}
inputs: []
---

# {title}

<!-- One-paragraph framing of what this change is trying to
     achieve. Plans reference this brief via `change.md`. -->
";

/// Title-case transform used by [`ChangeBrief::template`]:
/// `traffic-modernisation` → `Traffic modernisation`. Dashes become
/// spaces, the very first ASCII character is uppercased, everything
/// else is left as-is.
fn title_case(name: &str) -> String {
    let with_spaces: String = name.chars().map(|c| if c == '-' { ' ' } else { c }).collect();
    let mut chars = with_spaces.chars();
    chars
        .next()
        .map_or_else(String::new, |first| first.to_ascii_uppercase().to_string() + chars.as_str())
}
