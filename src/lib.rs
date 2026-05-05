//! Top-level `specify` library crate. Phase-1 subcommands, init
//! orchestration, and the curated public API live here.
//!
//! See also: the `specify` binary (`src/main.rs`) and domain crates under
//! `crates/` for the underlying logic.

pub use config::{ProjectConfig, detect_legacy_layout};
pub use init::{InitOptions, InitResult, VersionMode, init};
pub use specify_capability::{
    Brief, BriefFrontmatter, CAPABILITY_FILENAME, CacheMeta, Capability, CapabilitySource,
    InitiativeBrief, InitiativeFrontmatter, InitiativeInput, InputKind, LEGACY_SCHEMA_FILENAME,
    ManifestProbe, Phase, Pipeline, PipelineEntry, PipelineView, ResolvedCapability,
};
pub use specify_change::{
    ChangeMetadata, CreateIfExists, CreateOutcome, EntryKind, Journal, JournalEntry,
    LifecycleStatus, METADATA_VERSION, Outcome, Overlap, PhaseOutcome, Rfc3339Stamp, SpecType,
    TouchedSpec, actions as change_actions, format_rfc3339, is_valid_kebab_name,
};
pub use specify_error::{Error, ValidationStatus, ValidationSummary};
pub use specify_merge::{
    BaselineConflict, ContractAction, ContractPreviewEntry, Entry as MergeEntry, MergeOperation,
    MergeResult, PreviewResult, conflict_check, merge, merge_change, preview_change,
    validate_baseline,
};
pub use specify_spec::{
    DeltaSpec, ParsedSpec, RenameEntry, RequirementBlock, Scenario, has_delta_headers,
    parse_baseline, parse_delta,
};
pub use specify_task::{SkillDirective, Task, TaskProgress, mark_complete, parse_tasks};
pub use specify_validate::{
    ValidationReport, ValidationResult, serialize_report, validate_change,
};

mod config;
mod init;
