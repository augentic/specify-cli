//! Embedded core templates plus their source-to-target path mapping.

/// One core template entry.
pub(crate) struct CoreTemplate {
    /// Target path under `PROJECT_DIR`.
    pub target: &'static str,
    /// Embedded template contents.
    pub contents: &'static str,
}

/// Embedded core registry in write/report order.
pub(crate) const TEMPLATES: &[CoreTemplate] = &[
    CoreTemplate {
        target: "Cargo.toml",
        contents: include_str!("../../../../../templates/vectis/core/workspace-cargo.toml"),
    },
    CoreTemplate {
        target: "clippy.toml",
        contents: include_str!("../../../../../templates/vectis/core/clippy.toml"),
    },
    CoreTemplate {
        target: "rust-toolchain.toml",
        contents: include_str!("../../../../../templates/vectis/core/rust-toolchain.toml"),
    },
    CoreTemplate {
        target: ".gitignore",
        contents: include_str!("../../../../../templates/vectis/core/gitignore"),
    },
    CoreTemplate {
        target: "shared/Cargo.toml",
        contents: include_str!("../../../../../templates/vectis/core/shared-cargo.toml"),
    },
    CoreTemplate {
        target: "shared/src/lib.rs",
        contents: include_str!("../../../../../templates/vectis/core/lib.rs"),
    },
    CoreTemplate {
        target: "shared/src/app.rs",
        contents: include_str!("../../../../../templates/vectis/core/app.rs"),
    },
    CoreTemplate {
        target: "shared/src/ffi.rs",
        contents: include_str!("../../../../../templates/vectis/core/ffi.rs"),
    },
    CoreTemplate {
        target: "shared/src/bin/codegen.rs",
        contents: include_str!("../../../../../templates/vectis/core/codegen.rs"),
    },
    CoreTemplate {
        target: "deny.toml",
        contents: include_str!("../../../../../templates/vectis/core/deny.toml"),
    },
    CoreTemplate {
        target: "supply-chain/config.toml",
        contents: include_str!("../../../../../templates/vectis/core/supply-chain-config.toml"),
    },
    CoreTemplate {
        target: "supply-chain/audits.toml",
        contents: include_str!("../../../../../templates/vectis/core/supply-chain-audits.toml"),
    },
    CoreTemplate {
        target: "supply-chain/imports.lock",
        contents: include_str!("../../../../../templates/vectis/core/supply-chain-imports.lock"),
    },
];
