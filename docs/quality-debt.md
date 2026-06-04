# Rust quality debt inventory

Living burn-down for the [quality improvement plan](https://github.com/augentic/specify-cli). Delete this file when T1 items are cleared and T2 is owned by the declarative rule migration track.

## Lint suppressions

| Location | Lint | Tier | Action |
| -------- | ---- | ---- | ------ |
| `crates/standards/src/framework.rs` | module `allow` (pedantic, missing_docs, …) | T2 | Remove with CORE-NNN predicate migration |
| `crates/standards/src/rules.rs` | `module_name_repetitions` | T0 | Keep — wire names `Rule`, `ResolvedRules` |
| `crates/standards/src/lint/model.rs` | `module_name_repetitions` | T0 | Keep — schema `WorkspaceModel` |
| `crates/tool/src/error.rs` | `needless_pass_by_value` | T0 | Keep — Diag helper ergonomics |
| `crates/standards/tests/eval_support/mod.rs` | `dead_code` | T0 | Keep — shared test helpers |
| `tests/common/mod.rs`, `crates/workflow/tests/common/mod.rs` | `dead_code` | T0 | Keep — integration harness |
| `crates/workflow/src/merge/engine.rs` | `too_many_lines` | T1 | Done — split into `merge_into_*` + `apply_*` |
| `crates/workflow/src/merge/slice/read.rs` | `too_many_lines` | T1 | Done — `list_delta_specs`, `merge_delta_spec`, … |
| `crates/workflow/src/registry/workspace/push.rs` | `too_many_lines` | T1 | Done — `prepare_push` / `publish_push` |
| `crates/workflow/src/journal/tests.rs` | `too_many_lines` | T1 | Done — `journal/wire_shapes.rs` + shared `assert_wire_rows` |
| `src/runtime/commands/init.rs` | `too_many_arguments` | T1 | Done — `init::Args` + `run(&Args)` |
| `src/runtime/commands/init.rs`, `upgrade.rs`, `init.rs` (workflow) | `struct_excessive_bools` | T0 | Keep — JSON wire fields |
| `create.rs`, `merge/slice.rs` | `ptr_arg` | T0 | Keep — serde `serialize_with` |
| `crates/tool/src/lib.rs` | `unsafe_code` | T0 | Keep — env test lock |
| `wasi-tools/*` | various | — | Out of host workspace scope |

## Archaeology hotspots

`rust.archaeology-in-doc-comment` is burn-down-only (NOT a hard gate). Genuine historical narrative was stripped from `change/plan/core/propose/kernel.rs`, `slice/build/wire.rs`, `change/plan/core/model.rs` (`Divergence`), and `framework/check/links.rs` (history moved to `DECISIONS.md` pointers, ≤3-line "what today" kept). Workspace count went **214 → 202**.

The residual 202 are the canonical `RFC-NN` / `Phase N` / `DECISIONS.md §…` contract vocabulary the codebase and `AGENTS.md` use as stable anchor names (e.g. "RFC-29 D2", "RFC-36"), not migration history. The predicate's `RFC-`/`Phase ` markers over-fire on them, so promoting a hard gate would be perpetually red; it stays burn-down. Promote only after the markers are narrowed to actual history phrases.

## Test naming / vocabulary

- Renamed `init/workspace/tests.rs` fns under `mod init { … }`.
- Renamed top-level `init/tests.rs` fns.
- Tempdir `"hub"` already absent; workspace tests use `"workspace"`.
- `RustTestNaming` / `RustSourceQuality` via `cargo test --test rust_quality` (specify-cli roots only).
- Test-name burn-down complete: every `#[test]` / `#[tokio::test]` fn is `<= 40` chars and `tests/rust_quality.rs::no_long_test_fn_names` now hard-gates `rust.test-fn-name-too-long`.
- Bare-`#[allow]` burn-down complete: the scanned tree (`crates/` + `src/`) carries zero `#[allow(…)]` without a `reason`, and `tests/rust_quality.rs::no_bare_allow_attributes` now hard-gates `rust.allow-without-reason`.
- `rust.archaeology-in-doc-comment` is the only remaining burn-down-tracked predicate (see "Archaeology hotspots" — deferred, not gated).

## Trait audit (keep unless noted)

| Trait | Verdict |
| ----- | ------- |
| `AtomicYaml` | Keep — shared `.specify/` boundary |
| `Migrator` | Keep — extension point |
| `ShaResolver` | Keep — multiple git call sites + tests |
| `CmdRunner` | Keep — canonical subprocess boundary |
