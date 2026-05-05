//! Top-level `specify` library crate. Phase-1 subcommands, init
//! orchestration, and the curated public API live here.
//!
//! See also: the `specify` binary (`src/main.rs`) and domain crates under
//! `crates/` for the underlying logic.

pub use config::{ProjectConfig, detect_legacy_layout};
pub use init::{
    HUB_SCHEMA_SENTINEL, InitOptions, InitResult, VersionMode, ensure_specify_gitignore_entries,
    init,
};
pub use specify_change::plan::{Entry as PlanChange, Status as PlanStatus};
pub use specify_change::{
    Acquired as PlanLockAcquired, BlockingPredecessor, CODE_CYCLE, CODE_ORPHAN_SOURCE,
    CODE_STALE_CLONE, CODE_UNREACHABLE, ChangeMetadata, CloneSignature, CreateIfExists,
    CreateOutcome, EntryKind, EntryPatch as PlanChangePatch, Finding as PlanValidationResult,
    Guard as PlanLockGuard, Journal, JournalEntry, LifecycleStatus, METADATA_VERSION, Outcome,
    Overlap, PhaseOutcome, Plan, PlanDoctorDiagnostic, PlanDoctorPayload, PlanDoctorSeverity,
    PlanLockReleased, PlanLockState, Rfc3339Stamp, Severity as PlanValidationLevel, SpecType,
    StaleCloneReason, Stamp as PlanLockStamp, TouchedSpec, actions as change_actions,
    format_rfc3339, is_valid_kebab_name, plan_doctor,
};
pub use specify_error::{Error, ValidationStatus, ValidationSummary};
pub use specify_merge::{
    BaselineConflict, ContractAction, ContractPreviewEntry, Entry as MergeEntry, MergeOperation,
    MergeResult, PreviewResult, conflict_check, merge, merge_change, preview_change,
    validate_baseline,
};
pub use specify_capability::{
    Brief, BriefFrontmatter, CAPABILITY_FILENAME, CacheMeta, Capability, CapabilitySource,
    InitiativeBrief, InitiativeFrontmatter, InitiativeInput, InputKind, LEGACY_SCHEMA_FILENAME,
    ManifestProbe, Phase, Pipeline, PipelineEntry, PipelineView, Registry, RegistryProject,
    ResolvedCapability,
};
pub use specify_spec::{
    DeltaSpec, ParsedSpec, RenameEntry, RequirementBlock, Scenario, has_delta_headers,
    parse_baseline, parse_delta,
};
pub use specify_task::{SkillDirective, Task, TaskProgress, mark_complete, parse_tasks};
pub use specify_validate::{
    BriefContext, Classification, ContractFinding, CrossContext, CrossRule, Rule, RuleOutcome,
    ValidationReport, ValidationResult, cross_rules, rules_for, serialize_report,
    validate_baseline_contracts, validate_change,
};

mod config;
mod init;
mod initiative_finalize;
mod workspace;
mod workspace_merge;

pub use initiative_finalize::{
    FinalizeError, FinalizeInputs, FinalizeOutcome, FinalizeProbe, FinalizeProjectResult,
    FinalizeStatus, FinalizeSummaryCounts, RealFinalizeProbe, classify_pr_state, combine_status,
    is_terminal_for_finalize, load_plan_or_refuse, non_terminal_entries, run_finalize,
    summarise as summarise_finalize,
};
pub use workspace::{
    PushOutcome, SlotKind, SlotStatus, WorkspacePushResult, extract_github_slug,
    run_workspace_push_impl, sync_registry_workspace, workspace_status,
};
pub use workspace_merge::{
    CheckBucket, CheckOverall, GhClient, MergeProjectResult, MergeStatus, PrCheck, PrState, PrView,
    RealGhClient, SPECIFY_BRANCH_PREFIX, classify_checks, classify_status,
    matches_specify_branch_pattern, pr_branch_matches, project_path_for, run_workspace_merge_impl,
};
