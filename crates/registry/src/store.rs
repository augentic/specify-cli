//! Global content-addressed adapter store (RFC-48 D5).
//!
//! Adapters resolve from a single global store keyed by the immutable
//! `(name, version)` identity — the Cargo `~/.cargo/registry` model.
//! [`install`] pulls the published artifact once, verifies it against the
//! recorded digest (the verify-on-read gate, D4), unpacks it into a temp
//! dir under the store root, makes the tree read-only, and renames it
//! into place atomically. A file lock around the verify-rename window
//! makes concurrent installs of one identity idempotent. The store path
//! resolver itself lives on the `specify-schema` leaf
//! ([`specify_schema::cache::adapter_store_entry`]) so both this install
//! path and the offline read path agree on one location.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use specify_schema::cache::adapter_store_entry;

use crate::error::ExtensionError;
use crate::oci::{self, RegistryAuth};
use crate::pack;

/// Resolve the global store entry for the pinned `(name, version)`,
/// installing it from the immutable `reference` if absent. Returns the
/// read-only store entry path.
///
/// Idempotent: store entries are immutable, so an already-present entry
/// is returned without a re-pull (presence implies a verified install).
///
/// # Errors
///
/// Propagates `adapter-transport-failed` (pull), `adapter-digest-mismatch`
/// (verify-on-read), and `adapter-pack-failed` (unpack / store I/O).
pub fn install(
    name: &str, version: &str, reference: &str, recorded_digest: &str, auth: &RegistryAuth,
) -> Result<PathBuf, ExtensionError> {
    let entry = adapter_store_entry(name, version);
    if entry.is_dir() {
        return Ok(entry);
    }
    let layer = oci::pull_adapter(reference, auth)?;
    pack::verify_digest(reference, &layer, recorded_digest)?;
    install_layer(&entry, &layer)?;
    Ok(entry)
}

/// Materialize a verified `layer` at the immutable store `entry` with
/// atomic, idempotent, read-only semantics. Exposed within the crate so
/// the store layout is exercised without a live registry.
///
/// # Errors
///
/// Returns `adapter-pack-failed` when the store root cannot be created,
/// the lock cannot be taken, the layer cannot be unpacked, or the
/// temp-to-entry rename fails.
pub(crate) fn install_layer(entry: &Path, layer: &[u8]) -> Result<(), ExtensionError> {
    let root = entry
        .parent()
        .ok_or_else(|| ExtensionError::pack(format!("store entry {} has no parent", entry.display())))?;
    let key = entry_key(entry)?;
    fs::create_dir_all(root)
        .map_err(|err| ExtensionError::pack(format!("create store root {}: {err}", root.display())))?;

    // Serialize concurrent installers of this identity behind a sibling
    // lock file. The lock is advisory; the post-lock re-check is the
    // authority, so a peer that won the race makes this call a no-op.
    let lock_path = root.join(format!(".{key}.lock"));
    let lock = File::create(&lock_path)
        .map_err(|err| ExtensionError::pack(format!("create store lock {}: {err}", lock_path.display())))?;
    lock.lock()
        .map_err(|err| ExtensionError::pack(format!("lock store entry {}: {err}", lock_path.display())))?;

    if entry.is_dir() {
        return Ok(());
    }

    let temp = root.join(format!(".{key}.tmp.{}", std::process::id()));
    if temp.exists() {
        fs::remove_dir_all(&temp)
            .map_err(|err| ExtensionError::pack(format!("clear stale temp {}: {err}", temp.display())))?;
    }
    pack::unpack_adapter(layer, &temp)?;
    set_tree_read_only(&temp)?;
    // Atomic publish: a reader either sees the absent entry or the fully
    // materialized one, never a half-written tree.
    fs::rename(&temp, entry)
        .map_err(|err| ExtensionError::pack(format!("publish store entry {}: {err}", entry.display())))?;
    Ok(())
}

/// The immutable `name@version` final component used to derive sibling
/// lock and temp paths.
fn entry_key(entry: &Path) -> Result<String, ExtensionError> {
    entry
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ExtensionError::pack(format!("store entry {} has no name", entry.display())))
}

/// Recursively mark every file in the installed tree read-only so the
/// content-addressed store entry cannot be mutated in place.
fn set_tree_read_only(dir: &Path) -> Result<(), ExtensionError> {
    for entry in fs::read_dir(dir)
        .map_err(|err| ExtensionError::pack(format!("read store temp {}: {err}", dir.display())))?
    {
        let entry = entry
            .map_err(|err| ExtensionError::pack(format!("read store temp entry: {err}")))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| ExtensionError::pack(format!("stat {}: {err}", path.display())))?;
        if meta.is_dir() {
            set_tree_read_only(&path)?;
        } else {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            fs::set_permissions(&path, perms)
                .map_err(|err| ExtensionError::pack(format!("chmod {}: {err}", path.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
