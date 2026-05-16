//! DTOs for `surfaces.json` and `metadata.json`. Field order and serde
//! renames match the JSON schemas in `schemas/` verbatim.

use serde::{Deserialize, Serialize};

/// Envelope for `surfaces.json` — one file per legacy-code source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SurfacesDocument {
    /// Schema version; must be `1`.
    pub version: u8,
    /// Kebab-case identifier for the legacy source.
    pub source_key: String,
    /// Primary programming language of the source.
    pub language: String,
    /// Externally observable surfaces, sorted by `id`.
    pub surfaces: Vec<Surface>,
}

/// A single externally observable surface entry inside `surfaces.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Surface {
    /// Stable identifier unique within the file.
    pub id: String,
    /// Closed surface kind.
    pub kind: SurfaceKind,
    /// Legacy spelling of the observable surface.
    pub identifier: String,
    /// Handler or call-site reference.
    pub handler: String,
    /// Source files reached from the handler, sorted alphabetically.
    pub touches: Vec<String>,
    /// Declaration sites, sorted alphabetically. Non-empty.
    pub declared_at: Vec<String>,
}

/// Closed enum of surface kinds. Extensions require an RFC update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    /// HTTP route (GET, POST, PUT, DELETE, …).
    HttpRoute,
    /// Message publication (topic producer).
    MessagePub,
    /// Message subscription (topic consumer).
    MessageSub,
    /// `WebSocket` handler.
    WsHandler,
    /// Scheduled/cron job.
    ScheduledJob,
    /// CLI command entry point.
    CliCommand,
    /// UI route (frontend page or screen).
    UiRoute,
    /// Outbound service call.
    ExternalCallOut,
}

/// Envelope for `metadata.json` — coarse source facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MetadataDocument {
    /// Schema version; must be `1`.
    pub version: u8,
    /// Kebab-case identifier for the legacy source.
    pub source_key: String,
    /// Primary programming language of the source.
    pub language: String,
    /// Production lines of code.
    pub loc: u64,
    /// Number of modules in the source.
    pub module_count: u64,
    /// Names of top-level modules.
    pub top_level_modules: Vec<String>,
}
