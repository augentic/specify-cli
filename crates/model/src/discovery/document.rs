//! In-memory model of `<project_dir>/discovery.md` — the
//! `## Lead inventory` section plus the surrounding operator
//! prose.
//!
//! discovery alias contract — `slices[].sources[].lead` resolves
//! first against a lead's `lead`, then against any entry in
//! `aliases[]`, within that binding's `source`. Each block is a
//! raw, unmerged lead identified by the `(source, lead)` pair.
//! The `## Lead inventory` section uses the block grammar
//!
//! ```markdown
//! ### <source>:<lead>
//!
//! - lead: <lead>
//! - source: <source>
//! - aliases: [<alias>, <alias>]
//! - synopsis: <reconciliation-grade headline>
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

use specify_diagnostics::{Artifact, Diagnostic};
use specify_error::{Error, Result};

use super::lead::{Lead, LeadAliases};
use crate::atomic;

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

    /// Parse a survey `lead-set.md` artifact.
    ///
    /// Survey briefs ask agents to write only `### <lead>` blocks; the
    /// CLI owns the surrounding `## Lead inventory` frame. This accepts
    /// both framed and unframed lead sets, then delegates to the strict
    /// discovery parser.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Diag`] (`discovery-parse-failed`) when the
    /// normalized lead set has the same structural defects rejected by
    /// [`Self::parse`].
    pub fn parse_lead_set(text: &str) -> Result<Self> {
        if text.lines().any(is_inventory_heading) {
            Self::parse(text)
        } else {
            let mut normalized = String::with_capacity("## Lead inventory\n\n".len() + text.len());
            normalized.push_str("## Lead inventory\n\n");
            normalized.push_str(text);
            Self::parse(&normalized)
        }
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

    /// Borrow the parsed lead inventory in document order.
    ///
    /// Read-only view used by the `survey` runner to echo the existing
    /// lead set for a source into the agent handoff envelope (RFC-29 D1;
    /// DECISIONS.md §"Source operations (D1)") without taking ownership
    /// of the document.
    #[must_use]
    pub fn leads(&self) -> &[Lead] {
        &self.leads
    }

    /// Consume the document and return its lead inventory in document
    /// order.
    ///
    /// The `survey` finalize path parses the agent- / tool-produced
    /// `lead-set.md` into a [`Discovery`] and lifts the leads out for
    /// schema validation and [`Self::merge_survey`]; the surrounding
    /// prose of a lead-set artifact is discarded.
    #[must_use]
    pub fn into_leads(self) -> Vec<Lead> {
        self.leads
    }

    /// Locate a lead by its `id` for mutation. discovery alias contract calls
    /// out `id`-only addressing for amend-style operations (aliases
    /// are *resolved* against, not *addressed* by, on the amend
    /// path); use [`Self::resolve_lead`] when the caller wants
    /// to accept either form.
    #[must_use]
    pub fn lead_mut(&mut self, id: &str) -> Option<&mut Lead> {
        self.leads.iter_mut().find(|c| c.lead == id)
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
                let mut owners: Vec<String> = hits.iter().map(|c| c.lead.clone()).collect();
                owners.sort();
                Err(ResolveError::Collision {
                    token: token.to_string(),
                    leads: owners,
                })
            }
        }
    }

    /// Walk every `lead` and every `aliases[]` entry across all
    /// leads, returning every namespace collision sorted
    /// deterministically.
    ///
    /// The single-namespace rule per discovery alias contract is
    /// scoped **per `source`**: an alias MUST NOT collide with
    /// another lead's `lead` or `aliases[]` under the *same*
    /// `source`. The same `lead` under a different
    /// `source` is legal — leads are raw and per-source, and
    /// cross-source unification happens at plan time. Findings sort
    /// by `source`, then lexicographically on the colliding name,
    /// then by the bearing lead list so repeat runs produce
    /// byte-identical error envelopes.
    #[must_use]
    pub fn check_alias_collisions(&self) -> Vec<DiscoveryAliasCollision> {
        let mut owners_by_key: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for lead in &self.leads {
            owners_by_key
                .entry((lead.source.clone(), lead.lead.clone()))
                .or_default()
                .push(lead.lead.clone());
            for alias in &lead.aliases.names {
                owners_by_key
                    .entry((lead.source.clone(), alias.clone()))
                    .or_default()
                    .push(lead.lead.clone());
            }
        }

        let mut findings: Vec<DiscoveryAliasCollision> = Vec::new();
        for ((source, name), owners) in owners_by_key {
            if owners.len() <= 1 {
                continue;
            }
            let mut bearing: Vec<String> = owners;
            bearing.sort();
            bearing.dedup();
            findings.push(DiscoveryAliasCollision {
                source,
                name,
                bearing_leads: bearing,
            });
        }
        findings.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.bearing_leads.cmp(&b.bearing_leads))
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
    ///   lead with id `lead` exists.
    /// - [`Error::Validation`] (`discovery-alias-collision`) when
    ///   the operator-supplied alias collides with an existing
    ///   namespace entry (self-shadow or cross-lead).
    pub fn add_alias(&mut self, lead: &str, alias: &str) -> Result<()> {
        let entry = self.lead_mut_or_unknown(lead, "--add-alias")?;
        entry.add_alias(alias.to_string()).map_err(|collision| {
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
            if let Some(entry) = self.lead_mut(lead) {
                entry.remove_alias(alias);
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
    /// no lead with id `lead` exists. Operator-issued
    /// removals against a missing lead are a typo, not a
    /// no-op.
    pub fn remove_alias(&mut self, lead: &str, alias: &str) -> Result<()> {
        let entry = self.lead_mut_or_unknown(lead, "--remove-alias")?;
        entry.remove_alias(alias);
        Ok(())
    }

    /// Merge a re-survey of `source` into the inventory and
    /// atomically persist the result at `path`.
    ///
    /// `leads` is the lead set the source's `survey` produced (already
    /// validated against `schemas/discovery/lead.schema.json` by the
    /// caller). The surveying source owns attribution: every incoming
    /// lead's `source` is force-set to `source`, so a survey
    /// for one source only ever writes that source's blocks. Each
    /// incoming lead replaces the prior block sharing its
    /// `(source, lead)` pair **in place**, so a surviving lead
    /// keeps its document position and the file re-renders byte-stably
    /// when nothing moved. Operator-authored `aliases[]` on a
    /// surviving pair are carried forward (unioned with any alias the
    /// re-survey itself emits) per discovery alias contract. Incoming
    /// leads with no prior block are appended in survey order. Prior
    /// blocks from *other* source keys, and prior blocks of this
    /// source whose `lead` is absent from the incoming set, are left
    /// untouched — re-survey replaces by `(source, lead)`, it
    /// does not prune and never collapses across sources.
    ///
    /// The whole-document collision gate runs against the post-merge
    /// state *before* any byte is written: on a
    /// [`Self::check_alias_collisions`] hit the in-memory model is rolled
    /// back and the file is left untouched, so a failed merge never lands
    /// partial state on disk.
    ///
    /// # Errors
    ///
    /// - [`Error::Validation`] (`discovery-alias-collision`) when the
    ///   post-merge inventory contains any namespace collision; nothing
    ///   is written and `self` is restored to its pre-merge state.
    /// - [`Error::Io`] when the atomic re-render fails.
    pub fn merge_survey(&mut self, source: &str, leads: Vec<Lead>, path: &Path) -> Result<()> {
        let mut slots: Vec<Option<Lead>> = leads
            .into_iter()
            .map(|mut lead| {
                // The surveying source owns attribution: a survey for
                // `source` produces `source`'s leads, period.
                lead.source = source.to_string();
                Some(lead)
            })
            .collect();
        // First incoming slot per lead (a valid lead set has unique
        // leads within a single source).
        let mut slot_by_lead: BTreeMap<String, usize> = BTreeMap::new();
        for (idx, slot) in slots.iter().enumerate() {
            if let Some(lead) = slot {
                slot_by_lead.entry(lead.lead.clone()).or_insert(idx);
            }
        }

        let mut merged: Vec<Lead> = Vec::with_capacity(self.leads.len() + slots.len());
        for prior in &self.leads {
            // Only this source's prior blocks are eligible for
            // replacement; other source keys pass through untouched so
            // a survey never collapses leads across sources.
            let replacement = if prior.source == source {
                slot_by_lead.get(&prior.lead).and_then(|&idx| slots[idx].take())
            } else {
                None
            };
            match replacement {
                Some(mut next) => {
                    // Surviving (source, lead): carry
                    // operator-authored aliases forward, unioning any
                    // the re-survey itself emits so the namespace never
                    // silently narrows.
                    let mut names = prior.aliases.names.clone();
                    for alias in &next.aliases.names {
                        if !names.contains(alias) {
                            names.push(alias.clone());
                        }
                    }
                    next.aliases.names = names;
                    merged.push(next);
                }
                None => merged.push(prior.clone()),
            }
        }
        // Append incoming leads with no prior block, in survey order.
        for slot in &mut slots {
            if let Some(lead) = slot.take() {
                merged.push(lead);
            }
        }

        // Validate-before-write: stage the merge, gate on the whole-
        // document collision check, and roll back the in-memory model on
        // any hit so the file (and `self`) reflect the pre-merge state.
        let prior_leads = std::mem::replace(&mut self.leads, merged);
        let collisions = self.check_alias_collisions();
        if !collisions.is_empty() {
            self.leads = prior_leads;
            return Err(Self::collision_error(&collisions));
        }
        self.write_atomic(path)
    }

    /// Convert a non-empty list of collision findings into the
    /// payload-free [`Error::Validation`] envelope the operational
    /// `add_alias` path emits (exit 2). The `slice validate` surface
    /// instead renders [`DiscoveryAliasCollision::to_diagnostic`] on
    /// stdout; this constructor is the single-shot operational signal.
    #[must_use]
    pub fn collision_error(findings: &[DiscoveryAliasCollision]) -> Error {
        let detail =
            findings.iter().map(DiscoveryAliasCollision::detail).collect::<Vec<_>>().join("; ");
        Error::Validation {
            code: "discovery-alias-collision".to_string(),
            detail,
        }
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

/// Render a single `### <source>:<lead>` block onto `out`.
/// Bullet order mirrors discovery alias contract: `lead`,
/// `source`, optional `aliases`, `synopsis`.
fn render_lead(out: &mut String, lead: &Lead) {
    out.push_str("### ");
    out.push_str(&lead.source);
    out.push(':');
    out.push_str(&lead.lead);
    out.push_str("\n\n");
    out.push_str("- lead: ");
    out.push_str(&lead.lead);
    out.push('\n');
    out.push_str("- source: ");
    out.push_str(&lead.source);
    out.push('\n');
    if !lead.aliases.is_empty() {
        out.push_str("- aliases: [");
        out.push_str(&lead.aliases.names.join(", "));
        out.push_str("]\n");
    }
    out.push_str("- synopsis: ");
    out.push_str(&lead.synopsis);
    out.push('\n');
}

/// One alias-collision finding emitted by
/// [`Discovery::check_alias_collisions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAliasCollision {
    /// Source key whose per-source namespace the collision occurs in.
    pub source: String,
    /// Namespace entry that resolves to more than one lead under
    /// `source`.
    pub name: String,
    /// Sorted, de-duplicated list of leads that own the colliding
    /// name (either as `lead` or as a member of `aliases[]`).
    pub bearing_leads: Vec<String>,
}

impl DiscoveryAliasCollision {
    /// Human-readable detail naming the colliding name, its
    /// `source`, and the bearing leads.
    #[must_use]
    pub fn detail(&self) -> String {
        format!(
            "name `{}` resolves to multiple leads under source `{}`: {}",
            self.name,
            self.source,
            self.bearing_leads.join(", ")
        )
    }

    /// Project the finding into the neutral [`Diagnostic`] currency the
    /// `specrun slice validate` surface renders. A deterministic
    /// `violation` against the `plan`/discovery artifact.
    #[must_use]
    pub fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic::violation(
            "discovery-alias-collision",
            "lead id and aliases share a single namespace per discovery.md",
            self.detail(),
            Artifact::Plan,
            None,
        )
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
        let heading_label = lead_heading_id(heading).unwrap_or("").trim().to_string();
        self.cursor += 1;

        let mut lead: Option<String> = None;
        let mut source: Option<String> = None;
        let mut synopsis: Option<String> = None;
        let mut aliases: Option<Vec<String>> = None;

        while self.cursor < self.lines.len() {
            let raw = self.lines[self.cursor];
            if is_lead_heading(raw) || is_top_level_heading(raw) {
                break;
            }
            let trimmed = strip_newline(raw).trim_start();
            if let Some(bullet_body) = bullet_body(trimmed) {
                let (key, value) = split_bullet(bullet_body)?;
                match key {
                    "lead" => {
                        if lead.is_some() {
                            return Err(parse_err(format!(
                                "lead `{heading_label}`: duplicate `lead:` bullet"
                            )));
                        }
                        lead = Some(value.to_string());
                    }
                    "source" => {
                        source = Some(value.to_string());
                    }
                    "synopsis" => {
                        synopsis = Some(value.to_string());
                    }
                    "aliases" => {
                        aliases = Some(parse_inline_list(value, "aliases")?);
                    }
                    other => {
                        return Err(parse_err(format!(
                            "lead `{heading_label}`: unknown bullet `{other}`"
                        )));
                    }
                }
            }
            self.cursor += 1;
        }

        let lead = lead.ok_or_else(|| {
            parse_err(format!("lead `{heading_label}` is missing the `lead:` bullet"))
        })?;
        // `source` is optional on parse: a `survey` lead-set omits
        // it (attribution is CLI-owned via `merge_survey`), while a
        // persisted `discovery.md` always carries it. The schema
        // (`required: [lead, source, synopsis]`) enforces presence
        // on the merged document.
        let source = source.unwrap_or_default();
        let synopsis = synopsis
            .ok_or_else(|| parse_err(format!("lead `{lead}` is missing the `synopsis:` bullet")))?;
        let aliases = aliases.unwrap_or_default();
        Ok(Lead {
            lead,
            source,
            synopsis,
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

### legacy:user-registration

- lead: user-registration
- source: legacy
- aliases: [account-registration, user-signup]
- synopsis: Registration endpoint accepting email + password.

### legacy:password-reset-request

- lead: password-reset-request
- source: legacy
- aliases: [password-reset]
- synopsis: Reset endpoint.

## Notes

Some trailing prose.
";

    #[test]
    fn parses_canonical_layout() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        assert_eq!(doc.leads.len(), 2);
        assert_eq!(doc.leads[0].lead, "user-registration");
        assert_eq!(doc.leads[0].source, "legacy");
        assert_eq!(doc.leads[0].aliases.names, vec!["account-registration", "user-signup"]);
        assert_eq!(doc.leads[1].lead, "password-reset-request");
        assert_eq!(doc.leads[1].aliases.names, vec!["password-reset"]);
    }

    #[test]
    fn parse_lead_set_accepts_headingless_blocks() {
        let doc = Discovery::parse_lead_set(
            "\
### user-registration

- lead: user-registration
- aliases: [signup]
- synopsis: Registration endpoint.
",
        )
        .expect("parse ok");

        assert_eq!(doc.leads.len(), 1);
        assert_eq!(doc.leads[0].lead, "user-registration");
        assert_eq!(doc.leads[0].source, "");
        assert_eq!(doc.leads[0].aliases.names, vec!["signup"]);
    }

    #[test]
    fn parse_lead_set_accepts_existing_inventory_heading() {
        let lead_set = "\
## Lead inventory

### user-registration

- lead: user-registration
- synopsis: Registration endpoint.
";
        let framed = Discovery::parse(lead_set).expect("parse ok");
        let lead_set = Discovery::parse_lead_set(lead_set).expect("parse lead set ok");

        assert_eq!(lead_set, framed);
    }

    #[test]
    fn parse_lead_set_accepts_whitespace_only_content() {
        let doc = Discovery::parse_lead_set("\n  \n").expect("parse ok");

        assert!(doc.leads.is_empty());
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
        assert_eq!(hit.lead, "user-registration");
    }

    #[test]
    fn resolve_lead_matches_alias() {
        let doc = Discovery::parse(SAMPLE).expect("parse ok");
        let hit = doc.resolve_lead("password-reset").expect("resolves via alias");
        assert_eq!(hit.lead, "password-reset-request");
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

### legacy:a

- lead: a
- source: legacy
- aliases: [shared]
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [shared]
- synopsis: B.
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
                    lead: "a".to_string(),
                    source: "legacy".to_string(),
                    synopsis: "A.".to_string(),
                    aliases: LeadAliases::default(),
                },
                Lead {
                    lead: "a".to_string(),
                    source: "legacy".to_string(),
                    synopsis: "Duplicate id.".to_string(),
                    aliases: LeadAliases::default(),
                },
            ],
        };
        let findings = doc.check_alias_collisions();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source, "legacy");
        assert_eq!(findings[0].name, "a");
        assert_eq!(findings[0].bearing_leads, vec!["a".to_string()]);
    }

    #[test]
    fn same_lead_across_sources_is_legal() {
        // Raw, unmerged leads: the same `lead` surfaced by two
        // different sources is two distinct blocks, not a collision.
        let doc = Discovery {
            prefix: String::new(),
            has_inventory_heading: true,
            suffix: String::new(),
            leads: vec![
                Lead {
                    lead: "user-registration".to_string(),
                    source: "legacy".to_string(),
                    synopsis: "From legacy.".to_string(),
                    aliases: LeadAliases::default(),
                },
                Lead {
                    lead: "user-registration".to_string(),
                    source: "runtime".to_string(),
                    synopsis: "From runtime.".to_string(),
                    aliases: LeadAliases::default(),
                },
            ],
        };
        assert!(
            doc.check_alias_collisions().is_empty(),
            "same lead under different source keys must not collide"
        );
    }

    #[test]
    fn check_alias_collisions_id_vs_alias() {
        let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [a]
- synopsis: B aliases a's id.
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

### legacy:a

- lead: a
- source: legacy
- aliases: [shared]
- synopsis: A.

### legacy:b

- lead: b
- source: legacy
- aliases: [shared]
- synopsis: B.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        let findings = doc.check_alias_collisions();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "shared");
        assert_eq!(findings[0].bearing_leads, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn alias_collisions_clean_doc() {
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
            reparsed.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
        assert!(lead.aliases.contains("pwd-reset"));
        assert!(lead.aliases.contains("password-reset"), "preserves existing aliases");
    }

    #[test]
    fn add_alias_refuses_collision() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        let err =
            doc.add_alias("password-reset-request", "user-registration").expect_err("collision");
        match err {
            Error::Validation { code, .. } => {
                assert_eq!(code, "discovery-alias-collision");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
        // Ensure the mutation rolled back so subsequent edits start
        // from the same state the operator saw on disk.
        let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
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
        let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
        assert!(lead.aliases.contains("password-reset"));
    }

    #[test]
    fn remove_alias_drops_named_entry() {
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        doc.remove_alias("password-reset-request", "password-reset").expect("removed");
        let lead = doc.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
        assert!(!lead.aliases.contains("password-reset"));
    }

    #[test]
    fn parses_block_without_aliases_bullet() {
        let yaml = "\
## Lead inventory

### legacy:a

- lead: a
- source: legacy
- synopsis: A.
";
        let doc = Discovery::parse(yaml).expect("parse ok");
        assert!(doc.leads[0].aliases.is_empty());
    }

    fn lead(lead: &str, source: &str, synopsis: &str) -> Lead {
        Lead {
            lead: lead.to_string(),
            source: source.to_string(),
            synopsis: synopsis.to_string(),
            aliases: LeadAliases::default(),
        }
    }

    #[test]
    fn merge_survey_replaces_same_id_block() {
        // Re-survey survival — re-running `survey` for a source replaces
        // its leads by canonical `id` in place; untouched leads survive.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("discovery.md");
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");

        let incoming =
            vec![lead("user-registration", "legacy", "Registration endpoint (re-surveyed).")];
        doc.merge_survey("legacy", incoming, &path).expect("merge ok");

        let reloaded = Discovery::load(&path).expect("reload ok");
        let hit = reloaded.leads.iter().find(|c| c.lead == "user-registration").expect("present");
        assert_eq!(hit.synopsis, "Registration endpoint (re-surveyed).");
        assert_eq!(
            reloaded.leads.iter().filter(|c| c.lead == "user-registration").count(),
            1,
            "replaced in place, not duplicated"
        );
        assert!(
            reloaded.leads.iter().any(|c| c.lead == "password-reset-request"),
            "leads absent from the incoming set survive untouched"
        );
    }

    #[test]
    fn merge_survey_preserves_operator_aliases() {
        // discovery alias contract §re-survey survival — operator-authored
        // aliases on a surviving id are unioned with the adapter's re-emit.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("discovery.md");
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        doc.add_alias("password-reset-request", "pwd-reset").expect("operator alias ok");

        // The re-survey re-emits the adapter alias `password-reset`; the
        // operator's `pwd-reset` must survive the union.
        let mut reset = lead("password-reset-request", "legacy", "Reset endpoint (re-surveyed).");
        reset.aliases = LeadAliases::from_iter(["password-reset"]);
        doc.merge_survey("legacy", vec![reset], &path).expect("merge ok");

        let reloaded = Discovery::load(&path).expect("reload ok");
        let hit =
            reloaded.leads.iter().find(|c| c.lead == "password-reset-request").expect("present");
        assert_eq!(hit.synopsis, "Reset endpoint (re-surveyed).");
        assert_eq!(
            hit.aliases.names,
            vec!["password-reset", "pwd-reset"],
            "operator + adapter aliases union without duplication"
        );
    }

    #[test]
    fn merge_survey_preserves_deterministic_ordering() {
        // Replaced leads keep their document slot; brand-new leads append
        // in survey order, so re-survey re-renders deterministically.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("discovery.md");
        let doc_md = "\
## Lead inventory

### legacy:x

- lead: x
- source: legacy
- synopsis: X.

### legacy:y

- lead: y
- source: legacy
- synopsis: Y.

### legacy:z

- lead: z
- source: legacy
- synopsis: Z.
";
        let mut doc = Discovery::parse(doc_md).expect("parse ok");

        let incoming =
            vec![lead("y", "legacy", "Y (re-surveyed)."), lead("w", "legacy", "W (new).")];
        doc.merge_survey("legacy", incoming, &path).expect("merge ok");

        let reloaded = Discovery::load(&path).expect("reload ok");
        let ids: Vec<&str> = reloaded.leads.iter().map(|c| c.lead.as_str()).collect();
        assert_eq!(ids, vec!["x", "y", "z", "w"]);
    }

    #[test]
    fn merge_survey_collision_fails_without_writing() {
        // A post-merge collision fails the whole merge: nothing lands on
        // disk and the in-memory model rolls back to its pre-merge state.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("discovery.md");
        let mut doc = Discovery::parse(SAMPLE).expect("parse ok");
        let before = doc.clone();

        // Incoming lead aliases another lead's canonical id → collision.
        let mut rogue = lead("new-lead", "legacy", "Rogue.");
        rogue.aliases = LeadAliases::from_iter(["user-registration"]);
        let err = doc.merge_survey("legacy", vec![rogue], &path).expect_err("collision");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "discovery-alias-collision"),
            other => panic!("expected Validation, got: {other:?}"),
        }

        assert!(!path.exists(), "failed merge must not write the file");
        assert_eq!(doc, before, "failed merge must roll the in-memory model back");
    }
}
