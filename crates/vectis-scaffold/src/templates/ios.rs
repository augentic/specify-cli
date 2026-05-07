//! Embedded iOS templates plus their source-to-target path mapping.

/// One iOS template entry.
pub struct IosTemplate {
    /// Target path under `PROJECT_DIR`, with app placeholders allowed.
    pub target: &'static str,
    /// Embedded template contents.
    pub contents: &'static str,
}

/// Embedded iOS registry in write/report order.
pub const TEMPLATES: &[IosTemplate] = &[
    IosTemplate {
        target: "iOS/project.yml",
        contents: include_str!("../../../../templates/vectis/ios/project.yml"),
    },
    IosTemplate {
        target: "iOS/Makefile",
        contents: include_str!("../../../../templates/vectis/ios/Makefile"),
    },
    IosTemplate {
        target: "iOS/__APP_NAME__/__APP_NAME__App.swift",
        contents: include_str!("../../../../templates/vectis/ios/App.swift"),
    },
    IosTemplate {
        target: "iOS/__APP_NAME__/Core.swift",
        contents: include_str!("../../../../templates/vectis/ios/Core.swift"),
    },
    IosTemplate {
        target: "iOS/__APP_NAME__/ContentView.swift",
        contents: include_str!("../../../../templates/vectis/ios/ContentView.swift"),
    },
    IosTemplate {
        target: "iOS/__APP_NAME__/Views/LoadingScreen.swift",
        contents: include_str!("../../../../templates/vectis/ios/LoadingScreen.swift"),
    },
    IosTemplate {
        target: "iOS/__APP_NAME__/Views/HomeScreen.swift",
        contents: include_str!("../../../../templates/vectis/ios/HomeScreen.swift"),
    },
];
