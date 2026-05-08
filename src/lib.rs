#![allow(
    clippy::multiple_crate_versions,
    reason = "The RFC-15 tool runner pulls in Wasmtime/WASI transitive versions the workspace cannot unify yet."
)]

//! Top-level `specify` library crate. Phase-1 subcommands, init
//! orchestration, and the curated public API live here.
//!
//! See also: the `specify` binary (`src/main.rs`) and domain crates under
//! `crates/` for the underlying logic.

pub use config::{ProjectConfig, detect_legacy_layout, is_workspace_clone_path};
pub use init::{InitOptions, InitResult, VersionMode, init};
pub use specify_capability::{
    Brief, BriefFrontmatter, CAPABILITY_FILENAME, CHANGE_BRIEF_FILENAME, CacheMeta, Capability,
    CapabilitySource, ChangeBrief, ChangeFrontmatter, ChangeInput, InputKind,
    LEGACY_CHANGE_BRIEF_FILENAME, LEGACY_SCHEMA_FILENAME, ManifestProbe, Phase, Pipeline,
    PipelineEntry, PipelineView, ResolvedCapability,
};
pub use specify_error::{Error, ValidationStatus, ValidationSummary};
pub use specify_merge::{
    ArtifactClass, BaselineConflict, MergeOperation, MergePreviewEntry as MergeEntry, MergeResult,
    MergeStrategy, OpaqueAction, OpaquePreviewEntry, PreviewResult, conflict_check, merge,
    merge_slice, preview_slice, validate_baseline,
};
pub use specify_slice::{
    CreateIfExists, CreateOutcome, EntryKind, Journal, JournalEntry, LifecycleStatus,
    METADATA_VERSION, Outcome, Overlap, PhaseOutcome, Rfc3339Stamp, SLICES_DIR_NAME, SliceMetadata,
    SpecType, TouchedSpec, actions as slice_actions, format_rfc3339, is_valid_kebab_name,
};
pub use specify_spec::{
    DeltaSpec, ParsedSpec, RenameEntry, RequirementBlock, Scenario, has_delta_headers,
    parse_baseline, parse_delta,
};
pub use specify_task::{SkillDirective, Task, TaskProgress, mark_complete, parse_tasks};
pub use specify_validate::{ValidationReport, ValidationResult, serialize_report, validate_slice};

mod config;
mod init;
