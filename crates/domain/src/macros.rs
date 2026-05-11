//! Workspace macros. Currently: `kebab_enum!` for the recurring
//! "C-style enum, kebab-case wire shape, Display matching the wire
//! shape" pattern.

/// Define a copy enum whose serde representation is kebab-case strings
/// matching the variant list, with a matching `Display` implementation
/// and a `const fn as_str(self) -> &'static str` accessor.
///
/// ```ignore
/// kebab_enum! {
///     /// Doc comment forwarded to the generated enum.
///     #[derive(Debug)]
///     pub enum Status {
///         Pending => "pending",
///         InProgress => "in-progress",
///         Done => "done",
///     }
/// }
/// ```
///
/// Generates a `#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize,
/// serde::Deserialize)]` enum with `#[serde(rename_all = "kebab-case")]`,
/// plus the `as_str` and `Display` impls. Additional derives may be added
/// before the enum keyword. The `pub` visibility is forwarded.
#[macro_export]
macro_rules! kebab_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $Name:ident {
            $(
                $(#[$vmeta:meta])*
                $Variant:ident => $kebab:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        $vis enum $Name {
            $(
                $(#[$vmeta])*
                $Variant,
            )+
        }

        impl $Name {
            /// Stable kebab-case wire identifier — also the JSON value
            /// used by serde and the Display output.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$Variant => $kebab, )+
                }
            }
        }

        impl ::core::fmt::Display for $Name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
