# Diagnostics lint unification — status and remaining scope

Status record for the lint unification work originally tracked as **A19** (unify lint output path + framework/consumer dispatch) and **A16** (imperative→declarative lint burn-down). The two former binaries (`specrun` runtime + `specdev` authoring lint) have since converged onto a single `specify` binary; the framework authoring lint is now `specify lint framework`.

**RFC-31** (Accepted, Phases 0–4 complete, 2026-06) landed the engine program and steady-state posture below. Spike records: [`docs/standards/rfc-31-phase1-spike.md`](./docs/standards/rfc-31-phase1-spike.md), [`rfc-31-phase2-spike.md`](./docs/standards/rfc-31-phase2-spike.md). Historical RFC: [augentic/specify `rfcs/done/rfc-31-declarative-lints.md`](https://github.com/augentic/specify/blob/main/rfcs/done/rfc-31-declarative-lints.md).

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

Both lint surfaces converge on one kernel. `specify lint product` ([src/runtime/commands/lint/product.rs](./src/runtime/commands/lint/product.rs)) and `specify lint framework` ([src/runtime/commands/lint/framework.rs](./src/runtime/commands/lint/framework.rs)) each return `Result<()>` and call `output::run_lint(format, || build_report(...))` in [src/output.rs](./src/output.rs). The kernel owns the shared tail — `emit_lint_report` runs the pipeline and renders the envelope, the internal `finish_lint` collapses the outcome into the terminal `Result<()>` (`deny_blocking_findings` on success, the empty-envelope JSON fallback on a pre-emit abort). The two handlers differ only in the `PipelineConfig` their `build_report` closure assembles.

What this closed (relative to the original A19 gap list):

- **Bespoke `Exit` enum** — gone. There is no `src/authoring/exit.rs`; the framework lint's terminal error maps through the one `Exit::from(&Error)` table in [src/runtime/output.rs](./src/runtime/output.rs).
- **Manual error / `eprintln!` paths** — gone. Neither handler writes its own `println!`/`eprintln!`; the abort fallback lives only in `output::finish_lint`.
- **Handler-shape divergence** — gone. Both handlers obey the same `Result<()>` contract documented in [handler-shape.md](./docs/standards/handler-shape.md).
- **Shared kernel** — landed. `output::run_lint` is the single build → emit → finish → blocking-gate kernel; `finish_lint` is internal to it.

Pinned wire contract (unchanged):

- `LintEmit::trailing_newline` — `true` for `specify lint product`, `false` for `specify lint framework` — preserves each surface's historical stdout shape. It is caller config, not normalised, until an intentional wire bump.
- `--format json` / `--output-format json` emit a `DiagnosticReport` on stdout even on infrastructure failure (empty all-zero envelope), so CI consumers keep a stable shape.

Verification: `cargo make check`; `cargo nextest run -p specify-standards --test lint_diagnostics_json --test lint_diagnostics_pretty`; `cargo nextest run -p specify lint`.

---

## A16 — Imperative→declarative lint burn-down — COMPLETE (steady state)

### Steady-state architecture

| Tier | Ids | How it runs |
| --- | --- | --- |
| Native declarative | `CORE-001..008` | `rule_hints` only; no `CORE_ID_TABLE` row |
| Imperative (permanent) | `CORE-009` | `AuthoringProducer` + `framework::check::run` namespace bridge (`run_rules_check`: ownership, `FRAME-*`, dynamic owners, unknown-owner) |
| Declarative + bridge | `CORE-010..052` | `CORE-*` rule files; `kind: authoring-predicate` dispatches closed imperative `rule_id` until native hints reach parity |

There is no Wave-0 duplicate evaluation. `specify lint framework` resolves all `CORE-*` / `UNI-*` rules in one declarative pass, then runs the CORE-009 bridge — not the former full `Check` batch on every invocation.

### CORE-009 policy (unchanged)

The declarative `namespace-owner` rule is an intentional smoke test. It does **not** subsume fused `run_rules_check`. Do not retire the imperative row by weakening checks.

### Optional follow-on (not RFC-31 scope)

Migrating `CORE-010..052` off `authoring-predicate` to native hints is incremental: each id needs parity per [checks.md § Parity contract](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md#parity-contract-for-predicate-retirement). Sidecar schemas for CORE-035/036/047 vs CORE-044: [`rfc-31-sidecar-schemas.md`](./docs/standards/rfc-31-sidecar-schemas.md).

### Performance (post–Phase 4)

`make lint` on `augentic/specify` (2026-06-04): **~247s** wall (`real 246.75`); pre-teardown baseline not captured in-tree. Benchmark locally with `/usr/bin/time make lint`.

---

## Done definition

### A19 — done

- [x] No bespoke `authoring::Exit` enum.
- [x] Lint handlers share one `output::run_lint` kernel; no handler-local `println!`/`eprintln!`.
- [x] `Exit::from(&Error)` is the only exit mapping for lint on both binaries.
- [x] [handler-shape.md](./docs/standards/handler-shape.md) documents the lint kernel explicitly.

### A16 — done (steady state; bridge burn-down optional)

- [x] CORE-001..008 imperative predicates retired; declarative rules own them.
- [x] CORE-009 retained imperative by design (smoke-test declarative counterpart).
- [x] RFC-31 Phase 4: `CORE_ID_TABLE` is CORE-009-only; `check::run` is namespace bridge only; migratable ids use `authoring-predicate` hints.
- [x] Framework lint no longer runs the full imperative `Check` batch on every `make lint`.
- [x] RFC-31 Accepted; spike docs under `docs/standards/rfc-31-*.md`.

---

## Cross-repo touchpoints

| Topic | Repository | Files |
| --- | --- | --- |
| Steady-state posture | specify-cli | [DECISIONS.md](./DECISIONS.md) §"Crate layout", this file §A16 |
| Parity + extension guide | specify | [docs/contributing/checks.md](https://github.com/augentic/specify/blob/main/docs/contributing/checks.md) |
| CORE rule authoring | specify | [adapters/shared/rules/core/README.md](https://github.com/augentic/specify/blob/main/adapters/shared/rules/core/README.md) |
| Declarative CORE rules | specify | `adapters/shared/rules/core/CORE-*.md` |
| Parity harness | specify-cli | [crates/standards/tests/core_parity.rs](./crates/standards/tests/core_parity.rs) |
| Suppression burn-down | specify-cli | [docs/quality-debt.md](./docs/quality-debt.md) |
