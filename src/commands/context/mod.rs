#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to these command handlers."
)]
//! `specify context {generate, check}` command surface.
//!
//! This module owns the deterministic renderer, fenced `AGENTS.md` write
//! policy, and `.specify/context.lock` drift checks.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

mod detect;
mod fences;
mod fingerprint;
mod lock;
mod render;

use serde::Serialize;
use specify::{
    Capability, Error, ManifestProbe, PipelineView, ProjectConfig, SliceMetadata,
    is_workspace_clone_path,
};
use specify_registry::Registry;
use specify_slice::atomic::atomic_bytes_write;

use crate::cli::{ContextAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run(ctx: &CommandContext, action: ContextAction) -> Result<CliResult, Error> {
    match action {
        ContextAction::Generate { check, force } => run_generate(ctx, check, force),
        ContextAction::Check => run_check(ctx),
    }
}

pub fn run_generate(ctx: &CommandContext, check: bool, force: bool) -> Result<CliResult, Error> {
    if is_workspace_clone_path(&ctx.project_dir) {
        return Err(Error::Config(format!(
            "specify context generate: refusing to run inside a workspace clone at {}; \
             run context generation in the owning project instead",
            ctx.project_dir.display()
        )));
    }

    let body = generate(ctx, check, force)?;
    emit_generate_output(ctx.format, &body)?;

    Ok(if check && body.changed { CliResult::GenericFailure } else { CliResult::Success })
}

pub(super) fn generate_for_init(ctx: &CommandContext) -> Result<ContextGenerateOutcome, Error> {
    let body = generate(ctx, false, false)?;
    Ok(ContextGenerateOutcome {
        changed: body.changed,
        disposition: body.disposition,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContextGenerateOutcome {
    pub(super) changed: bool,
    pub(super) disposition: &'static str,
}

fn generate(ctx: &CommandContext, check: bool, force: bool) -> Result<GenerateBody, Error> {
    let (generated, context_fingerprint) = render_context(ctx)?;
    let expected_lock = lock::ContextLock::from_fingerprint(&context_fingerprint);
    let lock_path = context_lock_path(ctx);
    let existing_lock = lock::load(&lock_path)?;
    let agents_path = ctx.project_dir.join("AGENTS.md");
    let existing = read_optional(&agents_path)?;
    if !check {
        refuse_modified_fenced_body(existing.as_deref(), existing_lock.as_ref(), force)?;
    }
    let planned = fences::plan_agents_write(existing.as_deref(), generated.as_bytes(), force)
        .map_err(error_from_fence)?;
    let agents_changed = planned.disposition != fences::WriteDisposition::Unchanged;
    let lock_changed = existing_lock.as_ref() != Some(&expected_lock);
    let changed = agents_changed || lock_changed;

    if agents_changed && !check {
        atomic_bytes_write(&agents_path, &planned.bytes)?;
    }
    if lock_changed && !check {
        lock::save(&lock_path, &expected_lock)?;
    }

    Ok(GenerateBody {
        status: generate_status(check, changed),
        path: "AGENTS.md",
        check,
        force,
        changed,
        agents_changed,
        lock_changed,
        disposition: disposition_label(planned.disposition),
    })
}

fn render_context(
    ctx: &CommandContext,
) -> Result<(String, fingerprint::ContextFingerprint), Error> {
    let assembly = assemble_render_input(ctx)?;
    let aggregate =
        fingerprint::aggregate_fingerprint(env!("CARGO_PKG_VERSION"), assembly.inputs.clone());
    let generated = render::render_document_with_fingerprint(&assembly.input, &aggregate);
    let fenced = fences::parse_document(generated.as_bytes())
        .map_err(|err| Error::Config(err.to_string()))?
        .ok_or_else(|| {
            Error::Config(
                "context-generated-document-missing-fences: generated AGENTS.md content must \
                 contain a Specify context fence"
                    .to_string(),
            )
        })?;
    let context_fingerprint =
        fingerprint::context_fingerprint(env!("CARGO_PKG_VERSION"), assembly.inputs, fenced.body());
    Ok((generated, context_fingerprint))
}

pub fn run_check(ctx: &CommandContext) -> Result<CliResult, Error> {
    let body = check(ctx)?;
    emit_check_output(ctx.format, &body)?;
    Ok(if body.status == "up-to-date" { CliResult::Success } else { CliResult::GenericFailure })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "CLI JSON response mirrors independent check flags and write outcomes."
)]
struct GenerateBody {
    status: &'static str,
    path: &'static str,
    check: bool,
    force: bool,
    changed: bool,
    agents_changed: bool,
    lock_changed: bool,
    disposition: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckBody {
    status: &'static str,
    fingerprint: CheckFingerprint,
    inputs_changed: Vec<String>,
    inputs_added: Vec<String>,
    inputs_removed: Vec<String>,
    fences_modified: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CheckFingerprint {
    expected: Option<String>,
    actual: Option<String>,
}

fn emit_generate_output(format: OutputFormat, body: &GenerateBody) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(body)?,
        OutputFormat::Text => print_generate_text(body),
    }
    Ok(())
}

fn print_generate_text(body: &GenerateBody) {
    match body.status {
        "would-update" => {
            println!("context is out of date; run `specify context generate` to refresh it");
        }
        "unchanged" => println!("AGENTS.md is up to date"),
        "written" if body.agents_changed => println!("wrote AGENTS.md"),
        "written" => println!("wrote .specify/context.lock"),
        _ => println!("context generate finished"),
    }
}

const fn generate_status(check: bool, changed: bool) -> &'static str {
    match (check, changed) {
        (true, true) => "would-update",
        (_, false) => "unchanged",
        (false, true) => "written",
    }
}

const fn disposition_label(disposition: fences::WriteDisposition) -> &'static str {
    match disposition {
        fences::WriteDisposition::Create => "create",
        fences::WriteDisposition::ForceRewriteUnfenced => "force-rewrite-unfenced",
        fences::WriteDisposition::ReplaceFencedBlock => "replace-fenced-block",
        fences::WriteDisposition::Unchanged => "unchanged",
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Io(err)),
    }
}

fn check(ctx: &CommandContext) -> Result<CheckBody, Error> {
    let agents_path = ctx.project_dir.join("AGENTS.md");
    let agents = read_optional(&agents_path)?;
    let existing_lock = lock::load(&context_lock_path(ctx))?;
    let (_generated, actual_fingerprint) = render_context(ctx)?;
    let actual_lock = lock::ContextLock::from_fingerprint(&actual_fingerprint);

    if agents.is_none() {
        return Ok(CheckBody {
            status: "context-not-generated",
            fingerprint: check_fingerprint(existing_lock.as_ref(), Some(&actual_lock)),
            inputs_changed: Vec::new(),
            inputs_added: Vec::new(),
            inputs_removed: Vec::new(),
            fences_modified: false,
        });
    }

    let Some(expected_lock) = existing_lock else {
        return Ok(CheckBody {
            status: "context-lock-missing",
            fingerprint: check_fingerprint(None, Some(&actual_lock)),
            inputs_changed: Vec::new(),
            inputs_added: Vec::new(),
            inputs_removed: Vec::new(),
            fences_modified: false,
        });
    };

    let diff = lock::diff_inputs(&expected_lock.inputs, &actual_lock.inputs);
    let fences_modified = fences_modified(
        agents
            .as_deref()
            .expect("agents bytes are present because missing AGENTS.md returned above"),
        &expected_lock,
    );
    let has_input_drift =
        !diff.changed.is_empty() || !diff.added.is_empty() || !diff.removed.is_empty();
    let has_fingerprint_drift = expected_lock.fingerprint != actual_lock.fingerprint;
    let status = if has_fingerprint_drift || has_input_drift || fences_modified {
        "drift"
    } else {
        "up-to-date"
    };

    Ok(CheckBody {
        status,
        fingerprint: check_fingerprint(Some(&expected_lock), Some(&actual_lock)),
        inputs_changed: diff.changed,
        inputs_added: diff.added,
        inputs_removed: diff.removed,
        fences_modified,
    })
}

fn emit_check_output(format: OutputFormat, body: &CheckBody) -> Result<(), Error> {
    match format {
        OutputFormat::Json => emit_response(body)?,
        OutputFormat::Text => print_check_text(body),
    }
    Ok(())
}

fn print_check_text(body: &CheckBody) {
    match body.status {
        "up-to-date" => println!("context up to date"),
        "context-not-generated" => println!("context-not-generated: AGENTS.md is missing"),
        "context-lock-missing" => {
            println!("context-lock-missing: .specify/context.lock is missing");
        }
        "drift" => {
            println!("context drift detected");
            print_drift_list("inputs changed", &body.inputs_changed);
            print_drift_list("inputs added", &body.inputs_added);
            print_drift_list("inputs removed", &body.inputs_removed);
            if body.fences_modified {
                println!("fences modified: true");
            }
        }
        _ => println!("context check finished"),
    }
}

fn print_drift_list(label: &str, paths: &[String]) {
    if !paths.is_empty() {
        println!("{label}: {}", paths.join(", "));
    }
}

fn check_fingerprint(
    expected: Option<&lock::ContextLock>, actual: Option<&lock::ContextLock>,
) -> CheckFingerprint {
    CheckFingerprint {
        expected: expected.map(|lock| lock.fingerprint.clone()),
        actual: actual.map(|lock| lock.fingerprint.clone()),
    }
}

fn fences_modified(agents: &[u8], expected_lock: &lock::ContextLock) -> bool {
    match fences::parse_document(agents) {
        Ok(Some(current)) => {
            fingerprint::body_sha256(current.body()) != expected_lock.fences.body_sha256
        }
        Ok(None) | Err(_) => true,
    }
}

fn refuse_modified_fenced_body(
    agents: Option<&[u8]>, existing_lock: Option<&lock::ContextLock>, force: bool,
) -> Result<(), Error> {
    if force {
        return Ok(());
    }
    let (Some(agents), Some(existing_lock)) = (agents, existing_lock) else {
        return Ok(());
    };
    let Some(current) = fences::parse_document(agents).map_err(error_from_fence)? else {
        return Ok(());
    };
    let actual_body = fingerprint::body_sha256(current.body());
    if actual_body != existing_lock.fences.body_sha256 {
        return Err(Error::ContextDrift);
    }
    Ok(())
}

fn error_from_fence(err: fences::FenceError) -> Error {
    match err {
        fences::FenceError::ExistingUnfencedAgentsMd => Error::ContextUnfenced,
        other => Error::Config(other.to_string()),
    }
}

fn context_lock_path(ctx: &CommandContext) -> std::path::PathBuf {
    ProjectConfig::specify_dir(&ctx.project_dir).join("context.lock")
}

struct RenderAssembly {
    input: render::ContextRenderInput,
    inputs: Vec<fingerprint::InputFingerprint>,
}

fn assemble_render_input(ctx: &CommandContext) -> Result<RenderAssembly, Error> {
    let mut collector = fingerprint::InputCollector::new(&ctx.project_dir);
    collector.add_file(&ProjectConfig::config_path(&ctx.project_dir))?;

    let registry = Registry::load(&ctx.project_dir)?;
    collector.add_file_if_present(&ProjectConfig::registry_path(&ctx.project_dir))?;

    let capability = if ctx.config.hub {
        None
    } else {
        let pipeline = ctx.load_pipeline()?;
        collect_capability_inputs(&mut collector, &pipeline)?;
        Some(capability_summary(&pipeline))
    };
    let detection = if ctx.config.hub {
        detect::Detection::default()
    } else {
        detect::detect_root_markers(&ctx.project_dir)
    };
    emit_detection_warnings(&detection.warnings);
    collector.add_relative_files(detection.input_paths.iter().map(String::as_str))?;

    // The renderer's `plan.yaml` guidance is unconditional navigation text. It
    // does not inspect `plan.yaml` existence or content, so that file is not
    // fingerprinted unless a future renderer contract actually reads it.
    let active_slices =
        active_slice_names(&ProjectConfig::slices_dir(&ctx.project_dir), &mut collector)?;

    let input = render::ContextRenderInput {
        project_name: ctx.config.name.clone(),
        is_hub: ctx.config.hub,
        detection,
        domain: ctx.config.domain.clone(),
        capability,
        rule_overrides: rule_overrides(&ctx.config),
        declared_tools: declared_tools(&ctx.config),
        active_slices,
        workspace_peers: materialized_workspace_peers(registry.as_ref(), &ctx.project_dir)?,
        dependencies: dependency_peers(registry.as_ref()),
    };
    Ok(RenderAssembly {
        input,
        inputs: collector.finalize()?,
    })
}

fn collect_capability_inputs(
    collector: &mut fingerprint::InputCollector, pipeline: &PipelineView,
) -> Result<(), Error> {
    match Capability::probe_dir(&pipeline.capability.root_dir) {
        ManifestProbe::Found(path) | ManifestProbe::Legacy(path) => {
            collector.add_file(&path)?;
        }
        ManifestProbe::Missing => {}
    }
    for (_phase, brief) in &pipeline.briefs {
        collector.add_file(&brief.path)?;
    }
    Ok(())
}

fn capability_summary(pipeline: &PipelineView) -> render::CapabilitySummary {
    let mut briefs: Vec<render::BriefSummary> = pipeline
        .briefs
        .iter()
        .map(|(phase, brief)| render::BriefSummary {
            phase: phase.to_string(),
            id: brief.frontmatter.id.clone(),
            description: brief.frontmatter.description.clone(),
        })
        .collect();
    briefs.sort_by(|left, right| {
        (&left.phase, &left.id, &left.description).cmp(&(
            &right.phase,
            &right.id,
            &right.description,
        ))
    });

    render::CapabilitySummary {
        name: pipeline.capability.manifest.name.clone(),
        version: pipeline.capability.manifest.version,
        description: pipeline.capability.manifest.description.clone(),
        briefs,
    }
}

fn rule_overrides(config: &ProjectConfig) -> Vec<render::RuleOverride> {
    let mut overrides: Vec<render::RuleOverride> = config
        .rules
        .iter()
        .filter(|(_brief_id, path)| !path.is_empty())
        .map(|(brief_id, path)| render::RuleOverride {
            brief_id: brief_id.clone(),
            path: format!(".specify/{path}"),
        })
        .collect();
    overrides
        .sort_by(|left, right| (&left.brief_id, &left.path).cmp(&(&right.brief_id, &right.path)));
    overrides
}

fn declared_tools(config: &ProjectConfig) -> Vec<render::DeclaredTool> {
    let mut tools: Vec<render::DeclaredTool> = config
        .tools
        .iter()
        .map(|tool| render::DeclaredTool {
            name: tool.name.clone(),
            version: tool.version.clone(),
        })
        .collect();
    tools.sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    tools
}

fn dependency_peers(registry: Option<&Registry>) -> Vec<render::DependencyPeer> {
    let Some(registry) = registry else {
        return Vec::new();
    };
    if registry.projects.len() <= 1 {
        return Vec::new();
    }

    let mut peers: Vec<render::DependencyPeer> = registry
        .projects
        .iter()
        .map(|project| render::DependencyPeer {
            name: project.name.clone(),
            capability: project.schema.clone(),
            url: project.url.clone(),
            description: project.description.clone(),
        })
        .collect();
    peers.sort_by(|left, right| {
        (&left.name, &left.capability, &left.url).cmp(&(&right.name, &right.capability, &right.url))
    });
    peers
}

fn materialized_workspace_peers(
    registry: Option<&Registry>, project_dir: &Path,
) -> Result<Vec<render::WorkspacePeer>, Error> {
    let Some(registry) = registry else {
        return Ok(Vec::new());
    };
    if registry.projects.len() <= 1 {
        return Ok(Vec::new());
    }

    let workspace_dir = ProjectConfig::specify_dir(project_dir).join("workspace");
    let mut peers = Vec::new();
    for project in &registry.projects {
        let path = workspace_dir.join(&project.name);
        match fs::symlink_metadata(path) {
            Ok(_) => peers.push(render::WorkspacePeer {
                name: project.name.clone(),
                path: format!(".specify/workspace/{}/", project.name),
            }),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(Error::Io(err)),
        }
    }
    peers.sort_by(|left, right| (&left.path, &left.name).cmp(&(&right.path, &right.name)));
    Ok(peers)
}

fn active_slice_names(
    slices_dir: &Path, collector: &mut fingerprint::InputCollector,
) -> Result<Vec<String>, Error> {
    if !slices_dir.exists() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for entry in std::fs::read_dir(slices_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let candidate = entry.path();
        let metadata_path = SliceMetadata::path(&candidate);
        if !metadata_path.is_file() {
            continue;
        }
        collector.add_file(&metadata_path)?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn emit_detection_warnings(warnings: &[detect::DetectionWarning]) {
    for warning in warnings {
        eprintln!("warning: {}: {}", warning.path, warning.message);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    fn write_minimal_capability(project_dir: &Path) {
        let capability_dir = project_dir.join("schemas").join("mini");
        let briefs_dir = capability_dir.join("briefs");
        fs::create_dir_all(&briefs_dir).expect("create capability dirs");
        fs::write(
            capability_dir.join("capability.yaml"),
            "name: mini\nversion: 1\ndescription: Mini capability\npipeline:\n  define:\n    - id: proposal\n      brief: briefs/proposal.md\n  build: []\n  merge: []\n",
        )
        .expect("write capability");
        fs::write(
            briefs_dir.join("proposal.md"),
            "---\nid: proposal\ndescription: Draft the proposal\ngenerates: proposal.md\n---\n",
        )
        .expect("write brief");
    }

    fn sample_config() -> ProjectConfig {
        let mut rules = BTreeMap::new();
        rules.insert("proposal".to_string(), "rules/proposal.md".to_string());
        ProjectConfig {
            name: "demo".to_string(),
            domain: Some("demo domain".to_string()),
            capability: Some("mini".to_string()),
            specify_version: None,
            rules,
            tools: Vec::new(),
            hub: false,
        }
    }

    #[test]
    fn assemble_render_input_reads_existing_metadata_in_sorted_order() {
        let tmp = tempdir().expect("tempdir");
        write_minimal_capability(tmp.path());
        let slices_dir = ProjectConfig::slices_dir(tmp.path());
        fs::create_dir_all(slices_dir.join("zeta")).expect("create zeta");
        fs::create_dir_all(slices_dir.join("alpha")).expect("create alpha");
        fs::write(slices_dir.join("zeta").join(".metadata.yaml"), "not parsed").expect("zeta meta");
        fs::write(slices_dir.join("alpha").join(".metadata.yaml"), "also not parsed")
            .expect("alpha meta");
        fs::write(
            Registry::path(tmp.path()),
            "version: 1\nprojects:\n  - name: zeta\n    url: ../zeta\n    schema: mini@v1\n    description: Zeta service\n  - name: alpha\n    url: ../alpha\n    schema: mini@v1\n    description: Alpha service\n",
        )
        .expect("write registry");
        fs::create_dir_all(ProjectConfig::config_path(tmp.path()).parent().expect("config parent"))
            .expect("create .specify");
        fs::write(
            ProjectConfig::config_path(tmp.path()),
            "name: demo\ncapability: mini\nrules:\n  proposal: rules/proposal.md\n",
        )
        .expect("write project config");
        let ctx = CommandContext {
            format: OutputFormat::Text,
            project_dir: tmp.path().to_path_buf(),
            config: sample_config(),
        };

        let assembly = assemble_render_input(&ctx).expect("assemble render input");
        let input = &assembly.input;

        assert_eq!(input.active_slices, vec!["alpha".to_string(), "zeta".to_string()]);
        assert_eq!(
            input.dependencies.iter().map(|peer| peer.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(
            input.capability.as_ref().map(|capability| capability.name.as_str()),
            Some("mini")
        );
        assert_eq!(
            input.rule_overrides,
            vec![render::RuleOverride {
                brief_id: "proposal".to_string(),
                path: ".specify/rules/proposal.md".to_string(),
            }]
        );
        assert_eq!(
            assembly.inputs.iter().map(|input| input.path.as_str()).collect::<Vec<_>>(),
            vec![
                ".specify/project.yaml",
                ".specify/slices/alpha/.metadata.yaml",
                ".specify/slices/zeta/.metadata.yaml",
                "registry.yaml",
                "schemas/mini/briefs/proposal.md",
                "schemas/mini/capability.yaml",
            ]
        );
    }

    #[test]
    fn assemble_render_input_skips_pipeline_for_hubs() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir_all(ProjectConfig::config_path(tmp.path()).parent().expect("config parent"))
            .expect("create .specify");
        fs::write(ProjectConfig::config_path(tmp.path()), "name: platform\nhub: true\n")
            .expect("write project config");
        let ctx = CommandContext {
            format: OutputFormat::Text,
            project_dir: tmp.path().to_path_buf(),
            config: ProjectConfig {
                name: "platform".to_string(),
                domain: None,
                capability: None,
                specify_version: None,
                rules: BTreeMap::new(),
                tools: Vec::new(),
                hub: true,
            },
        };

        let assembly = assemble_render_input(&ctx).expect("hub assembly");
        let input = &assembly.input;

        assert!(input.is_hub);
        assert!(input.capability.is_none());
        assert!(input.dependencies.is_empty());
        assert_eq!(
            assembly.inputs.iter().map(|input| input.path.as_str()).collect::<Vec<_>>(),
            vec![".specify/project.yaml"]
        );
    }
}
