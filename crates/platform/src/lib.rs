//! Multi-repo coordination scaffolding (RFC-3, stub-level in Phase 1).
//!
//! Phase 1 only freezes the public surface: the `PeerRepo` wire shape, the
//! `PlatformConfig` trait that consumer configs implement, and the
//! `parse_platform_config` entry point. Every call returns `vec![]` until
//! RFC-3 defines the concrete config fields.
//!
//! See `DECISIONS.md` ("Change H — Platform stub layering") for why the
//! config trait lives here rather than taking a direct `ProjectConfig`
//! dependency.

use serde::{Deserialize, Serialize};

/// A single peer repository entry in a platform registry.
///
/// Field names serialise as `kebab-case` so the YAML shape
/// (`specs-path: …`) matches the project-config convention established
/// in RFC-1 §`config.rs`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PeerRepo {
    /// Human-readable peer name.
    pub name: String,
    /// Repository URL or path.
    pub repo: String,
    /// Path to the specs directory within the peer repository.
    pub specs_path: String,
}

/// Marker trait for project-config types that describe platform peers.
///
/// Intentionally empty in Phase 1 — RFC-3 will extend it
/// with methods like `fn peers(&self) -> &[…];` once the on-disk shape is
/// nailed down.
///
/// Keeping the trait in this crate lets `parse_platform_config` accept a
/// config without `specify-platform` having to depend on the root
/// `specify` crate (which depends on this crate, which would cycle). The
/// root crate satisfies the trait with a zero-method impl in Change I.
pub trait PlatformConfig {}

/// Parse platform peers from a project config.
///
/// Phase-1 stub: returns `vec![]` unconditionally. The signature is frozen
/// so RFC-3 can swap in the real implementation without re-threading
/// callers — the intent is `src/main.rs` and the future `specify drift`
/// subcommand wire through this function today and get correct (empty)
/// results, then automatically pick up real peers once RFC-3 lands.
#[allow(clippy::missing_const_for_fn)]
pub fn parse_platform_config<Cfg: PlatformConfig>(config: &Cfg) -> Vec<PeerRepo> {
    let _ = config;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cfg;
    impl PlatformConfig for Cfg {}

    #[test]
    fn returns_empty_for_any_config() {
        let peers = parse_platform_config(&Cfg);
        assert!(peers.is_empty());
    }

    #[test]
    fn peer_repo_yaml_roundtrip_is_kebab_case() {
        let peer = PeerRepo {
            name: "peer-a".to_string(),
            repo: "github.com/augentic/peer-a".to_string(),
            specs_path: ".specify/specs".to_string(),
        };

        let yaml = serde_saphyr::to_string(&peer).expect("serialise");
        // Kebab-case field naming must appear in the wire format.
        assert!(yaml.contains("specs-path:"), "expected kebab-case field, got:\n{yaml}");
        assert!(!yaml.contains("specs_path:"), "snake_case leaked into yaml:\n{yaml}");

        let round_tripped: PeerRepo = serde_saphyr::from_str(&yaml).expect("deserialise");
        assert_eq!(round_tripped, peer);
    }
}
