//! YAML sidecar for `specify context` drift checks.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};
use specify_error::{Error, ValidationStatus, ValidationSummary};
use specify_slice::atomic::atomic_yaml_write;

use super::fingerprint::ContextFingerprint;

const CURRENT_LOCK_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContextLock {
    pub(super) version: u64,
    pub(super) fingerprint: String,
    pub(super) cli_version: String,
    pub(super) inputs: Vec<LockInput>,
    pub(super) fences: LockFences,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LockInput {
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LockFences {
    pub(super) body_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputDiff {
    pub(super) changed: Vec<String>,
    pub(super) added: Vec<String>,
    pub(super) removed: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LockVersion {
    version: u64,
}

impl ContextLock {
    pub(super) fn from_fingerprint(fingerprint: &ContextFingerprint) -> Self {
        Self {
            version: CURRENT_LOCK_VERSION,
            fingerprint: fingerprint.fingerprint.clone(),
            cli_version: fingerprint.cli_version.clone(),
            inputs: fingerprint
                .inputs
                .iter()
                .map(|input| LockInput {
                    path: input.path.clone(),
                    sha256: input.sha256.clone(),
                })
                .collect(),
            fences: LockFences {
                body_sha256: fingerprint.body_sha256.clone(),
            },
        }
    }
}

pub(super) fn load(path: &Path) -> Result<Option<ContextLock>, Error> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(err)),
    };

    let version: LockVersion = serde_saphyr::from_str(&contents).map_err(|err| {
        validation_error(
            "context-lock-malformed",
            Error::ContextLockMalformed {
                detail: format!("failed to read lock version: {err}"),
            }
            .to_string(),
        )
    })?;
    if version.version > CURRENT_LOCK_VERSION {
        return Err(validation_error(
            "context-lock-version-too-new",
            Error::ContextLockTooNew {
                found: version.version,
                supported: CURRENT_LOCK_VERSION,
            }
            .to_string(),
        ));
    }
    if version.version != CURRENT_LOCK_VERSION {
        return Err(validation_error(
            "context-lock-malformed",
            Error::ContextLockMalformed {
                detail: format!(
                    "unsupported lock version {}; expected {}",
                    version.version, CURRENT_LOCK_VERSION
                ),
            }
            .to_string(),
        ));
    }

    let lock: ContextLock = serde_saphyr::from_str(&contents).map_err(|err| {
        validation_error(
            "context-lock-malformed",
            Error::ContextLockMalformed {
                detail: err.to_string(),
            }
            .to_string(),
        )
    })?;
    Ok(Some(lock))
}

pub(super) fn save(path: &Path, lock: &ContextLock) -> Result<(), Error> {
    atomic_yaml_write(path, lock)
}

pub(super) fn diff_inputs(expected: &[LockInput], actual: &[LockInput]) -> InputDiff {
    let expected_by_path = inputs_by_path(expected);
    let actual_by_path = inputs_by_path(actual);

    let changed = expected_by_path
        .iter()
        .filter_map(|(path, expected_sha)| {
            actual_by_path
                .get(path)
                .filter(|actual_sha| *actual_sha != expected_sha)
                .map(|_actual_sha| path.clone())
        })
        .collect();
    let added = actual_by_path
        .keys()
        .filter(|path| !expected_by_path.contains_key(*path))
        .cloned()
        .collect();
    let removed = expected_by_path
        .keys()
        .filter(|path| !actual_by_path.contains_key(*path))
        .cloned()
        .collect();

    InputDiff {
        changed,
        added,
        removed,
    }
}

fn inputs_by_path(inputs: &[LockInput]) -> BTreeMap<String, String> {
    inputs.iter().map(|input| (input.path.clone(), input.sha256.clone())).collect()
}

fn validation_error(rule_id: &'static str, detail: String) -> Error {
    Error::Validation {
        results: vec![ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: rule_id.to_string(),
            rule: "context.lock must be a supported context lock file".to_string(),
            detail: Some(detail),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str, sha256: &str) -> LockInput {
        LockInput {
            path: path.to_string(),
            sha256: sha256.to_string(),
        }
    }

    #[test]
    fn input_diff_sorts_changed_added_and_removed_by_path() {
        let expected = vec![input("a.txt", "old"), input("b.txt", "same"), input("d.txt", "gone")];
        let actual = vec![input("a.txt", "new"), input("b.txt", "same"), input("c.txt", "added")];

        let diff = diff_inputs(&expected, &actual);

        assert_eq!(diff.changed, vec!["a.txt"]);
        assert_eq!(diff.added, vec!["c.txt"]);
        assert_eq!(diff.removed, vec!["d.txt"]);
    }

    #[test]
    fn lock_serializes_with_rm02_snake_case_keys() {
        let lock = ContextLock {
            version: 1,
            fingerprint: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            cli_version: "0.2.0".to_string(),
            inputs: vec![input(
                ".specify/project.yaml",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )],
            fences: LockFences {
                body_sha256:
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                        .to_string(),
            },
        };

        let yaml = serde_saphyr::to_string(&lock).expect("serialize lock");

        assert!(yaml.contains("cli_version: 0.2.0"), "{yaml}");
        assert!(yaml.contains("body_sha256: sha256:cccc"), "{yaml}");
        assert!(!yaml.contains("cli-version"), "{yaml}");
        assert!(!yaml.contains("body-sha256"), "{yaml}");
    }
}
