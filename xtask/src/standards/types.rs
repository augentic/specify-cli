//! Shared count and baseline types for the standards engine.
//!
//! [`Counts`] is the per-file live tally produced by the predicate
//! engine; [`FileBaseline`] is the persisted TOML row in
//! `scripts/standards-allowlist.toml`. They mirror each other field by
//! field — keep the two in sync when adding a predicate.

use serde::Deserialize;

pub(super) const DEFAULT_LINE_CAP: u32 = 400;

#[derive(Default, Debug)]
pub(super) struct Counts {
    pub(super) inline_dtos: u32,
    pub(super) format_match_dispatch: u32,
    pub(super) rfc_numbers_in_code: u32,
    pub(super) ritual_doc_paragraphs: u32,
    pub(super) no_op_forwarders: u32,
    pub(super) error_envelope_inlined: u32,
    pub(super) path_helper_inlined: u32,
    pub(super) direct_fs_write: u32,
    pub(super) stale_cli_vocab: u32,
    pub(super) module_line_count: u32,
    pub(super) result_cliresult_default: u32,
    pub(super) verbose_doc_paragraphs: u32,
    pub(super) cli_help_shape: u32,
    pub(super) display_serde_mirror: u32,
    pub(super) crate_root_prose: u32,
    pub(super) unit_test_serde_roundtrip: u32,
    pub(super) mod_rs_forbidden: u32,
}

impl Counts {
    pub(super) fn iter(&self) -> impl Iterator<Item = (&'static str, u32)> {
        [
            ("inline-dtos", self.inline_dtos),
            ("format-match-dispatch", self.format_match_dispatch),
            ("rfc-numbers-in-code", self.rfc_numbers_in_code),
            ("ritual-doc-paragraphs", self.ritual_doc_paragraphs),
            ("no-op-forwarders", self.no_op_forwarders),
            ("error-envelope-inlined", self.error_envelope_inlined),
            ("path-helper-inlined", self.path_helper_inlined),
            ("direct-fs-write", self.direct_fs_write),
            ("stale-cli-vocab", self.stale_cli_vocab),
            ("module-line-count", self.module_line_count),
            ("result-cliresult-default", self.result_cliresult_default),
            ("verbose-doc-paragraphs", self.verbose_doc_paragraphs),
            ("cli-help-shape", self.cli_help_shape),
            ("display-serde-mirror", self.display_serde_mirror),
            ("crate-root-prose", self.crate_root_prose),
            ("unit-test-serde-roundtrip", self.unit_test_serde_roundtrip),
            ("mod-rs-forbidden", self.mod_rs_forbidden),
        ]
        .into_iter()
    }

    pub(super) const fn into_baseline(self) -> FileBaseline {
        FileBaseline {
            inline_dtos: self.inline_dtos,
            format_match_dispatch: self.format_match_dispatch,
            rfc_numbers_in_code: self.rfc_numbers_in_code,
            ritual_doc_paragraphs: self.ritual_doc_paragraphs,
            no_op_forwarders: self.no_op_forwarders,
            error_envelope_inlined: self.error_envelope_inlined,
            path_helper_inlined: self.path_helper_inlined,
            direct_fs_write: self.direct_fs_write,
            stale_cli_vocab: self.stale_cli_vocab,
            module_line_count: self.module_line_count,
            result_cliresult_default: self.result_cliresult_default,
            verbose_doc_paragraphs: self.verbose_doc_paragraphs,
            cli_help_shape: self.cli_help_shape,
            display_serde_mirror: self.display_serde_mirror,
            crate_root_prose: self.crate_root_prose,
            unit_test_serde_roundtrip: self.unit_test_serde_roundtrip,
            mod_rs_forbidden: self.mod_rs_forbidden,
        }
    }
}

#[derive(Debug, Default, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) struct FileBaseline {
    #[serde(default)]
    pub(super) inline_dtos: u32,
    #[serde(default)]
    pub(super) format_match_dispatch: u32,
    #[serde(default)]
    pub(super) rfc_numbers_in_code: u32,
    #[serde(default)]
    pub(super) ritual_doc_paragraphs: u32,
    #[serde(default)]
    pub(super) no_op_forwarders: u32,
    #[serde(default)]
    pub(super) error_envelope_inlined: u32,
    #[serde(default)]
    pub(super) path_helper_inlined: u32,
    #[serde(default)]
    pub(super) direct_fs_write: u32,
    #[serde(default)]
    pub(super) stale_cli_vocab: u32,
    #[serde(default)]
    pub(super) module_line_count: u32,
    #[serde(default)]
    pub(super) result_cliresult_default: u32,
    #[serde(default)]
    pub(super) verbose_doc_paragraphs: u32,
    #[serde(default)]
    pub(super) cli_help_shape: u32,
    #[serde(default)]
    pub(super) display_serde_mirror: u32,
    #[serde(default)]
    pub(super) crate_root_prose: u32,
    #[serde(default)]
    pub(super) unit_test_serde_roundtrip: u32,
    #[serde(default)]
    pub(super) mod_rs_forbidden: u32,
}

impl FileBaseline {
    pub(super) fn allowed(&self, key: &str) -> u32 {
        match key {
            "inline-dtos" => self.inline_dtos,
            "format-match-dispatch" => self.format_match_dispatch,
            "rfc-numbers-in-code" => self.rfc_numbers_in_code,
            "ritual-doc-paragraphs" => self.ritual_doc_paragraphs,
            "no-op-forwarders" => self.no_op_forwarders,
            "error-envelope-inlined" => self.error_envelope_inlined,
            "path-helper-inlined" => self.path_helper_inlined,
            "direct-fs-write" => self.direct_fs_write,
            "stale-cli-vocab" => self.stale_cli_vocab,
            "module-line-count" => self.module_line_count,
            "result-cliresult-default" => self.result_cliresult_default,
            "verbose-doc-paragraphs" => self.verbose_doc_paragraphs,
            "cli-help-shape" => self.cli_help_shape,
            "display-serde-mirror" => self.display_serde_mirror,
            "crate-root-prose" => self.crate_root_prose,
            "unit-test-serde-roundtrip" => self.unit_test_serde_roundtrip,
            "mod-rs-forbidden" => self.mod_rs_forbidden,
            _ => 0,
        }
    }

    /// Effective per-file cap. Most predicates default to 0 (new files
    /// start clean); `module-line-count` defaults to `DEFAULT_LINE_CAP`.
    pub(super) fn cap(&self, key: &str) -> u32 {
        if key == "module-line-count" && self.module_line_count == 0 {
            DEFAULT_LINE_CAP
        } else {
            self.allowed(key)
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
