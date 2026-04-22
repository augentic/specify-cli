//! Initiative brief parser — operator-authored brief
//! (RFC-3a §*The Initiative Brief*).
//!
//! `.specify/initiative.md` is a markdown document with a `---`-delimited
//! YAML frontmatter block at the top. The frontmatter shape is enforced
//! here (`#[serde(deny_unknown_fields)]` + [`InitiativeBrief::parse_str`]
//! invariants); the body is captured verbatim and **not** parsed in v1.
//! A future RFC may land structured body parsing, but today's consumers
//! treat the body as prose.
//!
//! No JSON schema file ships for v1 per the RFC — the shape is enforced
//! directly in code.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::brief::split_on_closing_delimiter;
use crate::registry::is_kebab_case;

/// In-memory representation of `.specify/initiative.md`.
///
/// Structured frontmatter (YAML) + free-form body (markdown). The
/// body is preserved byte-for-byte so round-tripping is faithful;
/// structured body interpretation is explicitly deferred.
#[derive(Debug, Clone, PartialEq)]
pub struct InitiativeBrief {
    /// Parsed YAML frontmatter.
    pub frontmatter: InitiativeFrontmatter,
    /// Markdown body captured verbatim. Not parsed further in v1.
    pub body: String,
}

/// Parsed frontmatter of `.specify/initiative.md`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitiativeFrontmatter {
    /// Kebab-case initiative name.
    pub name: String,
    /// Seed inputs. Optional — may be absent or empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InitiativeInput>,
}

/// One entry in [`InitiativeFrontmatter::inputs`].
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitiativeInput {
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

impl InitiativeBrief {
    /// Absolute path to `.specify/initiative.md` for a project dir.
    pub fn path(project_dir: &Path) -> PathBuf {
        project_dir.join(".specify").join("initiative.md")
    }

    /// Load + shape-validate the initiative brief.
    ///
    /// - `Ok(None)` — the file is absent. The brief is optional and a
    ///   missing file is *not* an error.
    /// - `Ok(Some(_))` — parsed and shape-validated.
    /// - `Err(_)` — malformed YAML, unknown keys, kebab-case / required-
    ///   field / empty-path violations.
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
    pub fn parse_str(content: &str) -> Result<Self, Error> {
        let after_open = content
            .strip_prefix("---\n")
            .or_else(|| content.strip_prefix("---\r\n"))
            .ok_or_else(|| Error::Config("initiative.md: missing YAML frontmatter".to_string()))?;
        let (frontmatter_text, body) = split_on_closing_delimiter(after_open).ok_or_else(|| {
            Error::Config("initiative.md: opening `---` has no closing `---` delimiter".to_string())
        })?;

        let frontmatter: InitiativeFrontmatter = serde_yaml::from_str(frontmatter_text)
            .map_err(|err| Error::Config(format!("initiative.md: invalid frontmatter: {err}")))?;

        let brief = InitiativeBrief {
            frontmatter,
            body: body.to_string(),
        };
        brief.validate_shape()?;
        Ok(brief)
    }

    /// Invariants serde cannot express: kebab-case name, non-empty
    /// input paths.
    pub fn validate_shape(&self) -> Result<(), Error> {
        if self.frontmatter.name.is_empty() {
            return Err(Error::Config("initiative.md: name is empty".to_string()));
        }
        if !is_kebab_case(&self.frontmatter.name) {
            return Err(Error::Config(format!(
                "initiative.md: name `{}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)",
                self.frontmatter.name
            )));
        }
        for (idx, input) in self.frontmatter.inputs.iter().enumerate() {
            if input.path.is_empty() {
                return Err(Error::Config(format!("initiative.md: inputs[{idx}].path is empty")));
            }
        }
        Ok(())
    }

    /// Render the canonical `.specify/initiative.md` template for the
    /// given kebab-case initiative name. Byte-stable — the `init` CLI
    /// verb compares against a golden fixture.
    pub fn template(name: &str) -> String {
        INITIATIVE_TEMPLATE.replace("{name}", name).replace("{title}", &title_case(name))
    }
}

/// Canonical template shipped by `specify initiative brief init`. The
/// golden-fixture test pins this byte-for-byte; any edit here must be
/// mirrored in the test constant.
const INITIATIVE_TEMPLATE: &str = "\
---
name: {name}
inputs: []
---

# {title}

<!-- One-paragraph framing of what this initiative is trying to
     achieve. Plans reference this brief via `.specify/initiative.md`. -->
";

/// Title-case transform used by [`InitiativeBrief::template`]:
/// `traffic-modernisation` → `Traffic modernisation`. Dashes become
/// spaces, the very first ASCII character is uppercased, everything
/// else is left as-is.
fn title_case(name: &str) -> String {
    let with_spaces: String = name.chars().map(|c| if c == '-' { ' ' } else { c }).collect();
    let mut chars = with_spaces.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}
