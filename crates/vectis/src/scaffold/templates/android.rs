//! Embedded Android templates plus their source-to-target path mapping.

use crate::scaffold::templates::Capability;

/// One Android template entry.
pub struct AndroidTemplate {
    /// Target path under `PROJECT_DIR`, with app/package placeholders allowed.
    pub target: &'static str,
    /// Embedded template contents.
    pub contents: &'static str,
    /// Whole-file inclusion predicate.
    pub include_when: IncludeWhen,
}

/// Whole-file inclusion predicate.
#[derive(Debug, Clone, Copy)]
pub enum IncludeWhen {
    /// File is rendered regardless of selected capabilities.
    Always,
    /// File is rendered iff any listed capability is selected.
    AnyOf(&'static [Capability]),
}

impl IncludeWhen {
    /// Should this entry be rendered for the given cap selection?
    #[must_use]
    pub fn should_include(self, caps: &[Capability]) -> bool {
        match self {
            Self::Always => true,
            Self::AnyOf(needed) => needed.iter().any(|c| caps.contains(c)),
        }
    }
}

/// Embedded Android registry in write/report order.
pub const TEMPLATES: &[AndroidTemplate] = &[
    AndroidTemplate {
        target: "Android/Makefile",
        contents: include_str!("../../../../../templates/vectis/android/Makefile"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/.gitignore",
        contents: include_str!("../../../../../templates/vectis/android/gitignore"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/build.gradle.kts",
        contents: include_str!("../../../../../templates/vectis/android/root-build.gradle.kts"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/settings.gradle.kts",
        contents: include_str!("../../../../../templates/vectis/android/settings.gradle.kts"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/gradle.properties",
        contents: include_str!("../../../../../templates/vectis/android/gradle.properties"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/gradle/libs.versions.toml",
        contents: include_str!("../../../../../templates/vectis/android/libs.versions.toml"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/build.gradle.kts",
        contents: include_str!("../../../../../templates/vectis/android/app-build.gradle.kts"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/shared/build.gradle.kts",
        contents: include_str!("../../../../../templates/vectis/android/shared-build.gradle.kts"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/AndroidManifest.xml",
        contents: include_str!("../../../../../templates/vectis/android/AndroidManifest.xml"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/res/values/themes.xml",
        contents: include_str!("../../../../../templates/vectis/android/themes.xml"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/res/xml/network_security_config.xml",
        contents: include_str!(
            "../../../../../templates/vectis/android/network-security-config.xml"
        ),
        include_when: IncludeWhen::AnyOf(&[Capability::Http, Capability::Sse]),
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/__APP_NAME__Application.kt",
        contents: include_str!("../../../../../templates/vectis/android/Application.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/MainActivity.kt",
        contents: include_str!("../../../../../templates/vectis/android/MainActivity.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/core/Core.kt",
        contents: include_str!("../../../../../templates/vectis/android/Core.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/ui/screens/LoadingScreen.kt",
        contents: include_str!("../../../../../templates/vectis/android/LoadingScreen.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/ui/screens/HomeScreen.kt",
        contents: include_str!("../../../../../templates/vectis/android/HomeScreen.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/ui/theme/Color.kt",
        contents: include_str!("../../../../../templates/vectis/android/Color.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/ui/theme/Theme.kt",
        contents: include_str!("../../../../../templates/vectis/android/Theme.kt"),
        include_when: IncludeWhen::Always,
    },
    AndroidTemplate {
        target: "Android/app/src/main/java/__ANDROID_PACKAGE_PATH__/ui/theme/Type.kt",
        contents: include_str!("../../../../../templates/vectis/android/Type.kt"),
        include_when: IncludeWhen::Always,
    },
];
