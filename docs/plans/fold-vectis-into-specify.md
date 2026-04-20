# Fold `vectis` CLI into `specify`

> **Plan reference for cross-session work.** Mirror of the in-flight plan
> originally created in plan mode. Update this file directly when the plan
> changes; agents picking up work should read it from
> `docs/plans/fold-vectis-into-specify.md` rather than the per-user Cursor
> plan cache.

**Overview.** Fold the standalone `vectis` CLI from
`../specify/crates/vectis-cli/` into `specify-cli` as a `specify vectis ...`
subcommand tree, adopt the `specify` v2 JSON contract for its outputs, then
remove all vectis-CLI artifacts from `../specify` and rewrite the
vectis-plugin references to point at the new home.

## Chunk status

| ID | Chunk | Status |
| --- | --- | --- |
| chunk-1-move | Move `../specify/crates/vectis-cli/` and `../specify/templates/vectis/` into `specify-cli` verbatim; rename crate to `specify-vectis`; add to workspace; verify it still builds and tests as a binary. | completed |
| chunk-2-lib | Convert `specify-vectis` from binary to library: lift arg structs and `CommandOutcome` into `lib.rs`, re-export the four `run` handlers, drop `[[bin]]` and delete `main.rs`. | completed |
| chunk-3-dispatch | Add `specify-vectis` as a dependency, extend `Commands` with a `Vectis { action }` variant + `VectisAction` subcommand enum in `src/main.rs`, and dispatch through to the library handlers (legacy snake_case payload, kebab-case error variants only). | pending |
| chunk-4-v2 | Rewrite every `serde_json::json!` literal in `crates/vectis/src/{init,verify,add_shell,update_versions}/*.rs` and `error.rs` to kebab-case keys/error variants; update unit tests; rely on `emit_json` to inject `schema-version: 2`. | pending |
| chunk-5-text-tests | Add `--format text` renderers for the four vectis verbs in `src/main.rs` and write `tests/vectis.rs` integration tests covering help, success JSON shape, invalid-project error, and missing-prerequisites error. | pending |
| chunk-6-clean-specify | In `../specify`: delete `crates/vectis-cli/`, `templates/vectis/`, and the prebuilt `vectis` binary; drop the workspace member from `Cargo.toml` (and the file/`[workspace]` block if empty); remove `Makefile` `build-vectis` target; remove `/vectis` from `.gitignore`; regenerate or delete `Cargo.lock`. | pending |
| chunk-7-plugin-docs | In `../specify`: rewrite `plugins/vectis/skills/template-updater/{SKILL.md,references/known-drift.md}` and the writer SKILLs to use `specify vectis ...` and `<specify-cli>/crates/vectis/...` paths; refresh `docs/vectis.md`, `docs/architecture.md`, `README.md`; banner-mark `rfcs/rfc-6-*.md` as superseded; verify grep is clean. | pending |

When you complete a chunk, flip its status (`pending` → `in_progress` →
`completed`) and add a short note under "Notes" at the bottom if anything
deviated from the plan.

## Decisions captured up-front

- **JSON contract**: vectis subcommands adopt the `specify` v2 contract — kebab-case keys, `schema-version: 2` envelope on object responses, `--format text|json` with `text` default, mapped exit codes via `emit_error`. Plugin skills + RFC-6 are updated to match.
- **Plugin location**: `../specify/plugins/vectis/` stays where it is; only its file contents are updated.
- **Library carve-out**: vectis becomes a workspace library crate `specify-vectis` at `crates/vectis/`. The `specify` binary depends on it; there is no separate `vectis` binary anywhere after migration.
- **Templates layout is preserved** (`templates/vectis/{core,ios,android}/`) and `embedded/versions.toml` stays a sibling of `src/`, so every existing `include_str!` path keeps the same number of `..` segments and needs no rewrite.

## End state

```
specify-cli/
├── Cargo.toml                    # workspace members += "crates/vectis"
├── crates/vectis/                # ex-`crates/vectis-cli`, no [[bin]], lib only
│   ├── Cargo.toml                # name = "specify-vectis"
│   ├── embedded/versions.toml
│   └── src/{lib.rs, error.rs, prerequisites.rs, versions.rs,
│            init/, verify/, add_shell/, update_versions/, templates/}
├── templates/vectis/{core,ios,android}/   # moved verbatim
├── src/main.rs                   # new Commands::Vectis { action } variant
└── tests/vectis.rs               # JSON contract + smoke tests for `specify vectis`

../specify/
├── (no Cargo.toml / Cargo.lock / Makefile build-vectis / .gitignore /vectis)
├── (no crates/, no templates/)
└── plugins/vectis/...            # text-only edits to point at `specify vectis`
                                  # and the new template paths in specify-cli
```

## Chunks (each runnable by a separate sequential agent session)

Each chunk ends with a concrete verification step. A fresh agent can pick up any chunk from `git status` + this plan alone.

### Chunk 1 — Land the vectis source tree in `specify-cli` (verbatim move)

Goal: get every byte of vectis source into the new repo, preserving relative paths so existing `include_str!`s resolve unchanged. No behavioural edits.

- Copy `../specify/crates/vectis-cli/` → `crates/vectis/` (directory rename).
- Copy `../specify/templates/vectis/` → `templates/vectis/`.
- Edit `crates/vectis/Cargo.toml`:
  - `name = "specify-vectis"` (was `vectis-cli`).
  - Adopt workspace inheritance for `version`/`edition`/`license`/`repository` so it matches the rest of `crates/`.
  - Keep the existing `[[bin]] name = "vectis"` block for now — chunk 2 deletes it.
- Edit root [`Cargo.toml`](../../Cargo.toml): add `crates/vectis` to `[workspace] members` (already present is `crates/{change,drift,error,federation,merge,schema,spec,task,validate}`).
- Confirm by ripgrep that all `include_str!("../../../../templates/vectis/...")` paths in `crates/vectis/src/templates/{core,ios,android}.rs` and `include_str!("../embedded/versions.toml")` in `crates/vectis/src/versions.rs` resolve under the new layout (no edits needed — same depth).
- Verify: `cargo build -p specify-vectis` and `cargo test -p specify-vectis` both green from the `specify-cli` checkout. The `target/debug/vectis` binary still exists at this point — that's fine.

### Chunk 2 — Convert `specify-vectis` from binary to library

Goal: expose the four subcommand handlers as a library API the `specify` binary can call; delete the standalone `vectis` binary.

- Promote the clap arg structs (`InitArgs`, `VerifyArgs`, `AddShellArgs`, `UpdateVersionsArgs`) and the `CommandOutcome` enum out of `crates/vectis/src/main.rs` into a new `crates/vectis/src/lib.rs` (or `args.rs`). They must remain `clap::Args` so `specify`'s clap derive can flatten them.
  - **Visibility bump (chunk-1 discovery)**: in the current `main.rs` the four arg structs are declared `pub(crate) struct …Args` (their fields are already `pub`, and `CommandOutcome` is already `pub` with `pub` variants). Chunk 2 must change each struct from `pub(crate)` → `pub` so the dispatcher in `specify` can name them.
  - **Submodule visibility (chunk-1 discovery)**: in `main.rs` the handler modules are declared as private (`mod init; mod verify; mod add_shell; mod update_versions; mod error;` plus internal-only `mod prerequisites; mod templates; mod versions;`). In `lib.rs` make `init`, `verify`, `add_shell`, `update_versions`, and `error` `pub mod`; keep `prerequisites`, `templates`, and `versions` private (they are used only by the handler modules).
  - **No edits needed in submodules (chunk-1 discovery)**: `init/mod.rs`, `verify/mod.rs`, `add_shell/mod.rs`, and `update_versions/mod.rs` already reference the args structs as `crate::InitArgs` etc. and `CommandOutcome` as `crate::CommandOutcome`. Once those types live in `lib.rs` (the new crate root), `crate::…` continues to resolve correctly without any submodule changes.
- Re-export from `lib.rs`:
  - `pub use error::{VectisError, MissingTool};`
  - `pub use {init, verify, add_shell, update_versions};` (each module already has `pub fn run(&Args) -> Result<CommandOutcome, VectisError>`).
  - `pub use args::{InitArgs, VerifyArgs, AddShellArgs, UpdateVersionsArgs};` (or define them inline in `lib.rs`).
  - `pub use CommandOutcome;` (the type is already `pub` at the current `main.rs` root).
- Drop the `[[bin]]` section from `crates/vectis/Cargo.toml`. Delete `crates/vectis/src/main.rs`.
- Verify: `cargo build -p specify-vectis` (lib only), `cargo test -p specify-vectis`, and `cargo build -p specify` still succeed. No `vectis` binary should be produced anywhere (after chunk 2, `target/debug/vectis` from chunk 1 will need a `cargo clean -p specify-vectis` to disappear; that's expected).

### Chunk 3 — Wire `specify vectis ...` into the dispatcher

Goal: surface the four vectis verbs under the `specify` binary, plumbed through to the library, but still emitting the legacy snake_case payload (kebab-case rewrite is chunk 4 to keep the diffs reviewable).

- Add `specify-vectis = { path = "crates/vectis" }` to the root [`Cargo.toml`](../../Cargo.toml) `[dependencies]`.
- In [`src/main.rs`](../../src/main.rs):
  - Extend the top-level `Commands` enum (currently lines 97–166 in `src/main.rs`) with a new variant:

```rust
/// Bootstrap and verify Crux cross-platform projects (RFC-6).
Vectis {
    #[command(subcommand)]
    action: VectisAction,
},
```

  - Define `enum VectisAction { Init(specify_vectis::InitArgs), Verify(specify_vectis::VerifyArgs), AddShell(specify_vectis::AddShellArgs), UpdateVersions(specify_vectis::UpdateVersionsArgs) }`.
  - **`--format` plumbing (chunk-1 discovery)**: `OutputFormat` is already a global flag on `Cli` (`--format text|json`, default `text`, defined at `src/main.rs:91-95` and consumed via `cli.format`). `run_vectis` should take that existing value; no new flag required.
  - Add `fn run_vectis(format: OutputFormat, action: &VectisAction) -> i32` that calls the matching `specify_vectis::<module>::run(args)` and turns the result into an exit code:
    - `Ok(CommandOutcome::Success(value))` → `emit_json(value)` then `EXIT_SUCCESS`.
    - `Ok(CommandOutcome::Stub { command })` → emit a `not-implemented` error and return `EXIT_GENERIC_FAILURE`. **Note (chunk-1 discovery)**: today's `main.rs` in vectis emits this as snake_case `not_implemented` (line 161 of `crates/vectis/src/main.rs` before deletion in chunk 2). Chunk 3 emits the kebab-case form directly; do **not** route the stub through the library — synthesize the JSON in the dispatcher.
    - `Err(VectisError)` → call a new `emit_vectis_error(format, &err)` that mirrors the existing `emit_error`/`emit_json_error` pattern (`src/main.rs:2750-2809`): pick the kebab-case variant string, attach `message` + `exit-code`, exit `2` for `MissingPrerequisites`, `1` otherwise.
  - **Don't reuse `emit_json_error` (chunk-1 discovery)**: the existing helper is hard-coded against the `Error` enum from `specify_error` (`src/main.rs:2785-2809`). Add a sibling `emit_vectis_error(format, &VectisError)` (or a small generic helper that takes a kebab variant + message + code). Mirror the existing `emit_json` helper at `src/main.rs:2773-2783`, which already auto-injects `"schema-version": 2` on object responses.
  - **`Cli`/`Command` clap roots are gone (chunk-2 discovery)**: chunk 2 deleted `crates/vectis/src/main.rs` outright, including its `#[derive(Parser)] struct Cli` and `#[derive(Subcommand)] enum Command` (with `name = "vectis"`). Chunk 3 must define `VectisAction` from scratch under the existing `specify` `Cli`; there is nothing to copy across, only the four arg structs to flatten in.
  - **Library shape (chunk-2 discovery)**: `CommandOutcome` and the four arg structs (`InitArgs`, `VerifyArgs`, `AddShellArgs`, `UpdateVersionsArgs`) live directly at the `specify_vectis` crate root (defined inline in `crates/vectis/src/lib.rs`, no `args` submodule). `pub use error::{MissingTool, VectisError};` is already in place. Reference them from `src/main.rs` as `specify_vectis::InitArgs`, `specify_vectis::CommandOutcome`, `specify_vectis::VectisError`, etc.
- Define a sibling exit-code constant if needed; reusing `EXIT_VALIDATION_FAILED = 2` for "missing prerequisites" is acceptable because the meaning is local to the `vectis` subtree (document this in the module-level doc comment).
- Verify: `specify vectis --help` lists four subcommands; `specify vectis init Foo --dir <tmpdir>` runs end-to-end against the embedded version pins; `cargo test --workspace` is green.

### Chunk 4 — Rewrite vectis payloads & errors for the v2 contract

Goal: every JSON byte the `specify vectis ...` tree emits matches the kebab-case + `schema-version` rules already enforced by `specify`.

- Inside `crates/vectis/src/{init,verify,add_shell,update_versions}/*.rs`, rewrite every `serde_json::json!({ ... })` literal so keys are kebab-case. Hot spots include `app_name`, `project_dir`, `detected_capabilities`, `unrecognized_capabilities`, `build_steps`, `version_file`, `assemblies`, `combos`, `verification`, `dry_run`, `passed`, `not_implemented`. Apply recursively (nested objects too).
- Rewrite `crates/vectis/src/error.rs` `VectisError::to_json` (currently `crates/vectis/src/error.rs:68-92`) so the `error` value is kebab-case (`missing-prerequisites`, `invalid-project`, `verify`, `internal`, `io`). The two existing unit tests at the bottom of `error.rs` already assert on `missing_prerequisites` and `invalid_project` — flip them to kebab-case at the same time.
- Update every test in `crates/vectis/` (and `tests/` if it grew one) that asserts on snake_case keys. Run them to confirm coverage.
- The `emit_json` helper in `src/main.rs` already auto-injects `"schema-version": 2` on object responses, so vectis payloads inherit that for free once they go through chunk 3's dispatcher.
- Verify: `cargo test -p specify-vectis`; `cargo test --workspace`; manual sanity check `specify vectis init Foo --dir /tmp/v --format json | jq 'keys'` returns kebab-case keys with `schema-version`.

### Chunk 5 — Add `--format text` renderers and integration tests

Goal: round out the v2 contract by giving `--format text` (the default) a humanised view, and lock the JSON contract behind `tests/vectis.rs`.

- Extend `run_vectis` (or per-subcommand helpers) in [`src/main.rs`](../../src/main.rs) so each verb has a text renderer. Suggested shapes:
  - `vectis init`: `Created N files in <dir>. Assemblies: core PASS, ios PASS, android FAIL (cargo check)`.
  - `vectis verify`: per-assembly bullet list with first error line for failing steps.
  - `vectis add-shell`: same as init, scoped to the platform.
  - `vectis update-versions`: bullet list of `crate: old → new`; when `--verify`, append per-combo pass/fail.
- Add `tests/vectis.rs` covering at minimum:
  - `--help` for `specify vectis` lists four subcommands.
  - Success JSON for `init` contains `"schema-version": 2` and kebab-case keys.
  - Error JSON for an `--version-file` pointing at a missing path emits `"error": "invalid-project"` and `"exit-code": 1`.
  - Error JSON when prerequisites are missing emits `"error": "missing-prerequisites"`, `"exit-code": 2`. (Use a guard such as gating this test behind a `VECTIS_PREREQ_TEST` env var if making prereqs reliably absent in CI is hard — note this in the test.)
- Verify: `cargo test --workspace` green.

### Chunk 6 — Delete vectis artifacts from `../specify`

Goal: leave `../specify` with no source, no build wiring, and no committed binary that exists only to define the vectis CLI.

- `rm -rf ../specify/crates/vectis-cli` and `rm -rf ../specify/templates/vectis`. If `../specify/crates/` is empty afterwards, remove it.
- Edit `../specify/Cargo.toml`: drop the `crates/vectis-cli` workspace member. If `members` becomes empty, delete the entire `[workspace]` block (and likely the file, since `../specify` is a docs/plugins repo with no other Rust code — confirm by `rg --type rust` first).
- Edit `../specify/Makefile`: delete the `build-vectis` target. Keep `checks`, `dev-plugins`, `prod-plugins`. If the file ends up only containing those, that's fine.
- Edit `../specify/.gitignore`: remove the `/vectis` line.
- Delete the prebuilt `../specify/vectis` binary.
- Run `cargo update --workspace` (or just delete `../specify/Cargo.lock` if no Rust crates remain) so the lockfile no longer references `vectis-cli`.
- Verify: `rg -l 'vectis-cli|target/release/vectis|build-vectis|crates/vectis-cli' ../specify` returns matches **only** inside `rfcs/rfc-6-*.md` (historical) — every other hit must have been resolved.

### Chunk 7 — Update plugin & doc references in `../specify`

Goal: the `vectis` plugin and surrounding docs work against the new `specify vectis ...` invocation, and grep is clean of stale paths.

- `../specify/plugins/vectis/skills/template-updater/SKILL.md` and its `references/known-drift.md`:
  - Replace every `./target/release/vectis ...` and bare `vectis ...` invocation with `specify vectis ...`.
  - Replace repo-relative paths so they refer to the `specify-cli` repo:
    - `crates/vectis-cli/src/templates/...` → `<specify-cli>/crates/vectis/src/templates/...`.
    - `crates/vectis-cli/src/add_shell/parser.rs` → `<specify-cli>/crates/vectis/src/add_shell/parser.rs`.
    - `crates/vectis-cli/src/verify/...` → `<specify-cli>/crates/vectis/src/verify/...`.
    - `crates/vectis-cli/embedded/versions.toml` → `<specify-cli>/crates/vectis/embedded/versions.toml`.
    - `templates/vectis/{core,ios,android}/...` → `<specify-cli>/templates/vectis/...`.
  - Replace `cargo build --release -p vectis-cli`, `cargo test -p vectis-cli`, `cargo clippy ... -p vectis-cli` with the `-p specify-vectis` equivalents and add a one-liner that the build runs inside the `specify-cli` checkout.
- `../specify/plugins/vectis/skills/{ios-writer,core-writer,android-writer}/SKILL.md`: rewrite any residual `vectis-cli` / `cargo run --package vectis-cli` mentions discovered via grep.
- `../specify/docs/vectis.md`: replace any "build the vectis CLI" instructions with `cargo install --path <specify-cli> --bin specify` (or `brew install augentic/specify` if the Formula is the canonical install path) and use `specify vectis ...` throughout.
- `../specify/docs/architecture.md` and `../specify/README.md`: refresh the structure diagrams + usage snippets.
- `../specify/rfcs/rfc-6-vectis-bootstrap.md` and `../specify/rfcs/rfc-6-tasks.md`: prepend a short banner ("Status: superseded by the `specify vectis` subcommand in augentic/specify-cli; this RFC documents the original standalone-binary design.") rather than rewriting history. Leave the body intact.
- Verify, from `../specify`:
  - `rg -n '\bvectis\s+(init|verify|add-shell|update-versions)\b' | rg -v ':\s*$|specify\s+vectis'` returns nothing — i.e. every active invocation now starts with `specify vectis`.
  - `rg -n 'vectis-cli|target/release/vectis|cargo .* vectis-cli|build-vectis|crates/vectis-cli|^/vectis$' .` returns hits only in the two RFC files (now under their banner).
  - The vectis plugin's `template-updater` SKILL.md "Validate" command sequence is fully runnable as written from the `specify-cli` checkout.

## Cross-cutting notes for any agent picking up a chunk

- `git status` at the start of work shows `M Cargo.lock` already on the `specify-cli` side — leave it alone unless your chunk requires regenerating it.
- The vectis source tree is ~6,300 lines across 22 Rust files; do not refactor opportunistically. Behavioural edits are restricted to chunks 3 and 4. Chunks 1, 2, 5, 6, 7 should produce mechanical diffs.
- The `../specify` repo is a separate git checkout; commit changes there as a sibling commit (or PR) rather than mixing them with the `specify-cli` commit history.
- After chunk 7, both repos should be installable/buildable with no manual fix-ups: `cargo install --path . --bin specify` in `specify-cli` produces a binary that satisfies every step the `vectis` plugin asks for.

## Notes (post-execution log)

- **chunk-1-move (completed)**: Copied `../specify/crates/vectis-cli/` →
  `crates/vectis/` and `../specify/templates/vectis/` →
  `templates/vectis/` (both via `cp -R`; chunk 6 will delete the
  originals from `../specify`). Rewrote `crates/vectis/Cargo.toml` to
  `name = "specify-vectis"` and adopted workspace inheritance for
  `version`/`edition`/`license`/`repository`. Folded `thiserror`,
  `serde`, and `serde_json` onto the workspace versions
  (`thiserror.workspace = true`, etc.); the remaining direct deps
  (`clap`, `roxmltree`, `syn`, `toml`, `ureq`) are not in
  `[workspace.dependencies]` and were left as-is — chunks 2–7 do not
  depend on this. Added `crates/vectis` to the root workspace
  `members` list (between `crates/federation` and end of array).
  Verification: `cargo build -p specify-vectis` and `cargo test -p
  specify-vectis` both green (118 tests pass), and
  `cargo build --workspace` + `cargo test --workspace` are green
  end-to-end. The chunk-1 binary `target/debug/vectis` exists as
  expected; chunk 2 will drop the `[[bin]]` block.
- **Forward-looking discoveries folded into chunks 2–4**:
  - Chunk 2 visibility section now spells out the `pub(crate) struct …Args`
    → `pub struct …Args` change and the `pub mod` exposure for
    `init`/`verify`/`add_shell`/`update_versions`/`error`.
  - Chunk 2 confirms submodule files reference args via `crate::…` already,
    so no edits are needed inside `init/`, `verify/`, `add_shell/`,
    `update_versions/`.
  - Chunk 3 calls out that `OutputFormat` already exists as a global
    `--format` flag on `Cli`, and that the existing `emit_json_error` is
    `Error`-typed and cannot be reused for `VectisError`.
  - Chunk 3 makes explicit that the `Stub` outcome's
    `not_implemented` payload is synthesized in the dispatcher (kebab
    form, no library round-trip), so chunk 4 does not need to touch
    `CommandOutcome::Stub` rendering at all.
  - Chunk 4 anchors the kebab-case rewrite on the exact line range in
    `error.rs` and notes the two existing snake_case test assertions.
- **chunk-2-lib (completed)**: Created `crates/vectis/src/lib.rs`
  containing the four arg structs (now `pub struct …Args` with `pub`
  fields, were `pub(crate)`), the `pub enum CommandOutcome` (already
  `pub` previously), `pub use error::{MissingTool, VectisError};`, and
  `pub mod` declarations for `add_shell`/`error`/`init`/
  `update_versions`/`verify` (with `prerequisites`, `templates`,
  `versions` left private). `CommandOutcome` and the arg structs are
  defined inline at the crate root rather than in a separate `args`
  module — chunk 3 should refer to them as `specify_vectis::InitArgs`
  etc. directly. Dropped the `[[bin]] name = "vectis"` block from
  `crates/vectis/Cargo.toml` and deleted `crates/vectis/src/main.rs`
  (including its `Cli`/`Command` clap roots — chunk 3 must define
  `VectisAction` from scratch). Verification: `cargo build -p
  specify-vectis` (lib only), `cargo test -p specify-vectis` (118
  passed), and `cargo build -p specify` and `cargo test --workspace`
  all green; `target/debug/vectis` is gone (cargo removed the binary
  on rebuild — no `cargo clean` needed).
