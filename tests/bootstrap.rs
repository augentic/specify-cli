//! Integration tests for the RFC-30 bootstrap lifecycle verbs
//! (`migrate`, `upgrade`, `plugins`). Shared helpers live in [`common`].

mod common;

#[path = "bootstrap/migrate.rs"]
mod migrate;

#[path = "bootstrap/upgrade.rs"]
mod upgrade;

#[path = "bootstrap/plugins.rs"]
mod plugins;
