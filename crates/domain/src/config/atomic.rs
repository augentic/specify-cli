//! Ergonomic load → mutate → atomic-write loop for `.specify/`-anchored
//! YAML state.
//!
//! Handlers throughout `src/commands/**` follow the same shape:
//!
//! ```text
//! load YAML → validate → mutate → atomic-write → emit Body
//! ```
//!
//! The [`AtomicYaml`] trait pairs each piece of state with its
//! canonical on-disk location ([`Layout`]); the [`with_state`] /
//! [`with_existing_state`] helpers wrap the load + atomic-write halves
//! so each handler keeps only the mutation closure and the response
//! shaping. Atomic writes go through [`crate::slice::atomic::yaml_write`]
//! — the only legitimate writer for these files.
//!
//! Scope: this helper is intended for binary handlers
//! (`src/commands/**`). Library code that mutates state should keep
//! using the inherent `load` / `save` / `path` helpers on the state
//! type directly.

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use specify_error::Error;

use crate::config::{Layout, ProjectConfig};
use crate::registry::Registry;
use crate::slice::atomic::yaml_write;

/// A piece of `.specify/`-anchored YAML state.
///
/// Implementors pair a canonical on-disk location with atomic-write
/// semantics by exposing a `load` / `path` pair so [`with_state`] (and
/// [`with_existing_state`]) can wrap the load → mutate → write loop.
pub trait AtomicYaml: Sized + Serialize + DeserializeOwned {
    /// Path under [`Layout`] where this state lives.
    fn path(layout: Layout<'_>) -> PathBuf;

    /// Default value used when the file does not yet exist. Return
    /// `None` to make absence an error in [`with_state`] (callers that
    /// require an existing file should prefer [`with_existing_state`]).
    #[must_use]
    fn default_for_load() -> Option<Self> {
        None
    }

    /// Load from disk. Default implementation reads [`Self::path`],
    /// deserialises, and returns `Ok(None)` when the file is absent.
    /// Override when the state needs validation at load time
    /// (the existing `Registry::load` runs `validate_shape` before
    /// returning).
    ///
    /// # Errors
    ///
    /// Propagates I/O failures and YAML parse errors.
    fn load(layout: Layout<'_>) -> Result<Option<Self>, Error> {
        let path = Self::path(layout);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let value: Self = serde_saphyr::from_str(&content)?;
        Ok(Some(value))
    }
}

/// Load → mutate → atomic-write loop.
///
/// Loads `S` (or synthesises it from [`AtomicYaml::default_for_load`]
/// if absent), runs `f` against the in-memory state, atomically writes
/// the mutated value back, then returns the body the closure produced.
///
/// `with_state` does **not** itself emit; the caller writes
/// `ctx.out().write(&body)?;`. This keeps response shaping local to
/// each handler and the helper focused on the IO loop.
///
/// # Panics
///
/// Panics if [`AtomicYaml::load`] returns `Ok(None)` and
/// [`AtomicYaml::default_for_load`] is also `None`. Handlers that
/// require an existing file should call [`with_existing_state`]
/// instead, which surfaces the absence as a typed error rather than a
/// panic.
///
/// # Errors
///
/// Propagates errors from `load`, the closure, and the atomic write.
pub fn with_state<S, B, F>(layout: Layout<'_>, f: F) -> Result<B, Error>
where
    S: AtomicYaml,
    F: FnOnce(&mut S) -> Result<B, Error>,
{
    let path = S::path(layout);
    let mut state = S::load(layout)?.unwrap_or_else(|| {
        S::default_for_load().expect(
            "AtomicYaml::load returned None but default_for_load() is None; \
             callers that require an existing file must use with_existing_state",
        )
    });
    let body = f(&mut state)?;
    yaml_write(&path, &state)?;
    Ok(body)
}

/// Like [`with_state`], but treats absence as an error.
///
/// Returns `Error::ArtifactNotFound { kind: missing_kind, path }` when
/// the underlying file is absent. Use this for handlers whose contract
/// is "operate on an existing file" (plan amend / transition,
/// registry remove pre-flight, …).
///
/// # Errors
///
/// Returns `Error::ArtifactNotFound` when [`AtomicYaml::load`] yields
/// `Ok(None)`; otherwise propagates errors from `load`, the closure,
/// and the atomic write.
pub fn with_existing_state<S, B, F>(
    layout: Layout<'_>, missing_kind: &'static str, f: F,
) -> Result<B, Error>
where
    S: AtomicYaml,
    F: FnOnce(&mut S) -> Result<B, Error>,
{
    let path = S::path(layout);
    let mut state = S::load(layout)?.ok_or_else(|| Error::ArtifactNotFound {
        kind: missing_kind,
        path: path.clone(),
    })?;
    let body = f(&mut state)?;
    yaml_write(&path, &state)?;
    Ok(body)
}

impl AtomicYaml for Registry {
    fn path(layout: Layout<'_>) -> PathBuf {
        layout.registry_path()
    }

    /// `add` may create `registry.yaml` from scratch. Mirrors the
    /// `Registry { version: 1, projects: Vec::new() }` fall-through
    /// the legacy `registry::add` handler used.
    fn default_for_load() -> Option<Self> {
        Some(Self {
            version: 1,
            projects: Vec::new(),
        })
    }

    /// Delegate to the inherent loader so `validate_shape` runs at
    /// load time — the trait's default impl would skip that. The
    /// explicit `Registry::` prefix (rather than `Self::`) selects the
    /// inherent associated function; `Self::load` would resolve to
    /// the trait method we are currently defining and recurse.
    #[expect(
        clippy::use_self,
        reason = "explicit type prefix disambiguates the inherent `Registry::load` from this trait method of the same name"
    )]
    fn load(layout: Layout<'_>) -> Result<Option<Self>, Error> {
        Registry::load(layout.project_dir())
    }
}

impl AtomicYaml for ProjectConfig {
    fn path(layout: Layout<'_>) -> PathBuf {
        layout.config_path()
    }

    /// `project.yaml` is created by `specify init`, never synthesised
    /// implicitly.
    fn default_for_load() -> Option<Self> {
        None
    }

    /// Delegate to the inherent loader so the `specify_version` floor
    /// check runs at load time. Map the canonical "absent" error
    /// ([`Error::NotInitialized`]) to `Ok(None)` so the trait's
    /// "absent → None" contract holds; callers that need the typed
    /// error should call [`ProjectConfig::load`] directly or use
    /// [`with_existing_state`] with a custom `missing_kind`.
    #[expect(
        clippy::use_self,
        reason = "explicit type prefix disambiguates the inherent `ProjectConfig::load` from this trait method of the same name"
    )]
    fn load(layout: Layout<'_>) -> Result<Option<Self>, Error> {
        match ProjectConfig::load(layout.project_dir()) {
            Ok(cfg) => Ok(Some(cfg)),
            Err(Error::NotInitialized) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::config::LayoutExt;

    #[test]
    fn with_state_creates_default_when_absent() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let body = with_state::<Registry, _, _>(layout, |reg| {
            assert_eq!(reg.version, 1);
            assert!(reg.projects.is_empty());
            Ok(reg.version)
        })
        .expect("with_state ok");
        assert_eq!(body, 1);
        assert!(layout.registry_path().exists(), "registry.yaml should be created");
    }

    #[test]
    fn with_state_propagates_closure_error_and_skips_write() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let err = with_state::<Registry, (), _>(layout, |_| {
            Err(Error::Diag {
                code: "test-abort",
                detail: "abort".into(),
            })
        })
        .expect_err("closure error must propagate");
        assert!(matches!(
            err,
            Error::Diag {
                code: "test-abort",
                ..
            }
        ));
        assert!(
            !layout.registry_path().exists(),
            "registry.yaml must not be written when the closure errs"
        );
    }

    #[test]
    fn with_existing_state_errors_on_absence() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let err = with_existing_state::<Registry, (), _>(layout, "registry.yaml", |_| Ok(()))
            .expect_err("absent file must error");
        match err {
            Error::ArtifactNotFound { kind, path } => {
                assert_eq!(kind, "registry.yaml");
                assert_eq!(path, layout.registry_path());
            }
            other => panic!("expected ArtifactNotFound, got {other:?}"),
        }
    }

    #[test]
    fn with_existing_state_round_trips_mutation() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let initial = Registry {
            version: 1,
            projects: Vec::new(),
        };
        yaml_write(&layout.registry_path(), &initial).expect("seed registry.yaml");

        with_existing_state::<Registry, (), _>(layout, "registry.yaml", |reg| {
            reg.projects.push(crate::registry::RegistryProject {
                name: "alpha".into(),
                url: ".".into(),
                capability: "omnia@v1".into(),
                description: None,
                contracts: None,
            });
            Ok(())
        })
        .expect("mutate ok");

        let reloaded = Registry::load(tmp.path()).expect("load").expect("present");
        assert_eq!(reloaded.projects.len(), 1);
        assert_eq!(reloaded.projects[0].name, "alpha");
    }

    #[test]
    fn project_config_load_maps_not_initialized_to_none() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let loaded = <ProjectConfig as AtomicYaml>::load(layout).expect("load ok");
        assert!(loaded.is_none(), "absent project.yaml must surface as None");
    }

    #[test]
    fn project_config_load_round_trips_when_present() {
        let tmp = tempdir().expect("tempdir");
        let layout = tmp.path().layout();
        let cfg = ProjectConfig {
            name: "demo".into(),
            domain: None,
            capability: Some("omnia".into()),
            specify_version: None,
            rules: BTreeMap::new(),
            tools: Vec::new(),
            hub: false,
        };
        fs::create_dir_all(layout.specify_dir()).expect("create .specify");
        yaml_write(&layout.config_path(), &cfg).expect("seed project.yaml");
        let loaded =
            <ProjectConfig as AtomicYaml>::load(layout).expect("load ok").expect("present");
        assert_eq!(loaded.name, "demo");
    }
}
