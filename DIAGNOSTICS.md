# Diagnostics lint unification — status and remaining scope

Status record for the lint unification work originally tracked as **A19** (unify lint output path + framework/consumer dispatch) and **A16** (imperative→declarative lint burn-down). The two former binaries (`specrun` runtime + `specdev` authoring lint) have since converged onto a single `specify` binary; the framework authoring lint is now `specify lint framework`.

The framework lint engine is now a generic dispatcher with no rule-specific logic or policy; the steady-state posture is below and the authoritative decision is [DECISIONS.md §"Framework lint engine: generic dispatcher (Road A / Road B)"](./DECISIONS.md#framework-lint-engine-generic-dispatcher-road-a--road-b).

Scope: `augentic/specify-cli` (primary) and `augentic/specify` (CORE rule files, docs).

Related docs:

- [DECISIONS.md §"Drained `Error::Validation` and the `Diagnostic` substrate"](./DECISIONS.md#drained-errorvalidation-and-the-diagnostic-substrate)
- [DECISIONS.md §"Crate layout"](./DECISIONS.md) — framework-authoring-checks steady state
- [docs/standards/handler-shape.md](./docs/standards/handler-shape.md) — "The two lint handlers share one tail"
- [adapters/shared/rules/core/README.md](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) (framework repo)
- [docs/contributing/checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md) — parity contract and extension guide (framework repo)
- [docs/quality-debt.md](./docs/quality-debt.md) — suppression burn-down tied to A16

---

## A19 — Unify lint output path and dispatch — COMPLETE

Both lint surfaces converge on one kernel. `specify lint project` ([src/runtime/commands/lint/project.rs](./src/runtime/commands/lint/project.rs)) and `specify lint framework` ([src/runtime/commands/lint/framework.rs](./src/runtime/commands/lint/framework.rs)) each return `Result<()>` and call `output::run_lint(format, || build_report(...))` in [src/output.rs](./src/output.rs). The kernel owns the shared tail — `emit_lint_report` runs the pipeline and renders the envelope, the internal `finish_lint` collapses the outcome into the terminal `Result<()>` (`deny_blocking_findings` on success, the empty-envelope JSON fallback on a pre-emit abort). The two handlers differ only in the `PipelineConfig` their `build_report` closure assembles.

What this closed (relative to the original A19 gap list):

- **Bespoke `Exit` enum** — gone. There is no `src/authoring/exit.rs`; the framework lint's terminal error maps through the one `Exit::from(&Error)` table in [src/runtime/output.rs](./src/runtime/output.rs).
- **Manual error / `eprintln!` paths** — gone. Neither handler writes its own `println!`/`eprintln!`; the abort fallback lives only in `output::finish_lint`.
- **Handler-shape divergence** — gone. Both handlers obey the same `Result<()>` contract documented in [handler-shape.md](./docs/standards/handler-shape.md).
- **Shared kernel** — landed. `output::run_lint` is the single build → emit → finish → blocking-gate kernel; `finish_lint` is internal to it.

Pinned wire contract (unchanged):

- `LintEmit::trailing_newline` — `true` for `specify lint project`, `false` for `specify lint framework` — preserves each surface's historical stdout shape. It is caller config, not normalised, until an intentional wire bump.
- `--format json` / `--output-format json` emit a `DiagnosticReport` on stdout even on infrastructure failure (empty all-zero envelope), so CI consumers keep a stable shape.

Verification: `cargo make check`; `cargo nextest run -p specify-standards --test lint_diagnostics_json --test lint_diagnostics_pretty`; `cargo nextest run -p specify lint`.

---

## A16 — Imperative→declarative lint burn-down — COMPLETE (steady state)

### Steady-state architecture

The engine is a generic dispatcher. Every framework `CORE-*` check is one of two roads (see [DECISIONS.md](./DECISIONS.md#framework-lint-engine-generic-dispatcher-road-a--road-b)):

| Road | Ids (example) | How it runs |
| --- | --- | --- |
| Road A — declarative hint | most of `CORE-001..052` | a generic per-kind evaluator (`lint/eval/*`) interprets the rule's `kind:` (`schema`, `reference-resolves`, `cardinality`, `set-coverage`, `set-eq`, `constant-eq`, `content-digest-eq`, `unique`, `fenced-block`, `regex`, `path-pattern`) over `WorkspaceModel` facts; caps/sets/maps ride the rule's `config:` |
| Road B — referenced WASI tool | `CORE-009`, `CORE-026`, the scenarios / skill / skill-body / agent-teams / adapter / links-registry / marketplace / prose families | `kind: tool` value `<tool>` + a sentinel `path-pattern`; the engine resolves the tool by name and folds its `DiagnosticReport`. Policy rides the rule's `config:`, forwarded as a second positional arg |

There is no Wave-0 duplicate evaluation. `specify lint framework` resolves all `CORE-*` / `UNI-*` rules in one pass; no imperative `Check` producer runs on any invocation, and the `kind: authoring-predicate` bridge is fully removed. CORE-034 (`scenarios.stale-recorded-trace`, a git-only advisory that emitted no finding) was removed rather than ported; its sibling CORE-031 filesystem header validation lives in the `scenarios` tool.

### Policy lives in `specify`, not the engine

Every rule-specific value (a line cap, an owner→prefix map, an expected operation set, a canonical-doc path) rides the rule's `config:` in the `specify` repo — CORE-009's owner→prefix map, source-axis prefixes, and reserved-namespace owners included. The Layer-3 guard test [`crates/standards/tests/lint_no_embedded_policy.rs`](./crates/standards/tests/lint_no_embedded_policy.rs) fails if any eval arm or `framework/check` module reintroduces such a literal; the duplicated owner maps (`BUILTIN_NAMESPACES` / `TARGET_OWNERS`) are deleted.

### Framework tools

Nine framework checkers — `scenarios`, `skill`, `skill-body`, `agent-teams`, `adapter`, `links-registry`, `marketplace`, `prose`, `rules` — live in `wasi-tools/<name>/`, ship a prebuilt `dist/<name>-<ver>.wasm` embedded into the binary via `FrameworkToolRunner` ([`src/runtime/commands/lint/framework_tools.rs`](./src/runtime/commands/lint/framework_tools.rs)), and are name-resolved with `sha256: None` (digest pinning deferred until the source moves to its colocated home).

### Performance (post-migration)

`make lint` on `augentic/specify` (2026-06-04): **~247s** wall (`real 246.75`); pre-teardown baseline not captured in-tree. Benchmark locally with `/usr/bin/time make lint`.

---

## Done definition

### A19 — done

- [x] No bespoke `authoring::Exit` enum.
- [x] Lint handlers share one `output::run_lint` kernel; no handler-local `println!`/`eprintln!`.
- [x] `Exit::from(&Error)` is the only exit mapping for lint on both binaries.
- [x] [handler-shape.md](./docs/standards/handler-shape.md) documents the lint kernel explicitly.

### A16 — done (steady state; engine is a generic dispatcher)

- [x] CORE-001..008 imperative predicates retired; declarative rules own them.
- [x] CORE-009 + CORE-026 migrated to the `rules` WASI tool; `CORE_ID_TABLE` is empty; the CORE-009 `AuthoringProducer` and `framework::check::run` are deleted.
- [x] CORE-034 removed; the `kind: authoring-predicate` bridge (`HintKind::AuthoringPredicate`, `lint/eval/authoring_predicate.rs`, `ScenariosCheck`) deleted. No imperative-predicate bridge remains.
- [x] No rule policy in the engine; `lint_no_embedded_policy` Layer-3 guard is green. `BUILTIN_NAMESPACES` / `TARGET_OWNERS` deleted.
- [x] Framework lint runs no imperative `Check` producer on `make lint`.
- [x] Transitional `core_parity` scaffolding and Road B integration parity tests deleted; coverage rests on the generic per-kind evaluator suite, the schema byte-match gate, and the tools' in-crate tests.

---

## Cross-repo touchpoints

| Topic | Repository | Files |
| --- | --- | --- |
| Steady-state posture | specify-cli | [DECISIONS.md §"Framework lint engine: generic dispatcher (Road A / Road B)"](./DECISIONS.md#framework-lint-engine-generic-dispatcher-road-a--road-b), this file §A16 |
| Contributor model + extension guide | specify | [docs/contributing/checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md) |
| CORE rule authoring | specify | [adapters/shared/rules/core/README.md](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) |
| Declarative CORE rules | specify | `adapters/shared/rules/core/CORE-*.md` |
| Layer-3 policy guard | specify-cli | [crates/standards/tests/lint_no_embedded_policy.rs](./crates/standards/tests/lint_no_embedded_policy.rs) |
| Per-kind evaluator suite | specify-cli | `crates/standards/tests/lint_hint_*.rs` |
| Suppression burn-down | specify-cli | [docs/quality-debt.md](./docs/quality-debt.md) |
