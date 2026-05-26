//! SHA-256 digest helpers shared across resolver and cache paths.

use sha2::{Digest, Sha256};

/// Lowercase hex encoding of a SHA-256 digest over `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256_output_hex(Sha256::digest(bytes))
}

/// Lowercase hex encoding of a finalized SHA-256 output.
#[must_use]
pub fn sha256_output_hex(digest: impl AsRef<[u8]>) -> String {
    base16ct::lower::encode_string(digest.as_ref())
}
