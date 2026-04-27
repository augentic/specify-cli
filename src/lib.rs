//! Top-level `specify` library crate. Phase-1 subcommands, init
//! orchestration, and the curated public API live here.
//!
//! See also: the `specify` binary (`src/main.rs`) and domain crates under
//! `crates/` for the underlying logic.

pub use config::ProjectConfig;
pub use init::{InitOptions, InitResult, VersionMode, ensure_specify_gitignore_entries, init};
pub use specify_change::{
    Acquired as PlanLockAcquired, ChangeMetadata, CreateIfExists, CreateOutcome, EntryKind,
    EntryPatch as PlanChangePatch, Finding as PlanValidationResult, Guard as PlanLockGuard,
    Journal, JournalEntry, LifecycleStatus, Outcome, Overlap, PhaseOutcome, Plan, PlanLockReleased,
    PlanLockState, Rfc3339Stamp, Severity as PlanValidationLevel, SpecType,
    Stamp as PlanLockStamp, TouchedSpec, actions as change_actions, format_rfc3339,
    is_valid_kebab_name,
    plan::Entry as PlanChange,
    plan::Status as PlanStatus,
};
pub use specify_drift::{Entry as DriftEntry, Status as DriftStatus, baseline_inventory};
pub use specify_error::{Error, ValidationSummary, ValidationStatus};
pub use specify_federation::{FederationConfig, PeerRepo, parse_federation_config};
pub use specify_merge::{
    BaselineConflict, ContractAction, ContractPreviewEntry, Entry as MergeEntry, MergeOperation,
    MergeResult, PreviewResult, conflict_check, merge, merge_change, preview_change,
    validate_baseline,
};
pub use specify_schema::{
    Brief, BriefFrontmatter, CacheMeta, InitiativeBrief, InitiativeFrontmatter, InitiativeInput,
    InputKind, Phase, Pipeline, PipelineEntry, PipelineView, Registry, RegistryProject,
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
mod workspace;

pub use workspace::{
    PushOutcome, SlotKind, SlotStatus, WorkspacePushResult, extract_github_slug,
    run_workspace_push_impl, sync_registry_workspace, workspace_status,
};
