//! `specify discovery show` — read-only window into
//! `<project_dir>/discovery.md` (RFC-27 §D6).
//!
//! Default output mirrors the inventory's block grammar. `--aliases`
//! switches to the alias-map view used by operators auditing a
//! cross-source merge before authoring `--sources <key>=<alias>`.

use std::io::Write;

use serde::Serialize;
use specify_domain::discovery::Discovery;
use specify_error::Result;

use crate::context::Ctx;

pub(super) fn show(ctx: &Ctx, aliases: bool) -> Result<()> {
    let discovery_path = ctx.layout().discovery_path();
    let discovery = Discovery::load(&discovery_path)?;

    if aliases {
        let body = build_alias_body(&discovery);
        ctx.write(&body, write_alias_text)?;
    } else {
        let body = build_inventory_body(&discovery);
        ctx.write(&body, write_inventory_text)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct InventoryBody {
    candidates: Vec<InventoryCandidate>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct InventoryCandidate {
    id: String,
    sources: Vec<String>,
    summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tentative: Option<bool>,
}

fn build_inventory_body(discovery: &Discovery) -> InventoryBody {
    InventoryBody {
        candidates: discovery
            .candidates()
            .iter()
            .map(|c| InventoryCandidate {
                id: c.id.clone(),
                sources: c.sources.clone(),
                summary: c.summary.clone(),
                aliases: c.aliases.names.clone(),
                tentative: c.tentative,
            })
            .collect(),
    }
}

fn write_inventory_text(w: &mut dyn Write, body: &InventoryBody) -> std::io::Result<()> {
    if body.candidates.is_empty() {
        writeln!(w, "(no candidates in discovery.md)")?;
        return Ok(());
    }
    for (idx, candidate) in body.candidates.iter().enumerate() {
        if idx > 0 {
            writeln!(w)?;
        }
        writeln!(w, "{}", candidate.id)?;
        writeln!(w, "  sources: [{}]", candidate.sources.join(", "))?;
        if !candidate.aliases.is_empty() {
            writeln!(w, "  aliases: [{}]", candidate.aliases.join(", "))?;
        }
        if candidate.tentative == Some(true) {
            writeln!(w, "  tentative: true")?;
        }
        writeln!(w, "  summary: {}", candidate.summary)?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AliasBody {
    aliases: Vec<AliasRow>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct AliasRow {
    id: String,
    aliases: Vec<String>,
}

fn build_alias_body(discovery: &Discovery) -> AliasBody {
    let mut rows: Vec<AliasRow> = discovery
        .candidates()
        .iter()
        .filter(|c| !c.aliases.is_empty())
        .map(|c| AliasRow {
            id: c.id.clone(),
            aliases: c.aliases.names.clone(),
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    AliasBody { aliases: rows }
}

fn write_alias_text(w: &mut dyn Write, body: &AliasBody) -> std::io::Result<()> {
    if body.aliases.is_empty() {
        writeln!(w, "(no aliases declared in discovery.md)")?;
        return Ok(());
    }
    for row in &body.aliases {
        writeln!(w, "{} -> [{}]", row.id, row.aliases.join(", "))?;
    }
    Ok(())
}
