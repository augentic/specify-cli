//! Deterministic Markdown renderer for generated `AGENTS.md` context.

#[cfg(test)]
const PLACEHOLDER_FINGERPRINT: &str = "sha256:pending";
use super::detect::Detection;

/// Complete input needed to render repository context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Input {
    pub(super) project_name: String,
    pub(super) is_hub: bool,
    pub(super) detection: Detection,
    pub(super) domain: Option<String>,
    pub(super) capability: Option<Capability>,
    pub(super) rule_overrides: Vec<Rule>,
    pub(super) declared_tools: Vec<Tool>,
    pub(super) active_slices: Vec<String>,
    pub(super) workspace_peers: Vec<Peer>,
    pub(super) dependencies: Vec<Dep>,
}

/// Capability details surfaced without embedding capability-specific prose in
/// the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Capability {
    pub(super) name: String,
    pub(super) version: u32,
    pub(super) description: String,
    pub(super) briefs: Vec<Brief>,
}

/// One resolved capability brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Brief {
    pub(super) phase: String,
    pub(super) id: String,
    pub(super) description: String,
}

/// One `project.yaml.rules` override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Rule {
    pub(super) brief_id: String,
    pub(super) path: String,
}

/// One project-scoped WASI tool declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tool {
    pub(super) name: String,
    pub(super) version: String,
}

/// One materialized registry workspace slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Peer {
    pub(super) name: String,
    pub(super) path: String,
}

/// One registry peer dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Dep {
    pub(super) name: String,
    pub(super) capability: String,
    pub(super) url: String,
    pub(super) description: Option<String>,
}

/// Render a complete fenced `AGENTS.md` document.
#[cfg(test)]
#[must_use]
fn render_document(input: &Input) -> String {
    render_document_with_fingerprint(input, PLACEHOLDER_FINGERPRINT)
}

/// Render a complete fenced `AGENTS.md` document with a computed fingerprint.
#[must_use]
pub(super) fn render_document_with_fingerprint(input: &Input, fingerprint: &str) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(&one_line(&input.project_name));
    out.push_str(" - Agent Instructions\n\n");
    out.push_str("<!-- specify:context begin\n");
    out.push_str("fingerprint: ");
    out.push_str(fingerprint);
    out.push('\n');
    out.push_str("generated-by: specify ");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push('\n');
    out.push_str("-->\n\n");
    out.push_str(&render_body(input));
    out.push_str("<!-- specify:context end -->\n");
    out
}

/// Render only the managed Markdown body between context fences.
#[must_use]
pub(super) fn render_body(input: &Input) -> String {
    let mut sections = Vec::new();
    if !input.is_hub {
        sections.push(render_section("Runtime", input.detection.runtime_bullets()));
        sections.push(render_section("Tests", input.detection.test_bullets()));
        sections.push(render_section("Linting", input.detection.lint_bullets()));
    }
    sections.push(render_section("Navigation", navigation_bullets(input)));
    sections.push(render_section("Conventions", conventions_bullets(input)));
    sections.push(render_section("Boundaries", boundaries_bullets(input)));
    sections.push(render_section("Dependencies", dependency_bullets(input)));

    let mut body = sections.join("\n");
    body.push('\n');
    body
}

fn render_section(title: &str, mut bullets: Vec<String>) -> String {
    bullets.sort();
    bullets.dedup();

    let mut out = String::new();
    out.push_str("## ");
    out.push_str(title);
    out.push('\n');
    for bullet in bullets {
        out.push_str("- ");
        out.push_str(&bullet);
        out.push('\n');
    }
    out
}

fn navigation_bullets(input: &Input) -> Vec<String> {
    let mut bullets = vec![
        format!("active slices: {} in `.specify/slices/`.", input.active_slices.len()),
        "`.specify/archive/` contains merged or dropped slice history.".to_string(),
        "`.specify/project.yaml` stores Specify project metadata.".to_string(),
        "`.specify/slices/` contains active slice workspaces.".to_string(),
        "`change.md` is the repo-root change brief.".to_string(),
        "`plan.yaml` is the optional repo-root platform plan.".to_string(),
        "`registry.yaml` is the optional repo-root platform registry.".to_string(),
    ];
    for peer in &input.workspace_peers {
        bullets.push(format!(
            "`{}` is the materialized workspace clone for registry peer `{}`.",
            one_line(&peer.path),
            one_line(&peer.name)
        ));
    }
    bullets
}

fn conventions_bullets(input: &Input) -> Vec<String> {
    let mut bullets = Vec::new();
    if let Some(domain) = input.domain.as_deref().map(one_line).filter(|value| !value.is_empty()) {
        bullets.push(format!("project domain: {domain}."));
    }
    if let Some(capability) = &input.capability {
        bullets.push(format!(
            "capability `{}` v{}: {}.",
            one_line(&capability.name),
            capability.version,
            one_line(&capability.description)
        ));
        for brief in &capability.briefs {
            bullets.push(format!(
                "pipeline `{}/{}`: {}.",
                one_line(&brief.phase),
                one_line(&brief.id),
                one_line(&brief.description)
            ));
        }
    }
    for rule in &input.rule_overrides {
        bullets.push(format!(
            "rule override `{}`: `{}`.",
            one_line(&rule.brief_id),
            one_line(&rule.path)
        ));
    }
    if bullets.is_empty() {
        bullets.push("no project rules declared.".to_string());
    }
    bullets
}

fn boundaries_bullets(input: &Input) -> Vec<String> {
    let mut bullets = vec![
        "`.metadata.yaml` files are framework-managed; update them through `specify slice` commands."
            .to_string(),
        "`.specify/archive/` is framework-managed history.".to_string(),
        "`project.yaml` is the source of truth for Specify project metadata.".to_string(),
    ];
    if let Some(capability) = &input.capability {
        bullets.push(format!(
            "capability `{}` owns generated artifact layout.",
            one_line(&capability.name)
        ));
    }
    if input.declared_tools.is_empty() {
        bullets.push("no project-scoped WASI tools declared.".to_string());
    } else {
        for tool in &input.declared_tools {
            bullets.push(format!(
                "declared WASI tool `{}` at version `{}`.",
                one_line(&tool.name),
                one_line(&tool.version)
            ));
        }
    }
    bullets
}

fn dependency_bullets(input: &Input) -> Vec<String> {
    if input.dependencies.is_empty() {
        return vec!["single-repo project; no registered peers.".to_string()];
    }
    input
        .dependencies
        .iter()
        .map(|peer| {
            let mut line = format!(
                "`{}` @ `{}` -> `{}`.",
                one_line(&peer.name),
                one_line(&peer.capability),
                one_line(&peer.url)
            );
            if let Some(description) =
                peer.description.as_deref().map(one_line).filter(|value| !value.is_empty())
            {
                line.push_str(" Description: ");
                line.push_str(&description);
                line.push('.');
            }
            line
        })
        .collect()
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular_input() -> Input {
        Input {
            project_name: "demo".to_string(),
            is_hub: false,
            detection: Detection::default(),
            domain: Some("Rust services".to_string()),
            capability: Some(Capability {
                name: "omnia".to_string(),
                version: 1,
                description: "Omnia Rust WASM workflow".to_string(),
                briefs: vec![
                    Brief {
                        phase: "define".to_string(),
                        id: "specs".to_string(),
                        description: "Write requirements".to_string(),
                    },
                    Brief {
                        phase: "define".to_string(),
                        id: "proposal".to_string(),
                        description: "Establish why".to_string(),
                    },
                ],
            }),
            rule_overrides: vec![Rule {
                brief_id: "proposal".to_string(),
                path: ".specify/rules/proposal.md".to_string(),
            }],
            declared_tools: vec![Tool {
                name: "contract".to_string(),
                version: "1.0.0".to_string(),
            }],
            active_slices: vec!["alpha".to_string(), "zeta".to_string()],
            workspace_peers: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn regular_project_renders_fixed_section_order_and_placeholders() {
        let rendered = render_body(&regular_input());

        let headings: Vec<&str> = rendered.lines().filter(|line| line.starts_with("## ")).collect();
        assert_eq!(
            headings,
            vec![
                "## Runtime",
                "## Tests",
                "## Linting",
                "## Navigation",
                "## Conventions",
                "## Boundaries",
                "## Dependencies",
            ]
        );
        assert!(rendered.contains("## Runtime\n- not detected\n"));
        assert!(rendered.contains("## Tests\n- not detected\n"));
        assert!(rendered.contains("## Linting\n- not detected\n"));
    }

    #[test]
    fn hub_project_omits_detection_sections() {
        let mut input = regular_input();
        input.is_hub = true;
        input.capability = None;

        let rendered = render_body(&input);

        assert!(!rendered.contains("## Runtime"));
        assert!(!rendered.contains("## Tests"));
        assert!(!rendered.contains("## Linting"));
        assert!(rendered.contains("## Navigation"));
        assert!(rendered.contains("## Dependencies"));
    }

    #[test]
    fn dependency_bullets_are_sorted_by_rendered_text() {
        let mut input = regular_input();
        input.dependencies = vec![
            Dep {
                name: "zeta".to_string(),
                capability: "omnia@v1".to_string(),
                url: "../zeta".to_string(),
                description: None,
            },
            Dep {
                name: "alpha".to_string(),
                capability: "omnia@v1".to_string(),
                url: "../alpha".to_string(),
                description: None,
            },
        ];

        let rendered = render_body(&input);
        let alpha = rendered.find("`alpha` @ `omnia@v1`").expect("alpha dependency rendered");
        let zeta = rendered.find("`zeta` @ `omnia@v1`").expect("zeta dependency rendered");

        assert!(alpha < zeta, "dependencies must render in sorted order:\n{rendered}");
    }

    #[test]
    fn dependency_bullets_include_descriptions_when_present() {
        let mut input = regular_input();
        input.dependencies = vec![Dep {
            name: "alpha".to_string(),
            capability: "omnia@v1".to_string(),
            url: "../alpha".to_string(),
            description: Some("Alpha service".to_string()),
        }];

        let rendered = render_body(&input);

        assert!(
            rendered.contains("`alpha` @ `omnia@v1` -> `../alpha`. Description: Alpha service."),
            "dependency description must render when present:\n{rendered}"
        );
    }

    #[test]
    fn navigation_lists_materialized_workspace_peers() {
        let mut input = regular_input();
        input.workspace_peers = vec![Peer {
            name: "billing".to_string(),
            path: ".specify/workspace/billing/".to_string(),
        }];

        let rendered = render_body(&input);

        assert!(
            rendered.contains(
                "`.specify/workspace/billing/` is the materialized workspace clone for registry peer `billing`."
            ),
            "workspace peer path must be repo-relative:\n{rendered}"
        );
    }

    #[test]
    fn full_document_contains_context_fences() {
        let rendered = render_document(&regular_input());

        assert!(rendered.starts_with("# demo - Agent Instructions\n\n"));
        assert!(rendered.contains("<!-- specify:context begin\n"));
        assert!(rendered.contains("generated-by: specify "));
        assert!(rendered.ends_with("<!-- specify:context end -->\n"));
    }
}
