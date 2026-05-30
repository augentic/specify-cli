//! SHA-256 digest helpers shared across cache, fingerprint, and tool paths.

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

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{sha256_hex, sha256_output_hex};

    #[test]
    fn sha256_hex_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_output_hex_matches_digest() {
        let digest = Sha256::digest(b"specify");
        assert_eq!(sha256_output_hex(digest), sha256_hex(b"specify"));
    }
}
