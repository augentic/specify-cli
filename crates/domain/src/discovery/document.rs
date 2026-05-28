//! In-memory model of `<project_dir>/discovery.md` — the
//! `## Lead inventory` section plus the surrounding operator
//! prose.
//!
//! discovery alias contract — `slices[].sources[].lead` resolves first against
//! a lead's `id`, then against any entry in `aliases[]`. The
//! `## Lead inventory` section uses the block grammar
//!
//! ```markdown
//! ### <id>
//!
//! - id: <id>
//! - aliases: [<alias>, <alias>]
//! - sources: [<key>, <key>]
//! - summary: <one-line summary>
//! ```
//!
//! [`Discovery::load`] parses the file faithfully (preserving prose
//! before, between, and after lead blocks) and exposes the
//! `## Lead inventory` section as structured [`Lead`]
//! rows. [`Discovery::write_atomic`] re-renders the file with any
//! mutations propagated; prose around lead blocks round-trips
//! unchanged.
//!
//! The on-disk format intentionally mirrors what source adapters
//! write at `survey` time, so the same parser feeds re-survey
//! and operator edits.

use std::collections::BTreeMap;
use std::path::Path;

use specify_error::{Error, Result};

use super::lead::{Lead, LeadAliases};
use crate::slice::atomic;

/// In-memory model of one `discovery.md` file.
///
/// Stores every lead block under the canonical `## Lead
/// inventory` heading plus the file's surrounding prose. Mutations
/// flow through the [`Lead`] accessors ([`Discovery::lead_mut`])
/// or the alias-focused helpers ([`Discovery::add_alias`] /
/// [`Discovery::remove_alias`]); [`Discovery::write_atomic`] persists
/// the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// Raw prose preceding `## Lead inventory`, with the trailing
    /// newline preserved. Empty when the file starts with the heading.
    prefix: String,
    /// Parsed lead inventory in document order.
    leads: Vec<Lead>,
    /// Raw prose following the last lead block (or following the
    /// `## Lead inventory` heading when no leads were
    /// declared). Empty when nothing trails the inventory.
    suffix: String,
    /// `true` when the input file contained a `## Lead inventory`
    /// heading. A discovery.md without the heading round-trips as
    /// pure prose (the `leads` vector is empty and the heading
    /// is appended on write when leads have been added).
    has_inventory_heading: bool,
}

impl Discovery {
    /// Parse `text` as the in-memory discovery document.
    ///
    /// The parser preserves all prose outside the `## Lead
    /// inventory` section verbatim. Inside the section, every
    /// `### <id>` block plus its bullet list is collected as a
    /// [`Lead`]; bullets are parsed line-by-line. Aliases use
    /// the inline `[a, b, c]` form per discovery alias contract.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Diag`] (`discovery-parse-failed`) on a
    /// structural defect — duplicate `id:` bullets, malformed
    /// `aliases:` value, missing required bullets.
    pub fn parse(text: &str) -> Result<Self> {
        Parser::new(text).run()
    }

    /// Load and parse `discovery.md` at `path`. Returns
    /// [`Error::ArtifactNotFound`] when the file is absent.
    ///
    /// # Errors
    ///
    /// - [`Error::ArtifactNotFound`] when the file does not exist.
    /// - [`Error::Filesystem`] on read failure.
    /// - [`Error::Diag`] on parse failure.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::ArtifactNotFound {
                kind: "discovery.md",
                path: path.to_path_buf(),
            });
        }
        let raw = std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw)
    }

    /// Re-render the document and atomically persist it at `path`.
    /// The atomic envelope is shared with every other `.specify/`
    /// writer ([`atomic::bytes_write`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the temp-file write / rename
    /// fails. Schema validation is the caller's responsibility — the
    /// document's invariants (single namespace for `id` + `aliases`,
    /// kebab-case) are enforced by [`Self::check_alias_collisions`]
    /// and the schema-side checks already wired in `slice validate`.
    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let body = self.render();
        atomic::bytes_write(path, body.as_bytes())
    }

    /// Locate a lead by its `id` for mutation. discovery alias contract calls
    /// out `id`-only addressing for amend-style operations (aliases
    /// are *resolved* against, not *addressed* by, on the amend
    /// path); use [`Self::resolve_lead`] when the caller wants
    /// to accept either form.
    #[must_use]
    pub fn lead_mut(&mut self, id: &str) -> Option<&mut Lead> {
        self.leads.iter_mut().find(|c| c.id == id)
    }

    /// Convenience wrapper around [`Self::lead_mut`] that
    /// converts the `None` arm into the canonical
    /// `discovery-lead-unknown` diagnostic. `flag` is the CLI
    /// flag token (e.g. `--add-alias`) the caller wants threaded
    /// through the operator-facing detail string.
    fn lead_mut_or_unknown(&mut self, id: &str, flag: &str) -> Result<&mut Lead> {
        self.lead_mut(id).ok_or_else(|| Error::Diag {
            code: "discovery-lead-unknown",
            detail: format!(
                "no lead `{id}` in discovery.md; {flag} must reference \
                 an existing lead id"
            ),
        })
    }

    /// Resolve a `--sources <key>=<value>` token to its lead
    /// per discovery alias contract. Walks every lead, returning a hit when
    /// `token` matches the lead's `id` or any of its
    /// `aliases[]`. Multiple hits surface as
    /// [`ResolveError::Collision`] — the document is invalid, not
    /// the input.
    ///
    /// # Errors
    ///
    /// - [`ResolveError::Unknown`] when no lead resolves
    ///   `token`.
    /// - [`ResolveError::Collision`] when more than one lead
    ///   resolves `token` — caller surfaces this as
    ///   `discovery-alias-collision`.
    pub fn resolve_lead(&self, token: &str) -> std::result::Result<&Lead, ResolveError> {
        let hits: Vec<&Lead> = self.leads.iter().filter(|c| c.resolves(token)).collect();
        match hits.len() {
            0 => Err(ResolveError::Unknown {
                token: token.to_string(),
            }),
            1 => Ok(hits[0]),
            _ => {
                let mut owners: Vec<String> = hits.iter().map(|c| c.id.clone()).collect();
                owners.sort();
                Err(ResolveError::Collision {
                    token: token.to_string(),
                    leads: owners,
                })
            }
        }
    }

    /// Walk every `id` and every `aliases[]` entry across all
    /// leads, returning every namespace collision sorted
    /// deterministically.
    ///
    /// The single-namespace rule per discovery alias contract: an alias MUST NOT
    /// collide with ANY other lead's `id` or `aliases[]` in the
    /// same `discovery.md`. Findings sort lexicographically on the
    /// colliding name, then by the bearing lead id list so
    /// repeat runs produce byte-identical error envelopes.
    #[must_use]
    pub fn check_alias_collisions(&self) -> Vec<DiscoveryAliasCollision> {
        let mut owners_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for lead in &self.leads {
            owners_by_name.entry(lead.id.clone()).or_default().push(lead.id.clone());
            for alias in &lead.aliases.names {
                owners_by_name.entry(alias.clone()).or_default().push(lead.id.clone());
            }
        }

        let mut findings: Vec<DiscoveryAliasCollision> = Vec::new();
        for (name, owners) in owners_by_name {
            if owners.len() <= 1 {
                continue;
            }
            let mut bearing: Vec<String> = owners;
            bearing.sort();
            bearing.dedup();
            findings.push(DiscoveryAliasCollision {
                name,
                bearing_leads: bearing,
            });
        }
        findings.sort_by(|a, b| {
            a.name.cmp(&b.name).then_with(|| a.bearing_leads.cmp(&b.bearing_leads))
        });
        findings
    }

    /// Append `alias` to the named lead's `aliases[]`. Refuses
    /// when the alias would shadow the lead's own id; runs the
    /// whole-document collision check on the result and refuses the
    /// edit (the mutation is reverted) when any cross-lead
    /// collision fires.
    ///
    /// # Errors
    ///
    /// - [`Error::Diag`] (`discovery-lead-unknown`) when no
    ///   lead with id `lead_id` exists.
    /// - [`Error::Validation`] (`discovery-alias-collision`) when
    ///   the operator-supplied alias collides with an existing
    ///   namespace entry (self-shadow or cross-lead).
    pub fn add_alias(&mut self, lead_id: &str, alias: &str) -> Result<()> {
        let lead = self.lead_mut_or_unknown(lead_id, "--add-alias")?;
        lead.add_alias(alias.to_string()).map_err(|collision| {
            Error::validation_failed(
                "discovery-alias-collision",
                "alias must not collide with the bearing lead's own id",
                collision.to_string(),
            )
        })?;
        // Whole-document collision gate: refuse the edit on any
        // cross-lead clash so the on-disk document never
        // contains an unresolvable namespace.
        let collisions = self.check_alias_collisions();
        if !collisions.is_empty() {
            // Roll back the mutation we just applied so the in-memory
            // model reflects the on-disk state. Cross-lead
            // collisions can only involve this alias (every other
            // pair was clean before the mutation), so removing the
            // alias from this lead is sufficient.
            if let Some(lead) = self.lead_mut(lead_id) {
                lead.remove_alias(alias);
            }
            return Err(Self::collision_error(&collisions));
        }
        Ok(())
    }

    /// Remove `alias` from the named lead's `aliases[]`.
    /// Idempotent — silently returns when the alias is not present.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Diag`] (`discovery-lead-unknown`) when
    /// no lead with id `lead_id` exists. Operator-issued
    /// removals against a missing lead are a typo, not a
    /// no-op.
    pub fn remove_alias(&mut self, lead_id: &str, alias: &str) -> Result<()> {
        let lead = self.lead_mut_or_unknown(lead_id, "--remove-alias")?;
        lead.remove_alias(alias);
        Ok(())
    }

    /// Convert a non-empty list of collision findings into the
    /// single [`Error::Validation`] envelope the CLI emits. Shared
    /// between [`Self::add_alias`] and `specrun slice validate`.
    #[must_use]
    pub fn collision_error(findings: &[DiscoveryAliasCollision]) -> Error {
        let results: Vec<specify_error::ValidationSummary> =
            findings.iter().map(DiscoveryAliasCollision::to_summary).collect();
        Error::Validation { results }
    }

    /// Render the document back to its on-disk shape. Prose
    /// surrounding the inventory section round-trips byte-for-byte
    /// when no lead edits were applied. The
    /// `## Lead inventory` heading is synthesised when the
    /// input lacked one but leads have been added in-memory
    /// (currently never reached from the CLI; future-proofing for
    /// programmatic Discovery construction).
    fn render(&self) -> String {
        let mut out = String::with_capacity(self.prefix.len() + self.suffix.len() + 128);
        out.push_str(&self.prefix);
        if self.has_inventory_heading || !self.leads.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("## Lead inventory\n\n");
            for (idx, lead) in self.leads.iter().enumerate() {
                if idx > 0 {
                    out.push('\n');
                }
                render_lead(&mut out, lead);
            }
        }
        if !self.suffix.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&self.suffix);
        }
        out
    }
}

/// Render a single `### <id>` block onto `out`. Bullet order mirrors
/// discovery alias contract: `id`, optional `aliases`, `sources`, `summary`, plus
/// the optional `tentative` flag.
fn render_lead(out: &mut String, lead: &Lead) {
    out.push_str("### ");
    out.push_str(&lead.id);
    out.push_str("\n\n");
    out.push_str("- id: ");
    out.push_str(&lead.id);
    out.push('\n');
    if !lead.aliases.is_empty() {
        out.push_str("- aliases: [");
        out.push_str(&lead.aliases.names.join(", "));
        out.push_str("]\n");
    }
    out.push_str("- sources: [");
    out.push_str(&lead.sources.join(", "));
    out.push_str("]\n");
    out.push_str("- summary: ");
    out.push_str(&lead.summary);
    out.push('\n');
    if let Some(tentative) = lead.tentative
        && tentative
    {
        out.push_str("- tentative: true\n");
    }
}

/// One alias-collision finding emitted by
/// [`Discovery::check_alias_collisions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAliasCollision {
    /// Namespace entry that resolves to more than one lead.
    pub name: String,
    /// Sorted, de-duplicated list of lead ids that own the
    /// colliding name (either as `id` or as a member of `aliases[]`).
    pub bearing_leads: Vec<String>,
}

impl DiscoveryAliasCollision {
    /// Project the finding into the [`specify_error::ValidationSummary`]
    /// shape `specrun slice validate` and `specrun plan amend` emit.
    #[must_use]
    pub fn to_summary(&self) -> specify_error::ValidationSummary {
        specify_error::ValidationSummary {
            status: specify_error::ValidationStatus::Fail,
            rule_id: "discovery-alias-collision".to_string(),
            rule: "lead id and aliases share a single namespace per discovery.md".to_string(),
            detail: Some(format!(
                "name `{}` resolves to multiple leads: {}",
                self.name,
                self.bearing_leads.join(", ")
            )),
        }
    }
}

/// Outcome of [`Discovery::resolve_lead`] when the supplied
/// token cannot be reduced to exactly one lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No lead has an `id` or `aliases[]` entry matching
    /// `token`.
    Unknown {
        /// Operator-supplied value that failed to resolve.
        token: String,
    },
    /// More than one lead resolves `token`. The discovery
    /// document is itself invalid; the caller emits a
    /// `discovery-alias-collision` error referring to the bearing
    /// leads.
    Collision {
        /// Operator-supplied value that resolved to multiple
        /// leads.
        token: String,
        /// Sorted, de-duplicated list of lead ids that own
        /// `token`.
        leads: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { token } => {
                write!(f, "no lead in discovery.md has an id or alias matching `{token}`")
            }
            Self::Collision { token, leads } => write!(
                f,
                "`{token}` resolves to multiple leads in discovery.md: {}",
                leads.join(", ")
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Hand-rolled, single-pass parser for `discovery.md`.
///
/// We avoid pulling a Markdown crate to keep the
/// parser narrowly scoped to the section grammar we actually need.
/// Prose outside `## Lead inventory` is preserved verbatim;
/// the inventory section is parsed line-by-line into [`Lead`]
/// rows.
struct Parser<'a> {
    lines: Vec<&'a str>,
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        // `split_inclusive('\n')` preserves the trailing newline so
        // we can round-trip the file byte-for-byte even when the
        // input lacks a final newline.
        Self {
            lines: text.split_inclusive('\n').collect(),
            cursor: 0,
        }
    }

    fn run(mut self) -> Result<Discovery> {
        let prefix = self.consume_until_inventory();
        let has_inventory_heading = self.cursor < self.lines.len();
        let leads = if has_inventory_heading {
            // Skip the `## Lead inventory` line.
            self.cursor += 1;
            self.parse_leads()?
        } else {
            Vec::new()
        };
        let suffix = self.lines[self.cursor..].concat();
        Ok(Discovery {
            prefix,
            leads,
            suffix,
            has_inventory_heading,
        })
    }

    /// Walk until the canonical `## Lead inventory` heading is
    /// found. Returns the accumulated prose. Anything that survives
    /// `cursor` after this call is either the heading line itself or
    /// the end of the file.
    fn consume_until_inventory(&mut self) -> String {
        let mut out = String::new();
        while self.cursor < self.lines.len() {
            let line = self.lines[self.cursor];
            if is_inventory_heading(line) {
                return out;
            }
            out.push_str(line);
            self.cursor += 1;
        }
        out
    }

    fn parse_leads(&mut self) -> Result<Vec<Lead>> {
        let mut out: Vec<Lead> = Vec::new();
        while self.cursor < self.lines.len() {
            let line = self.lines[self.cursor];
            if is_top_level_heading(line) {
                // Some other `## …` section ends the inventory.
                break;
            }
            if is_lead_heading(line) {
                let lead = self.parse_lead_block()?;
                out.push(lead);
                continue;
            }
            // Blank lines and stray prose between lead blocks
            // are skipped; the bullets carry the data so any
            // surrounding decoration round-trips through the
            // re-render path's stable formatting.
            self.cursor += 1;
        }
        Ok(out)
    }

    fn parse_lead_block(&mut self) -> Result<Lead> {
        let heading = self.lines[self.cursor];
        let id_from_heading = lead_heading_id(heading).unwrap_or("").trim().to_string();
        self.cursor += 1;

        let mut id: Option<String> = None;
        let mut sources: Option<Vec<String>> = None;
        let mut summary: Option<String> = None;
        let mut aliases: Option<Vec<String>> = None;
        let mut tentative: Option<bool> = None;

        while self.cursor < self.lines.len() {
            let raw = self.lines[self.cursor];
            if is_lead_heading(raw) || is_top_level_heading(raw) {
                break;
            }
            let trimmed = strip_newline(raw).trim_start();
            if let Some(bullet_body) = bullet_body(trimmed) {
                let (key, value) = split_bullet(bullet_body)?;
                match key {
                    "id" => {
                        if id.is_some() {
                            return Err(parse_err(format!(
                                "lead `{id_from_heading}`: duplicate `id:` bullet"
                            )));
                        }
                        id = Some(value.to_string());
                    }
                    "sources" => {
                        sources = Some(parse_inline_list(value, "sources")?);
                    }
                    "summary" => {
                        summary = Some(value.to_string());
                    }
                    "aliases" => {
                        aliases = Some(parse_inline_list(value, "aliases")?);
                    }
                    "tentative" => match value {
                        "true" => tentative = Some(true),
                        "false" => tentative = Some(false),
                        other => {
                            return Err(parse_err(format!(
                                "lead `{id_from_heading}`: tentative must be true or false, \
                                 got `{other}`"
                            )));
                        }
                    },
                    other => {
                        return Err(parse_err(format!(
                            "lead `{id_from_heading}`: unknown bullet `{other}`"
                        )));
                    }
                }
            }
            self.cursor += 1;
        }

        let id = id.unwrap_or_else(|| id_from_heading.clone());
        let sources = sources
            .ok_or_else(|| parse_err(format!("lead `{id}` is missing the `sources:` bullet")))?;
        let summary = summary
            .ok_or_else(|| parse_err(format!("lead `{id}` is missing the `summary:` bullet")))?;
        let aliases = aliases.unwrap_or_default();
        Ok(Lead {
            id,
            sources,
            summary,
            tentative,
            aliases: LeadAliases { names: aliases },
        })
    }
}

fn is_inventory_heading(line: &str) -> bool {
    let trimmed = strip_newline(line).trim();
    trimmed.eq_ignore_ascii_case("## Lead inventory")
}

fn is_top_level_heading(line: &str) -> bool {
    let trimmed = strip_newline(line);
    trimmed.starts_with("## ") && !is_inventory_heading(line)
}

fn is_lead_heading(line: &str) -> bool {
    let trimmed = strip_newline(line);
    trimmed.starts_with("### ")
}

fn lead_heading_id(line: &str) -> Option<&str> {
    let trimmed = strip_newline(line);
    trimmed.strip_prefix("### ")
}

fn strip_newline(line: &str) -> &str {
    line.strip_suffix('\n').map_or(line, |s| s.strip_suffix('\r').unwrap_or(s))
}

fn bullet_body(trimmed: &str) -> Option<&str> {
    trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "))
}

fn split_bullet(body: &str) -> Result<(&str, &str)> {
    let (key, value) = body
        .split_once(':')
        .ok_or_else(|| parse_err(format!("bullet `{body}` must use `key: value` form")))?;
    Ok((key.trim(), value.trim()))
}

fn parse_inline_list(value: &str, field: &'static str) -> Result<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| parse_err(format!("`{field}:` must be wrapped in `[…]`, got `{value}`")))?;
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    Ok(inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

const fn parse_err(detail: String) -> Error {
    Error::Diag {
        code: "discovery-parse-failed",
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Discovery

Some prose before the inventory.

## Lead inventory

### user-registration

- id: user-registration
- aliases: [account-registration, user-signup]
- sources: [legacy, runtime]
- summary: Registration endpoint accepting email + password.

### password-reset-request

- id: password-reset-request
- aliases: [password-reset]
- sources: [legacy]
- summary: Reset endpoint.

## Notes

Some trailing prose.
";

    #[test]
    fn parses_canonical_layout() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        assert_eq!(doc.leads.len(), 2);
        assert_eq!(doc.leads[0].id, "user-registration");
        assert_eq!(doc.leads[0].aliases.names, vec!["account-registration", "user-signup"]);
        assert_eq!(doc.leads[1].id, "password-reset-request");
        assert_eq!(doc.leads[1].aliases.names, vec!["password-reset"]);
    }

    #[test]
    fn round_trips_byte_stable_when_unchanged() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let rendered = doc.render();
        let reparsed = Discovery::parse(&rendered).expect("reparse ok");
        assert_eq!(doc.leads, reparsed.leads);
    }

    #[test]
    fn resolve_lead_matches_id() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let hit = doc.resolve_lead("user-registration").expect("resolves");
        assert_eq!(hit.id, "user-registration");
    }

    #[test]
    fn resolve_lead_matches_alias() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let hit = doc.resolve_lead("password-reset").expect("resolves via alias");
        assert_eq!(hit.id, "password-reset-request");
    }

    #[test]
    fn resolve_lead_unknown_errors() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let err = doc.resolve_lead("never-heard-of-it").expect_err("unknown errs");
        match err {
            ResolveError::Unknown { token } => assert_eq!(token, "never-heard-of-it"),
            ResolveError::Collision { .. } => panic!("expected Unknown, got Collision"),
        }
    }

    #[test]
    fn resolve_lead_collision_errors() {
        let yaml = "\
## Lead inventory

### a

- id: a
- aliases: [shared]
- sources: [legacy]
- summary: A.

### b

- id: b
- aliases: [shared]
- sources: [legacy]
- summary: B.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        let err = doc.resolve_lead("shared").expect_err("collision errs");
        match err {
            ResolveError::Collision { token, leads } => {
                assert_eq!(token, "shared");
                assert_eq!(leads, vec!["a".to_string(), "b".to_string()]);
            }
            ResolveError::Unknown { .. } => panic!("expected Collision, got Unknown"),
        }
    }

    #[test]
    fn check_alias_collisions_id_vs_id() {
        // Manually construct a Discovery with a duplicate id (the
        // parser doesn't reject this — the schema check upstream
        // would, but this gate is the cross-check for hand-edited
        // discovery.md files).
        let doc = Discovery {
            prefix: String::new(),
            has_inventory_heading: true,
            suffix: String::new(),
            leads: vec![
                Lead {
                    id: "a".to_string(),
                    sources: vec!["legacy".to_string()],
                    summary: "A.".to_string(),
                    tentative: None,
                    aliases: LeadAliases::default(),
                },
                Lead {
                    id: "a".to_string(),
                    sources: vec!["legacy".to_string()],
                    summary: "Duplicate id.".to_string(),
                    tentative: None,
                    aliases: LeadAliases::default(),
                },
            ],
        };
        let findings = doc.check_alias_collisions();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "a");
        assert_eq!(findings[0].bearing_leads, vec!["a".to_string()]);
    }

    #[test]
    fn check_alias_collisions_id_vs_alias() {
        let yaml = "\
## Lead inventory

### a

- id: a
- sources: [legacy]
- summary: A.

### b

- id: b
- aliases: [a]
- sources: [legacy]
- summary: B aliases a's id.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        let findings = doc.check_alias_collisions();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "a");
        assert_eq!(findings[0].bearing_leads, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn check_alias_collisions_alias_vs_alias() {
        let yaml = "\
## Lead inventory

### a

- id: a
- aliases: [shared]
- sources: [legacy]
- summary: A.

### b

- id: b
- aliases: [shared]
- sources: [legacy]
- summary: B.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        let findings = doc.check_alias_collisions();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "shared");
        assert_eq!(findings[0].bearing_leads, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn check_alias_collisions_no_findings_on_clean_doc() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let findings = doc.check_alias_collisions();
        assert!(findings.is_empty(), "clean doc must produce no findings; got: {findings:?}");
    }

    #[test]
    fn add_alias_persists_through_render() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        doc.add_alias("password-reset-request", "pwd-reset").expect("add ok");
        let rendered = doc.render();
        let reparsed = Discovery::parse(&rendered).expect("reparse ok");
        let lead =
            reparsed.leads.iter().find(|c| c.id == "password-reset-request").expect("present");
        assert!(lead.aliases.contains("pwd-reset"));
        assert!(lead.aliases.contains("password-reset"), "preserves existing aliases");
    }

    #[test]
    fn add_alias_refuses_collision_with_other_id() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        let err =
            doc.add_alias("password-reset-request", "user-registration").expect_err("collision");
        match err {
            Error::Validation { results } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].rule_id, "discovery-alias-collision");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
        // Ensure the mutation rolled back so subsequent edits start
        // from the same state the operator saw on disk.
        let lead = doc.leads.iter().find(|c| c.id == "password-reset-request").expect("present");
        assert!(!lead.aliases.contains("user-registration"));
    }

    #[test]
    fn add_alias_unknown_lead_errors() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        let err = doc.add_alias("nope", "x").expect_err("unknown");
        match err {
            Error::Diag { code, .. } => assert_eq!(code, "discovery-lead-unknown"),
            other => panic!("expected Diag, got: {other:?}"),
        }
    }

    #[test]
    fn remove_alias_idempotent_when_absent() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        doc.remove_alias("password-reset-request", "never-set").expect("no-op ok");
        let lead = doc.leads.iter().find(|c| c.id == "password-reset-request").expect("present");
        assert!(lead.aliases.contains("password-reset"));
    }

    #[test]
    fn remove_alias_drops_named_entry() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        doc.remove_alias("password-reset-request", "password-reset").expect("removed");
        let lead = doc.leads.iter().find(|c| c.id == "password-reset-request").expect("present");
        assert!(!lead.aliases.contains("password-reset"));
    }

    #[test]
    fn parses_block_without_aliases_bullet() {
        let yaml = "\
## Lead inventory

### a

- id: a
- sources: [legacy]
- summary: A.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        assert!(doc.leads[0].aliases.is_empty());
    }
}
