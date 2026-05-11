//! Digest computation and verification for resolver-acquired bytes.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::AcquiredBytes;
use crate::error::ToolError;

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub(super) fn cached_matches(module: &Path, expected: Option<&str>) -> Result<bool, ToolError> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    let bytes = fs::read(module).map_err(|err| {
        ToolError::cache_io("read cached module for sha256 verification", module, err)
    })?;
    Ok(sha256_hex(&bytes) == expected)
}

pub(super) fn validate(
    source: &str, acquired: &AcquiredBytes, expected_sha256: Option<&str>,
) -> Result<(), ToolError> {
    if acquired.len()? == 0 {
        return Err(ToolError::EmptySource {
            source_value: source.to_string(),
        });
    }

    if let Some(expected) = expected_sha256 {
        let actual = acquired.sha256_hex();
        if actual != expected {
            return Err(ToolError::DigestMismatch {
                source_value: source.to_string(),
                expected: expected.to_string(),
                actual,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use std::path::Path;

    use super::super::resolve;
    use super::*;
    use crate::error::ToolError;
    use crate::manifest::ToolSource;
    use crate::test_support::{
        cached_bytes, fixed_now, project_scope, scratch_dir, tool, with_cache_env, write_source,
    };

    fn corrupt_cached_module(path: &Path, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .expect("open cached module for corruption");
        file.write_all(bytes).expect("overwrite cached module");
    }

    #[test]
    fn digest_mismatch_fails_before_install_and_preserves_previous_cache() {
        let cache_dir = scratch_dir("resolver-digest-cache");
        let source_dir = scratch_dir("resolver-digest-source");
        let old_source = write_source(&source_dir, "old.wasm", b"old-good");
        let new_source = write_source(&source_dir, "new.wasm", b"new-good");
        let old_sha = sha256_hex(b"old-good");
        let wrong_sha =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        let scope = project_scope();
        let old_tool = tool(ToolSource::LocalPath(old_source), Some(old_sha));
        let wrong_tool = tool(ToolSource::LocalPath(new_source.clone()), Some(wrong_sha));

        with_cache_env(Some(&cache_dir), None, None, || {
            resolve(&scope, &old_tool, fixed_now()).expect("initial digest install");
            let err =
                resolve(&scope, &wrong_tool, fixed_now()).expect_err("wrong digest must fail");
            assert!(matches!(err, ToolError::DigestMismatch { .. }), "{err}");
            assert_eq!(cached_bytes(&scope, &old_tool), b"old-good");

            let correct_tool =
                tool(ToolSource::LocalPath(new_source), Some(sha256_hex(b"new-good")));
            resolve(&scope, &correct_tool, fixed_now()).expect("correct digest updates cache");
            assert_eq!(cached_bytes(&scope, &correct_tool), b"new-good");
        });
    }

    #[test]
    fn cached_bytes_are_rehashed_on_digest_pinned_hit() {
        let cache_dir = scratch_dir("resolver-hit-digest-cache");
        let source_dir = scratch_dir("resolver-hit-digest-source");
        let source = write_source(&source_dir, "module.wasm", b"trusted");
        let scope = project_scope();
        let pinned = tool(ToolSource::LocalPath(source), Some(sha256_hex(b"trusted")));

        with_cache_env(Some(&cache_dir), None, None, || {
            let resolved = resolve(&scope, &pinned, fixed_now()).expect("install pinned");
            corrupt_cached_module(&resolved.bytes_path, b"corrupt");
            let repaired =
                resolve(&scope, &pinned, fixed_now()).expect("digest mismatch re-fetches");
            assert_eq!(repaired.bytes_path, resolved.bytes_path);
            assert_eq!(std::fs::read(repaired.bytes_path).expect("repaired bytes"), b"trusted");
        });
    }
}
