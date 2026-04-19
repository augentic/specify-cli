//! Top-level `specify` library crate. Phase-1 subcommands, init
//! orchestration, and the curated public API live here.
//!
//! See also: the `specify` binary (`src/main.rs`) and domain crates under
//! `crates/` for the underlying logic.

pub use config::ProjectConfig;
pub use init::{InitOptions, InitResult, VersionMode, init};

pub use specify_change::{
    ChangeMetadata, CreateIfExists, CreateOutcome, EntryKind, Journal, JournalEntry,
    LifecycleStatus, Outcome, Overlap, PhaseOutcome, Plan, PlanChange, PlanChangePatch,
    PlanLockAcquired, PlanLockReleased, PlanLockStamp, PlanLockState, PlanStatus,
    PlanValidationLevel, PlanValidationResult, SpecType, TouchedSpec, actions as change_actions,
    format_rfc3339,
};
pub use specify_drift::{DriftEntry, DriftStatus, baseline_inventory};
pub use specify_error::{Error, ValidationResultSummary};
pub use specify_federation::{FederationConfig, PeerRepo, parse_federation_config};
pub use specify_merge::{
    BaselineConflict, MergeEntry, MergeOperation, MergeResult, conflict_check, merge, merge_change,
    preview_change, validate_baseline,
};
pub use specify_schema::{
    Brief, BriefFrontmatter, CacheMeta, Phase, Pipeline, PipelineEntry, PipelineView,
    ResolvedSchema, Schema, SchemaSource,
};
pub use specify_spec::{
    DeltaSpec, ParsedSpec, RenameEntry, RequirementBlock, Scenario, has_delta_headers,
    parse_baseline, parse_delta,
};
pub use specify_task::{SkillDirective, Task, TaskProgress, mark_complete, parse_tasks};
pub use specify_validate::{
    BriefContext, Classification, CrossContext, CrossRule, Rule, RuleOutcome, ValidationReport,
    ValidationResult, cross_rules, rules_for, serialize_report, validate_change,
};

mod config;
mod init;
