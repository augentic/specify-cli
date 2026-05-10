//! Deterministic fingerprinting for generated context inputs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use specify_error::Error;

/// One renderer input file and its content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputFingerprint {
    /// Repo-relative path, using `/` separators.
    pub(super) path: String,
    /// Lowercase hex SHA-256 digest of the input bytes.
    pub(super) sha256: String,
}

/// Fingerprint values persisted in `.specify/context.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContextFingerprint {
    /// `sha256:<hex>` digest over the canonical aggregate input.
    pub(super) fingerprint: String,
    /// CLI version included as the first line of the aggregate input.
    pub(super) cli_version: String,
    /// Sorted per-file inputs.
    pub(super) inputs: Vec<InputFingerprint>,
    /// `sha256:<hex>` digest of the bytes between the context fences.
    pub(super) body_sha256: String,
}

/// Collects candidate paths and hashes their bytes in deterministic order.
#[derive(Debug, Clone)]
pub(super) struct InputCollector {
    project_dir: PathBuf,
    paths: BTreeMap<String, PathBuf>,
}

impl InputCollector {
    /// Start collecting inputs for a project root.
    pub(super) fn new(project_dir: &Path) -> Self {
        Self {
            project_dir: project_dir.to_path_buf(),
            paths: BTreeMap::new(),
        }
    }

    /// Add a required input file.
    pub(super) fn add_file(&mut self, path: &Path) -> Result<(), Error> {
        let relative = repo_relative_path(&self.project_dir, path)?;
        self.paths.entry(relative).or_insert_with(|| path.to_path_buf());
        Ok(())
    }

    /// Add an input file only when it exists as a regular file.
    pub(super) fn add_file_if_present(&mut self, path: &Path) -> Result<(), Error> {
        match path.try_exists() {
            Ok(true) if path.is_file() => self.add_file(path),
            Ok(_) => Ok(()),
            Err(err) => Err(Error::Io(err)),
        }
    }

    /// Add repo-relative input paths captured by another renderer component.
    pub(super) fn add_relative_files<I, S>(&mut self, paths: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for path in paths {
            self.add_file(&self.project_dir.join(path.as_ref()))?;
        }
        Ok(())
    }

    /// Read and hash every collected input in repo-relative path order.
    pub(super) fn finalize(&self) -> Result<Vec<InputFingerprint>, Error> {
        self.paths
            .iter()
            .map(|(relative, absolute)| {
                let bytes = fs::read(absolute).map_err(Error::Io)?;
                Ok(InputFingerprint {
                    path: relative.clone(),
                    sha256: sha256_hex(&bytes),
                })
            })
            .collect()
    }
}

/// Build the lock-ready fingerprint structure from input hashes and body bytes.
pub(super) fn context_fingerprint(
    cli_version: &str, inputs: Vec<InputFingerprint>, body: &[u8],
) -> ContextFingerprint {
    ContextFingerprint {
        fingerprint: aggregate_fingerprint(cli_version, inputs.clone()),
        cli_version: cli_version.to_string(),
        inputs,
        body_sha256: body_sha256(body),
    }
}

/// Hash the canonical aggregate encoding used by `.specify/context.lock`.
pub(super) fn aggregate_fingerprint(
    cli_version: &str, mut inputs: Vec<InputFingerprint>,
) -> String {
    inputs.sort_by(|left, right| left.path.cmp(&right.path));

    let mut canonical = String::new();
    canonical.push_str(cli_version);
    canonical.push('\n');
    for input in inputs {
        canonical.push_str(&input.path);
        canonical.push('\t');
        canonical.push_str(&input.sha256);
        canonical.push('\n');
    }

    prefixed_sha256(canonical.as_bytes())
}

/// Hash fenced body bytes with the `sha256:` prefix used by the lock file.
pub(super) fn body_sha256(body: &[u8]) -> String {
    prefixed_sha256(body)
}

fn prefixed_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn repo_relative_path(project_dir: &Path, path: &Path) -> Result<String, Error> {
    let relative = path.strip_prefix(project_dir).map_err(|_err| Error::Diag {
        code: "context-fingerprint-input-outside-project",
        detail: format!(
            "context fingerprint input {} is outside project root {}",
            path.display(),
            project_dir.display()
        ),
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str, sha256: &str) -> InputFingerprint {
        InputFingerprint {
            path: path.to_string(),
            sha256: sha256.to_string(),
        }
    }

    #[test]
    fn aggregate_hash_uses_sorted_canonical_inputs() {
        let inputs = vec![
            input(
                "registry.yaml",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
            input(
                ".specify/project.yaml",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ];

        let fingerprint = aggregate_fingerprint("0.2.0", inputs);

        assert_eq!(
            fingerprint,
            "sha256:96f096c433da7e43d6ab7ce7aa305882f3eb2933fa160d00640af8a0df17e73f"
        );
    }

    #[test]
    fn aggregate_hash_is_stable_when_collection_order_changes() {
        let alpha = input(
            ".specify/project.yaml",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let beta = input(
            "registry.yaml",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let gamma =
            input("Cargo.toml", "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");

        let left = aggregate_fingerprint("0.2.0", vec![alpha.clone(), beta.clone(), gamma.clone()]);
        let right = aggregate_fingerprint("0.2.0", vec![gamma, alpha, beta]);

        assert_eq!(left, right);
    }

    #[test]
    fn body_hash_changes_when_fenced_body_changes() {
        let original = body_sha256(b"\n## Runtime\n- detected: Rust.\n\n");
        let edited = body_sha256(b"\n## Runtime\n- detected: Node.js.\n\n");

        assert_ne!(original, edited);
    }
}
