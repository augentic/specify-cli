//! Codex rule schema drift guard.
//!
//! RFC-28 §"Relationship to framework authoring" requires the vendored
//! runtime copy of the codex rule schema (consumed by the resolver) to stay
//! byte-for-byte identical to the authoring source distributed with
//! `specdev`. Drift would let the resolver and the authoring checks accept
//! divergent shapes, so this predicate computes SHA-256 over both files
//! and emits a finding whenever they disagree. The only sanctioned way to
//! resync is `scripts/sync-codex-schema.sh` from the `specify-cli`
//! workspace root — never hand-edit the vendored copy.
//!
//! Path-existence cases (resolved relative to `Context::framework_root`):
//!
//! 1. Neither file exists — no-op (e.g. plugin repo, where the codex-rule
//!    schema lives only in `specify-cli`).
//! 2. Authoring exists, vendored does not — no-op (Phase 2 setup state
//!    before CH-08 ships the vendored copy; not authoring drift).
//! 3. Vendored exists, authoring does not — emit a finding (vendored copy
//!    without an authoring source-of-truth means the project layout is
//!    wrong).
//! 4. Both exist — compute SHA-256 of each and emit a finding when they
//!    differ.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::context::Context;
use crate::finding::{Check, Finding, Location};

/// Rule id for the schema drift predicate.
pub const RULE_SCHEMA_DRIFT: &str = "codex.schema-drift";

const AUTHORING_REL: &str = "crates/authoring/schemas/codex-rule.schema.json";
const VENDORED_REL: &str = "schemas/codex/codex-rule.schema.json";
const SYNC_HINT: &str =
    "Regenerate via scripts/sync-codex-schema.sh from the specify-cli workspace.";

/// Codex rule schema drift check (authoring vs vendored runtime copy).
pub struct CodexSchemaDriftCheck;

impl Check for CodexSchemaDriftCheck {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
        run(ctx)
    }
}

/// Run the schema drift check against `ctx`.
pub fn run(ctx: &Context) -> Vec<Finding> {
    let root = ctx.framework_root();
    let authoring = root.join(AUTHORING_REL);
    let vendored = root.join(VENDORED_REL);

    let authoring_exists = authoring.is_file();
    let vendored_exists = vendored.is_file();

    match (authoring_exists, vendored_exists) {
        (false, false) | (true, false) => Vec::new(),
        (false, true) => vec![finding(
            format!(
                "Codex schema drift: vendored runtime copy {VENDORED_REL} is present but authoring source-of-truth {AUTHORING_REL} is missing. {SYNC_HINT}"
            ),
            vendored,
        )],
        (true, true) => compare(&authoring, &vendored),
    }
}

fn compare(authoring: &Path, vendored: &Path) -> Vec<Finding> {
    let authoring_bytes = match fs::read(authoring) {
        Ok(bytes) => bytes,
        Err(source) => {
            return vec![finding(
                format!(
                    "Codex schema drift: cannot read authoring schema {AUTHORING_REL}: {source}"
                ),
                authoring.to_path_buf(),
            )];
        }
    };
    let vendored_bytes = match fs::read(vendored) {
        Ok(bytes) => bytes,
        Err(source) => {
            return vec![finding(
                format!(
                    "Codex schema drift: cannot read vendored runtime schema {VENDORED_REL}: {source}"
                ),
                vendored.to_path_buf(),
            )];
        }
    };

    if authoring_bytes == vendored_bytes {
        return Vec::new();
    }

    let authoring_sha = sha256_hex(&authoring_bytes);
    let vendored_sha = sha256_hex(&vendored_bytes);
    vec![finding(
        format!(
            "Codex schema drift: authoring ({authoring_sha}) and vendored runtime ({vendored_sha}) diverged. {SYNC_HINT}"
        ),
        vendored.to_path_buf(),
    )]
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    Sha256::digest(bytes)
        .iter()
        .copied()
        .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
        .map(char::from)
        .collect()
}

fn finding(message: String, path: PathBuf) -> Finding {
    Finding {
        rule_id: RULE_SCHEMA_DRIFT,
        message,
        location: Some(Location {
            path,
            line: 1,
            column: None,
        }),
    }
}
