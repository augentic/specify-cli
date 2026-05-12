//! Ergonomic load → mutate → atomic-write loop for `.specify/` YAML
//! state. [`AtomicYaml`] pairs state with its [`Layout`] location;
//! [`with_state`] wraps the load and atomic-write halves.

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
/// semantics by exposing a `load` / `path` pair so [`with_state`] can
/// wrap the load → mutate → write loop.
pub trait AtomicYaml: Sized + Serialize + DeserializeOwned {
    /// Path under [`Layout`] where this state lives.
    fn path(layout: Layout<'_>) -> PathBuf;

    /// Default value used when the file does not yet exist and the
    /// caller passes [`InitPolicy::CreateMissing`]. Return `None` to
    /// signal that the state has no synthesizable default; callers
    /// that allow creation must override.
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

/// How [`with_state`] reacts when the underlying file is absent.
#[derive(Debug, Clone, Copy)]
pub enum InitPolicy {
    /// Synthesise the state from [`AtomicYaml::default_for_load`] when
    /// the file is missing. Used by handlers whose contract is
    /// "create or update" (e.g. `registry add`).
    CreateMissing,
    /// Surface absence as [`Error::ArtifactNotFound`] with the given
    /// `kind`. Used by handlers whose contract is "operate on an
    /// existing file" (plan amend / transition, registry remove
    /// pre-flight, …).
    RequireExisting(&'static str),
}

/// Load → mutate → atomic-write loop.
///
/// Loads `S` from disk, applying `policy` when the file is absent:
/// [`InitPolicy::CreateMissing`] synthesises a default via
/// [`AtomicYaml::default_for_load`]; [`InitPolicy::RequireExisting`]
/// returns [`Error::ArtifactNotFound`]. Runs `f` against the in-memory
/// state, atomically writes the mutated value back, then returns the
/// body the closure produced.
///
/// `with_state` does **not** itself emit; the caller writes
/// `ctx.out().write(&body)?;`. This keeps response shaping local to
/// each handler and the helper focused on the IO loop.
///
/// # Panics
///
/// Panics if `policy` is [`InitPolicy::CreateMissing`], the file is
/// absent, and [`AtomicYaml::default_for_load`] is also `None`.
/// Implementors must provide a default when they advertise creation.
///
/// # Errors
///
/// - Returns [`Error::ArtifactNotFound`] when `policy` is
///   [`InitPolicy::RequireExisting`] and the file is absent.
/// - Otherwise propagates errors from `load`, the closure, and the
///   atomic write.
pub fn with_state<S, B, F>(layout: Layout<'_>, policy: InitPolicy, f: F) -> Result<B, Error>
where
    S: AtomicYaml,
    F: FnOnce(&mut S) -> Result<B, Error>,
{
    let path = S::path(layout);
    let mut state = match (S::load(layout)?, policy) {
        (Some(state), _) => state,
        (None, InitPolicy::CreateMissing) => S::default_for_load().expect(
            "AtomicYaml::load returned None and InitPolicy::CreateMissing was requested but \
             default_for_load() is None; the impl must provide a default when it allows creation",
        ),
        (None, InitPolicy::RequireExisting(kind)) => {
            return Err(Error::ArtifactNotFound { kind, path });
        }
    };
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
    /// [`with_state`] with [`InitPolicy::RequireExisting`] and a custom
    /// `missing_kind`.
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
    use crate::config::Layout;

    #[test]
    fn with_state_creates_default_when_absent() {
        let tmp = tempdir().expect("tempdir");
        let layout = Layout::new(tmp.path());
        let body = with_state::<Registry, _, _>(layout, InitPolicy::CreateMissing, |reg| {
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
        let layout = Layout::new(tmp.path());
        let err = with_state::<Registry, (), _>(layout, InitPolicy::CreateMissing, |_| {
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
    fn with_state_require_existing_errors_on_absence() {
        let tmp = tempdir().expect("tempdir");
        let layout = Layout::new(tmp.path());
        let err = with_state::<Registry, (), _>(
            layout,
            InitPolicy::RequireExisting("registry.yaml"),
            |_| Ok(()),
        )
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
    fn with_state_require_existing_round_trips_mutation() {
        let tmp = tempdir().expect("tempdir");
        let layout = Layout::new(tmp.path());
        let initial = Registry {
            version: 1,
            projects: Vec::new(),
        };
        yaml_write(&layout.registry_path(), &initial).expect("seed registry.yaml");

        with_state::<Registry, (), _>(
            layout,
            InitPolicy::RequireExisting("registry.yaml"),
            |reg| {
                reg.projects.push(crate::registry::RegistryProject {
                    name: "alpha".into(),
                    url: ".".into(),
                    capability: "omnia@v1".into(),
                    description: None,
                    contracts: None,
                });
                Ok(())
            },
        )
        .expect("mutate ok");

        let reloaded = Registry::load(tmp.path()).expect("load").expect("present");
        assert_eq!(reloaded.projects.len(), 1);
        assert_eq!(reloaded.projects[0].name, "alpha");
    }

    #[test]
    fn project_config_load_maps_not_initialized_to_none() {
        let tmp = tempdir().expect("tempdir");
        let layout = Layout::new(tmp.path());
        let loaded = <ProjectConfig as AtomicYaml>::load(layout).expect("load ok");
        assert!(loaded.is_none(), "absent project.yaml must surface as None");
    }

    #[test]
    fn project_config_load_round_trips_when_present() {
        let tmp = tempdir().expect("tempdir");
        let layout = Layout::new(tmp.path());
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
