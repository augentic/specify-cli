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
| chunk-3-dispatch | Add `specify-vectis` as a dependency, extend `Commands` with a `Vectis { action }` variant + `VectisAction` subcommand enum in `src/main.rs`, and dispatch through to the library handlers (legacy snake_case payload, kebab-case error variants only). | completed |
| chunk-4-v2 | Rewrite every `serde_json::json!` literal in `crates/vectis/src/{init,verify,add_shell,update_versions}/*.rs` and `error.rs` to kebab-case keys/error variants; update unit tests; rely on `emit_json` to inject `schema-version: 2`. | completed |
| chunk-5-text-tests | Add `--format text` renderers for the four vectis verbs in `src/main.rs` and write `tests/vectis.rs` integration tests covering help, success JSON shape, invalid-project error, and missing-prerequisites error. | completed |
| chunk-6-clean-specify | In `../specify`: delete `crates/vectis-cli/`, `templates/vectis/`, and the prebuilt `vectis` binary; drop the workspace member from `Cargo.toml` (and the file/`[workspace]` block if empty); remove `Makefile` `build-vectis` target; remove `/vectis` from `.gitignore`; regenerate or delete `Cargo.lock`. | completed |
| chunk-7-plugin-docs | In `../specify`: rewrite `plugins/vectis/skills/template-updater/{SKILL.md,references/known-drift.md}` and the writer SKILLs to use `specify vectis ...` and `<specify-cli>/crates/vectis/...` paths; refresh `docs/vectis.md`, `docs/architecture.md`, `README.md`; banner-mark `rfcs/rfc-6-*.md` as superseded; verify grep is clean. | completed |

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
- Edit root [`Cargo.toml`](../../Cargo.toml): add `crates/vectis` to `[workspace] members` (already present is `crates/{change,drift,error,platform,merge,schema,spec,task,validate}`).
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
- **Confirmed snake_case keys observed live (chunk-3 discovery)**: `specify --format json vectis init Foo --dir <tmp>` currently emits the top-level keys `app_name`, `app_struct`, `assemblies`, `capabilities`, `project_dir`, `shells`, plus the auto-injected `schema-version`. Chunk 4 must rename `app_name` → `app-name`, `app_struct` → `app-struct`, `project_dir` → `project-dir`. The nested `assemblies.<name>.{files,status}` payload is already kebab-safe for the success path; double-check for snake_case in failing-assembly shapes.
- Rewrite `crates/vectis/src/error.rs` `VectisError::to_json` (currently `crates/vectis/src/error.rs:68-92`) so the `error` value is kebab-case (`missing-prerequisites`, `invalid-project`, `verify`, `internal`, `io`). The two existing unit tests at the bottom of `error.rs` already assert on `missing_prerequisites` and `invalid_project` — flip them to kebab-case at the same time.
- **`MissingTool` field names are already kebab-safe (chunk-3 discovery)**: the `MissingTool` struct's fields (`tool`, `assembly`, `check`, `install`) are single-word and survive kebab-case-ification untouched; chunk 4 only needs to flip the wrapping `error` value, not the per-tool fields.
- **Dispatcher already does the right thing for `Stub` and errors (chunk-3 discovery)**: `run_vectis` synthesises the kebab-case `not-implemented` envelope itself, and `emit_vectis_error` already emits kebab variants (`missing-prerequisites`, `invalid-project`, `verify`, `internal`, `io`). That means chunk 4 only needs to fix the *success-payload* literals inside `init/`, `verify/`, `add_shell/`, `update_versions/` and `VectisError::to_json` — there is no second pass required in `src/main.rs`. Once `to_json` is kebab-case it should also be wired through `emit_vectis_error` (instead of the hand-built map) so the two code paths can't drift; doing that swap is in-scope for chunk 4.
- Update every test in `crates/vectis/` (and `tests/` if it grew one) that asserts on snake_case keys. Run them to confirm coverage.
- The `emit_json` helper in `src/main.rs` already auto-injects `"schema-version": 2` on object responses, so vectis payloads inherit that for free once they go through chunk 3's dispatcher (verified live in chunk 3).
- Verify: `cargo test -p specify-vectis`; `cargo test --workspace`; manual sanity check `specify vectis init Foo --dir /tmp/v --format json | jq 'keys'` returns kebab-case keys with `schema-version`.

### Chunk 5 — Add `--format text` renderers and integration tests

Goal: round out the v2 contract by giving `--format text` (the default) a humanised view, and lock the JSON contract behind `tests/vectis.rs`.

- **Replace the chunk-3 placeholder text renderers (chunk-3 discovery)**: chunk 3 left `run_vectis` printing `serde_json::to_string_pretty(&value)` for `OutputFormat::Text` so the dispatcher works end-to-end, and `emit_vectis_error` falls back to `eprintln!("error: {err}")` for every variant including `MissingPrerequisites`. Chunk 5 owns turning both into the humanised shapes below; do not assume the text path is empty when starting.
- Extend `run_vectis` (or per-subcommand helpers) in [`src/main.rs`](../../src/main.rs) so each verb has a text renderer. Suggested shapes:
  - `vectis init`: `Created N files in <dir>. Assemblies: core PASS, ios PASS, android FAIL (cargo check)`.
  - `vectis verify`: per-assembly bullet list with first error line for failing steps.
  - `vectis add-shell`: same as init, scoped to the platform.
  - `vectis update-versions`: bullet list of `crate: old → new`; when `--verify`, append per-combo pass/fail.
- Humanise `emit_vectis_error` for the text path too — at minimum the `MissingPrerequisites` variant should list each missing tool's `tool`, `check`, and `install` (one per line) instead of the single-line `Display` impl, so operators can act on it without re-running with `--format json`.
- Add `tests/vectis.rs` covering at minimum:
  - `--help` for `specify vectis` lists four subcommands. (Verified live in chunk 3 — the four expected commands `init`, `verify`, `add-shell`, `update-versions` are present.)
  - Success JSON for `init` contains `"schema-version": 2` and kebab-case keys. **Chunk 4 has landed (verified live)**: `specify --format json vectis init Foo --dir <tmp>` (no `--shells`) returns exactly the top-level key set `["app-name", "app-struct", "assemblies", "capabilities", "project-dir", "schema-version", "shells"]`. The nested `assemblies.<name>` objects use single-word kebab-safe keys (`status`, `files`); when a shell is present its assembly object also carries `build-steps` (renamed from `build_steps`). Test should assert on this exact key set rather than spot-checking a single field, so a future regression that re-introduces a snake_case key fails loudly.
  - Error JSON for a `--version-file` pointing at a missing path emits `"error": "invalid-project"` and `"exit-code": 1`. (Verified live in chunk 3 with the chunk-3 dispatcher: `specify --format json vectis init Foo --dir /tmp/x --version-file /tmp/does-not-exist.toml` already returns exactly this shape, so this test can be authored at any time.)
  - Error JSON when prerequisites are missing emits `"error": "missing-prerequisites"`, `"exit-code": 2`. (Use a guard such as gating this test behind a `VECTIS_PREREQ_TEST` env var if making prereqs reliably absent in CI is hard — note this in the test.)
- **Test harness convention (chunk-3 discovery)**: the existing `tests/` directory uses `assert_cmd` + `tempfile` (see `Cargo.toml` `[dev-dependencies]`); reuse them. The chunk-3 dispatcher is invoked as `specify [--format json] vectis <verb> ...`, with `--format` placed before the subcommand because it is a global flag on `Cli`. Tests should follow the same ordering.
- Verify: `cargo test --workspace` green.

### Chunk 6 — Delete vectis artifacts from `../specify`

Goal: leave `../specify` with no source, no build wiring, and no committed binary that exists only to define the vectis CLI.

- `rm -rf ../specify/crates/vectis-cli` and `rm -rf ../specify/templates/vectis`. If `../specify/crates/` is empty afterwards, remove it.
- Edit `../specify/Cargo.toml`: drop the `crates/vectis-cli` workspace member. If `members` becomes empty, delete the entire `[workspace]` block (and likely the file, since `../specify` is a docs/plugins repo with no other Rust code — confirm by `rg --type rust` first).
- Edit `../specify/Makefile`: delete the `build-vectis` target. Keep `checks`, `dev-plugins`, `prod-plugins`. If the file ends up only containing those, that's fine.
- Edit `../specify/.gitignore`: remove the `/vectis` line.
- Delete the prebuilt `../specify/vectis` binary.
- Run `cargo update --workspace` (or just delete `../specify/Cargo.lock` if no Rust crates remain) so the lockfile no longer references `vectis-cli`.
- Verify: `rg -l 'vectis-cli|target/release/vectis|build-vectis|crates/vectis-cli' ../specify` returns matches inside `rfcs/rfc-6-*.md` (historical) **and** inside `plugins/vectis/skills/{template-updater,ios-writer,core-writer,android-writer}/...` — the latter set is chunk 7's responsibility, not chunk 6's. After chunk 6, the only files chunk 7 still needs to touch for these tokens are those five SKILL paths plus `references/known-drift.md`. Confirm that no Rust source, build wiring, or committed binary in `../specify` references vectis any longer.

### Chunk 7 — Update plugin & doc references in `../specify`

Goal: the `vectis` plugin and surrounding docs work against the new `specify vectis ...` invocation, and grep is clean of stale paths.

- **Exact set of remaining files (chunk-6 discovery)**: after chunk 6, a `vectis-cli|target/release/vectis|build-vectis|crates/vectis-cli` grep across `../specify` returns hits **only** in:
  - `plugins/vectis/skills/template-updater/SKILL.md`
  - `plugins/vectis/skills/template-updater/references/known-drift.md`
  - `plugins/vectis/skills/template-updater/references/playbook.md` (if present)
  - `plugins/vectis/skills/ios-writer/SKILL.md`
  - `plugins/vectis/skills/core-writer/SKILL.md`
  - `plugins/vectis/skills/android-writer/SKILL.md`
  - `rfcs/rfc-6-vectis-bootstrap.md` (banner-mark only, body intact)
  - `rfcs/rfc-6-tasks.md` (banner-mark only, body intact)

  Outside this set, `../specify` is already clean: `Cargo.toml`/`Cargo.lock` are gone (no Rust crates remain in the repo), `Makefile`'s `build-vectis` target is removed, `.gitignore` no longer carries `/vectis`, and the prebuilt binary is deleted. `docs/vectis.md`, `docs/architecture.md`, and `README.md` did **not** appear in the post-chunk-6 grep — re-check before editing them; they may need only minor wording updates rather than a full rewrite. (`docs/architecture.md` may not even exist; `glob ../specify/docs/**/*.md` to see the actual doc surface before assuming the bullets below apply verbatim.)
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
- **No JSON-key churn required (chunk-4 discovery)**: a `grep -rEn '"app_name"|"project_dir"|"build_steps"|"version_file"|"dry_run"|"detected_capabilities"|"unrecognized_capabilities"|"missing_prerequisites"|"invalid_project"|"not_implemented"' ../specify/plugins/vectis ../specify/docs ../specify/README.md` after chunk 4 returned no hits — the snake_case JSON examples only live inside `../specify/rfcs/rfc-6-*.md`, which this chunk explicitly leaves intact under a "superseded" banner. Plugin and doc rewrites are command-line and path-only.
- `../specify/docs/vectis.md`: replace any "build the vectis CLI" instructions with `cargo install --path <specify-cli> --bin specify` (or `brew install augentic/specify` if the Formula is the canonical install path) and use `specify vectis ...` throughout.
- `../specify/docs/architecture.md` and `../specify/README.md`: refresh the structure diagrams + usage snippets.
- `../specify/rfcs/rfc-6-vectis-bootstrap.md` and `../specify/rfcs/rfc-6-tasks.md`: prepend a short banner ("Status: superseded by the `specify vectis` subcommand in augentic/specify-cli; this RFC documents the original standalone-binary design.") rather than rewriting history. Leave the body intact.
- Verify, from `../specify`:
  - `rg -n '\bvectis\s+(init|verify|add-shell|update-versions)\b' | rg -v ':\s*$|specify\s+vectis'` returns nothing — i.e. every active invocation now starts with `specify vectis`.
  - `rg -n 'vectis-cli|target/release/vectis|cargo .* vectis-cli|build-vectis|crates/vectis-cli|^/vectis$' .` returns hits only in the two RFC files (now under their banner).
  - The vectis plugin's `template-updater` SKILL.md "Validate" command sequence is fully runnable as written from the `specify-cli` checkout. **chunk-5 cross-reference**: `<specify-cli>/tests/vectis.rs` already exercises `specify [--format json] vectis init Foo --dir <tmp>` end-to-end (success + invalid-project + missing-prereqs paths), so the rewritten Validate sequence in `template-updater/SKILL.md` should match the env/PATH conventions used there (notably: `--format json vectis init` works without any prior `specify init`, and `env -i HOME=$HOME ./target/debug/specify ...` with empty PATH reliably hits the `missing-prerequisites` JSON shape — no `VECTIS_PREREQ_TEST` env-var gating was needed in the end).

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
  `members` list (between `crates/platform` and end of array).
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
- **chunk-3-dispatch (completed)**: Added
  `specify-vectis = { path = "crates/vectis" }` to the root
  `[dependencies]` (between `specify-validate` and `clap`). Extended
  the top-level `Commands` enum with a `Vectis { action: VectisAction }`
  variant and defined `enum VectisAction { Init(specify_vectis::InitArgs),
  Verify(specify_vectis::VerifyArgs), AddShell(specify_vectis::AddShellArgs),
  UpdateVersions(specify_vectis::UpdateVersionsArgs) }`. Added
  `Commands::Vectis { action } => run_vectis(cli.format, &action)` to
  the `run()` match. Implemented `run_vectis(format, &VectisAction)` and
  `emit_vectis_error(format, &VectisError)` immediately above
  `absolute_string` near the bottom of `src/main.rs`. The dispatcher
  matches the four arg variants, calls
  `specify_vectis::{init,verify,add_shell,update_versions}::run`, and:
  - On `Ok(CommandOutcome::Success(value))` calls `emit_json(value)` for
    JSON (which auto-injects `schema-version: 2`) and pretty-prints the
    JSON for the text path as a placeholder until chunk 5 lands the
    humanised renderers; returns `EXIT_SUCCESS`.
  - On `Ok(CommandOutcome::Stub { command })` synthesises a kebab-case
    `not-implemented` envelope locally (no library round-trip) and
    returns `EXIT_GENERIC_FAILURE`.
  - On `Err(VectisError)` routes through `emit_vectis_error`, which
    builds the JSON object by hand with kebab-case `error` variants
    (`missing-prerequisites`, `invalid-project`, `verify`, `internal`,
    `io`), attaches `message` + `exit-code`, includes the `missing`
    array for `MissingPrerequisites`, and maps the exit code to
    `EXIT_VALIDATION_FAILED` (`2`) for `MissingPrerequisites` and
    `EXIT_GENERIC_FAILURE` (`1`) otherwise. The `Vectis` variant's
    rustdoc on the `Commands` enum documents the `EXIT_VALIDATION_FAILED`
    reuse rationale.
  Verification: `cargo build -p specify` is green;
  `cargo test --workspace` runs all 487+ tests across 34 binaries with
  zero failures; `cargo clippy -p specify --all-targets` is clean;
  `./target/debug/specify vectis --help` lists the four subcommands;
  `specify --format json vectis init Foo --dir <tmp>` runs end-to-end
  and emits a JSON envelope containing `schema-version: 2` plus the
  legacy snake_case payload keys (`app_name`, `app_struct`,
  `project_dir` — chunk 4's job to rename); `specify --format json
  vectis init Foo --dir /tmp/x --version-file /tmp/does-not-exist.toml`
  exits 1 with `{ "error": "invalid-project", "exit-code": 1, "message":
  "...", "schema-version": 2 }` exactly as chunk 5's test plan expects.
- **chunk-4-v2 (completed)**: Rewrote every `serde_json::json!` and
  `serde_json::Map::insert` literal in `crates/vectis/src/{init,verify,
  add_shell,update_versions}/*.rs` and `error.rs` to kebab-case keys
  and kebab-case error variants. Concrete renames:
  - `init/mod.rs` (success payload): `app_name` → `app-name`,
    `app_struct` → `app-struct`, `project_dir` → `project-dir`; per-shell
    `assemblies.{ios,android}.build_steps` → `build-steps`.
  - `verify/mod.rs`: top-level `project_dir` → `project-dir`. The nested
    `assemblies.<name>.{passed, steps}` and `BuildStep`'s
    `{name, passed, error}` were already kebab-safe (single-word).
  - `add_shell/mod.rs`: `app_name` → `app-name`, `project_dir` →
    `project-dir`, `detected_capabilities` → `detected-capabilities`,
    `unrecognized_capabilities` → `unrecognized-capabilities`; the
    inner `assembly.build_steps` insert key flipped to `build-steps`.
  - `update_versions/mod.rs`: `version_file` → `version-file`,
    `dry_run` → `dry-run`. The other top-level keys (`verify`,
    `passed`, `written`, `changes`, `unchanged`, `errors`,
    `verification`) and the `Change`/`Unchanged`/`ComboResult`/
    `MatrixResult` struct fields (`key`, `current`, `proposed`,
    `value`, `caps`, `passed`, `combos`, `error`, `verify`) are
    single-word and survived untouched. `update_versions/matrix.rs`
    needed no edits (the per-combo `ComboResult` is `Serialize`-derived
    on single-word fields and the `value.get("passed")` lookup against
    the verify JSON is a single-word kebab-safe key).
  - `error.rs` `VectisError::to_json`: `error` value flipped to kebab
    (`missing-prerequisites`, `io`, `invalid-project`, `verify`,
    `internal`); the `MissingTool` payload (`tool`, `assembly`, `check`,
    `install`) is single-word and survives untouched. Added a new
    `pub fn variant_str(&self) -> &'static str` so the kebab variant
    string is a single source of truth (`to_json` calls it). The two
    `error::tests` JSON-shape assertions were flipped from
    `missing_prerequisites` / `invalid_project` to the kebab forms.
    Also fixed the `prerequisites.rs` doc-comment reference to
    `missing_prerequisites` → `missing-prerequisites` so grep stays
    clean of the snake form.
  - `src/main.rs`: `emit_vectis_error`'s hand-built JSON map was
    replaced with a thin wrapper over `VectisError::to_json` that
    splices `exit-code` on top via `entry().or_insert(...)`. This is
    the "wire `to_json` through `emit_vectis_error` so the two code
    paths can't drift" step the chunk plan called out — there is now
    a single kebab-variant table (`VectisError::variant_str`) used by
    both the dispatcher and any future direct caller of `to_json`.
  Verification (all green): `cargo build -p specify-vectis`,
  `cargo test -p specify-vectis` (118 passed), `cargo clippy -p
  specify-vectis -p specify --all-targets`, `cargo test --workspace`
  (all targets, 0 failed). Manual sanity:
  `specify --format json vectis init Foo --dir <tmp>` returns the
  exact top-level key set
  `["app-name", "app-struct", "assemblies", "capabilities",
  "project-dir", "schema-version", "shells"]` with `schema-version: 2`
  auto-injected by `emit_json`. Manual error sanity:
  `specify --format json vectis init Foo --dir <tmp>
  --version-file /tmp/does-not-exist.toml` exits 1 with
  `{"error": "invalid-project", "exit-code": 1, "message":
  "version file not found: ...", "schema-version": 2}` — exact shape
  chunk 5's test plan asserts.
  **Forward impact**: chunk 5 may now author the JSON-shape
  assertions immediately (no further blockers); the plan's chunk-5
  "Success JSON for init" bullet has been updated to spell out the
  exact key set so the test does not have to spot-check. Chunk 7's
  plugin/docs rewrite has no JSON-key churn — a grep across
  `../specify/{plugins/vectis,docs,README.md}` for the old snake_case
  keys returned no hits; the only snake_case JSON examples live in
  `../specify/rfcs/rfc-6-*.md`, which chunk 7 explicitly leaves
  intact under a "superseded" banner. The chunk-5 humanised text
  renderer for `MissingPrerequisites` should pull `tool`, `check`,
  `install` directly off `VectisError::missing` rather than off
  `to_json`'s payload, since the `MissingTool` struct is `pub` and
  reachable as `specify_vectis::MissingTool` from the dispatcher.
- **chunk-5-text-tests (completed)**: Replaced `run_vectis`'s
  text-format placeholder (which pretty-printed the JSON `Value`) with
  a per-verb dispatcher `vectis_render_text(action, value)` and four
  free functions (`vectis_render_init_text`,
  `vectis_render_verify_text`, `vectis_render_add_shell_text`,
  `vectis_render_update_versions_text`) plus a small
  `vectis_render_build_steps_summary` helper that renders an init /
  add-shell `build-steps` array as `build PASS` or
  `build FAIL (<first failing step name>)`. Renderers consume the v2
  JSON shape directly via defensive `as_*`/`get` chains rather than
  re-threading the typed library results — this keeps the dispatcher
  in lock-step with the JSON contract by construction and avoids
  having to plumb the four success types through `run_vectis`. The
  renderers preserve a stable assembly order (`core`, `ios`,
  `android`, then anything else alphabetically) so the text output
  matches the visual order users see in the JSON envelope. Output
  shapes (verified live):
  - `init`: `Created app "Foo" at <dir>` / `Capabilities: <list> | (none)` /
    `Assemblies:` then `  - <name>: <status> (N files)` plus
    `, build PASS|FAIL (<step>)` when a `build-steps` array is present.
  - `verify`: `Verified <dir>: PASS|FAIL` then `  - <name>: PASS|FAIL`,
    expanding failing assemblies into per-step indented lines with
    the first non-empty line of `step.error` as `error: <line>`.
  - `add-shell`: `Added <platform> shell to "<app>" at <dir>`,
    `Detected capabilities: ...`, optional `Unrecognized
    capabilities: ...`, `Files: N` plus `, build PASS|FAIL (<step>)`.
  - `update-versions`: `Versions file: <path> (dry-run|written|no
    write)`, then either `Changes:` with `  - <key>: <cur> →
    <prop>` lines or `No changes.`; optional `Errors:` block; optional
    `Verify matrix: PASS|FAIL` with per-combo `  - <caps>: PASS|FAIL`.
  Humanised `emit_vectis_error`'s text path: `MissingPrerequisites`
  now prints `error: missing prerequisites` followed by one indented
  line per missing tool (`  - <tool> (<assembly>): <check> | install:
  <install>`) and the trailing message — operators can act on it
  without re-running with `--format json`. Other variants keep the
  one-line `error: {err}` shape (which already includes the variant
  prefix via `Display`). Also refreshed the rustdoc on
  `Commands::Vectis` (it still claimed success payloads carried legacy
  snake_case keys "until chunk 4"; chunk 4 has landed). Authored
  `tests/vectis.rs` covering the four cases the plan called for:
  - `vectis_help_lists_four_subcommands` — asserts `init`, `verify`,
    `add-shell`, `update-versions` all appear under
    `specify vectis --help`.
  - `init_success_json_has_kebab_keys_and_schema_version` — asserts
    the exact top-level key set
    `["app-name", "app-struct", "assemblies", "capabilities",
    "project-dir", "schema-version", "shells"]`, plus
    `schema-version == 2`, `app-name == "Foo"`, `app-struct == "Foo"`,
    `project-dir` canonicalises to the tempdir, and `assemblies.core`
    has `status == "created"` with a `files` array. Soft-skips with
    a `eprintln!` when the host returns `missing-prerequisites` so
    the suite stays green on CI hosts without `rustup`/`cargo-deny`/
    `cargo-vet`.
  - `init_invalid_project_json_shape` — points `--version-file` at a
    non-existent path; asserts `error == "invalid-project"`,
    `exit-code == 1`, `schema-version == 2`, message contains
    `"version file not found"`, and the process exit code is `1`.
    Independent of workstation toolchain so it runs unconditionally.
  - `init_missing_prereqs_json_shape` — sets `PATH=""` (and clears
    `CARGO_HOME`/`RUSTUP_HOME`) so every `Command::new("rustup")`
    etc. lookup fails with ENOENT, forcing the
    `MissingPrerequisites` path; asserts
    `error == "missing-prerequisites"`, `exit-code == 2`,
    `schema-version == 2`, `missing` array is non-empty, and each
    `missing[i]` has the four kebab-safe single-word fields (`tool`,
    `assembly`, `check`, `install`). The plan's suggested
    `VECTIS_PREREQ_TEST` env-var gating turned out unnecessary —
    `PATH=""` reliably forces the failure on every host. Cleared
    `CARGO_HOME`/`RUSTUP_HOME` defensively because rustup's shim can
    sometimes resolve via those env vars without consulting PATH;
    leaving them set produced no observed false negative on the
    development host but the explicit removal is cheap insurance.
  Verification (all green): `cargo build -p specify`, `cargo test -p
  specify --test vectis` (4 passed), `cargo test --workspace`
  (487+ tests across all binaries, 0 failures), `cargo clippy -p
  specify -p specify-vectis --all-targets -- -D warnings` clean.
  **Forward impact**:
  - chunk 6 (delete `../specify` artifacts) is unaffected — chunk 5
    was entirely additive on the `specify-cli` side.
  - chunk 7 (plugin/docs rewrite) gains a runnable reference for the
    `template-updater` plugin's "Validate" command sequence:
    `tests/vectis.rs` shows the exact `assert_cmd` invocations,
    env-var conventions (notably the `PATH=""` trick for forcing
    `missing-prerequisites` deterministically), and JSON-shape
    expectations the plugin's docs should match. The chunk-7 verify
    bullet was extended to call this out.
  - The dispatcher in `src/main.rs` no longer has any chunk-3
    placeholder pretty-print fallback to clean up. The text path is
    now the public face of `specify vectis ...` and matches the
    shapes the plugin's docs (chunk 7) will reference.
- **chunk-6-clean-specify (completed)**: From `../specify` (branch
  `move-vectis`):
  - `rm -rf crates/vectis-cli templates/vectis` and removed the
    now-empty parent dirs (`crates/`, `templates/`) since no other
    siblings remained.
  - Deleted the prebuilt `vectis` binary (untracked — it lived only on
    disk thanks to the `.gitignore` `/vectis` entry, never committed).
  - Deleted `Cargo.toml` and `Cargo.lock` outright. The workspace
    manifest only declared `members = ["crates/vectis-cli"]` and a
    `glob ../specify/**/Cargo.toml` confirmed no other Rust crates
    exist anywhere in the repo (the `*.rs` files left under
    `plugins/spec/skills/plan/fixtures/discovery/legacy/src/` and
    under what *was* `templates/vectis/core/` were not part of any
    `Cargo.toml` — the latter set is now gone, the former is fixture
    data not built by Cargo). With the workspace gone there is no
    lockfile to maintain. Both deletions were the chunk-plan's
    "delete the entire `[workspace]` block (and likely the file)"
    branch.
  - Rewrote `Makefile`: dropped the `.PHONY: build-vectis` declaration
    and the `build-vectis: cargo build --release --package vectis-cli
    && cp target/release/vectis .` recipe. Kept `checks`,
    `dev-plugins`, `prod-plugins` exactly as they were.
  - Edited `.gitignore`: removed the `/vectis` line. Left the
    pre-existing `/target` line in place — the `target/` dir on disk
    is stale post-deletion but harmless and untracked, and removing
    `/target` was outside the chunk's scope (operator can `rm -rf
    target/` at leisure).
  - Did not touch any of the eight files listed in chunk 7's "exact
    set of remaining files" subsection (the four `plugins/vectis/skills`
    SKILL.md files plus `template-updater/references/known-drift.md`,
    plus the two `rfcs/rfc-6-*.md` files); chunk 7 owns those.
  Verification:
  - `git status` after the chunk lists: modified `.gitignore`, deleted
    `Cargo.lock`, deleted `Cargo.toml`, modified `Makefile`, plus the
    full set of deleted `crates/vectis-cli/**` and `templates/vectis/**`
    paths. No untracked files added.
  - `grep 'vectis-cli|target/release/vectis|build-vectis|crates/vectis-cli'
    ../specify` returns hits only in
    `rfcs/rfc-6-{vectis-bootstrap,tasks}.md` (historical, intentionally
    preserved) and in
    `plugins/vectis/skills/{template-updater/{SKILL.md,references/known-drift.md},
    {ios,core,android}-writer/SKILL.md}` (chunk 7's responsibility).
    Every other file in the repo is now clean of these tokens.
  - `grep 'vectis (init|verify|add-shell|update-versions)|target/release/vectis|cargo .* vectis|crates/vectis|build-vectis|templates/vectis|\\./vectis'`
    against `../specify/docs/` and `../specify/README.md` returns no
    matches — neither file references the CLI binary or its build
    commands, only the conceptual `vectis` plugin/schema.
  **Plan deviations / forward impact**:
  - The chunk's verification bullet originally claimed "matches **only**
    inside `rfcs/rfc-6-*.md`" — that was optimistic; it overlooked
    `plugins/vectis/skills/...` references, which are chunk 7's
    responsibility. The chunk-6 verification bullet has been rewritten
    to acknowledge this and the chunk-7 section now opens with the
    exact list of files that still need editing (saving the next
    agent a grep round-trip).
  - Chunk 7 currently lists `docs/architecture.md`, `docs/vectis.md`,
    and `README.md` as needing structure-diagram / usage-snippet
    refreshes. A post-chunk-6 grep against those three files turned
    up zero `vectis-cli` / build-command / path mentions; their
    `vectis` references are all conceptual (plugin schema URL, plugin
    description, doc cross-link). The chunk-7 plan note has been
    softened accordingly: the next agent should re-grep before
    assuming edits are required.
  - The `target/` dir under `../specify` is now stale (no Rust crates
    to populate it) but is `.gitignore`d so it's not in `git status`.
    Deleting it is outside chunk 6/7 scope; flagged here for awareness.
- **chunk-7-plugin-docs (completed)**: Edited the eight files chunk 6
  flagged, confined to `../specify` (branch `move-vectis`):
  - `plugins/vectis/skills/template-updater/SKILL.md`: every
    `./target/release/vectis ...` and bare `vectis <verb>` invocation
    rewritten to `{repo-dir}/target/debug/specify --format json vectis
    <verb> ...` (the `--format json` flag is global and must precede the
    `vectis` subcommand — chunk 3 wired it on `Cli`, chunk 5's
    `tests/vectis.rs` uses the same ordering, so the SKILL stays in
    lock-step). All `crates/vectis-cli/...` paths repointed at
    `<specify-cli>/crates/vectis/...`; all `templates/vectis/...`
    references repointed at `<specify-cli>/templates/vectis/...`.
    `cargo build --release -p vectis-cli` was replaced with `cargo build
    -p specify` (the `specify` binary is what the SKILL needs to run end
    to end), while `cargo test -p vectis-cli` and `cargo clippy ... -p
    vectis-cli` became `-p specify-vectis` (the library carve-out from
    chunk 2 — same crate, narrower scope). The "prerequisites" section
    now spells out that `{repo-dir}` refers to the `specify-cli`
    checkout, not `../specify`. Reference table at the bottom lists
    paths under `<specify-cli>/crates/vectis/` and
    `<specify-cli>/templates/vectis/`.
  - `plugins/vectis/skills/template-updater/references/known-drift.md`:
    same path repointing (`crates/vectis-cli/embedded/versions.toml` →
    `<specify-cli>/crates/vectis/embedded/versions.toml`,
    `crates/vectis-cli/src/update_versions/query.rs` →
    `<specify-cli>/crates/vectis/src/update_versions/query.rs`); every
    `vectis update-versions ...` invocation became `specify vectis
    update-versions ...`.
  - `plugins/vectis/skills/{ios,core,android}-writer/SKILL.md`: every
    `vectis init`, `vectis verify`, and `vectis add-shell {ios,android}`
    invocation became `specify vectis ...`. Embedded-template path
    references (`crates/vectis-cli/embedded/`,
    `crates/vectis-cli/src/...`) now point at
    `<specify-cli>/crates/vectis/embedded/` and
    `<specify-cli>/crates/vectis/src/...`; every
    `templates/vectis/{core,ios,android}/...` reference picked up the
    `<specify-cli>/` prefix.
  - `rfcs/rfc-6-vectis-bootstrap.md` and `rfcs/rfc-6-tasks.md`:
    prepended a four-line "Status: superseded" banner referencing
    `augentic/specify-cli` (`crates/vectis/` library +
    `templates/vectis/`) and pointing readers at the four
    `plugins/vectis/skills/...` SKILLs for the current invocation, paths,
    and JSON contract. Bodies were not rewritten — both files are
    preserved verbatim as historical record.
  - `rfcs/roadmap.md`: the RFC-6 entry already says "Status: " for every
    other RFC; I added a `**Status:** Superseded — folded into the
    `specify` CLI ...` line to its RFC-6 paragraph and rewrote its
    "Solution" sentence so the four bullet examples use `specify vectis
    init|verify|add-shell|update-versions` (instead of bare `vectis ...`).
    This was outside chunk 7's original file list but came up in the
    verification grep — leaving the paragraph's bare invocations in
    place would have left a non-banner-marked file failing the
    `\bvectis\s+(init|verify|add-shell|update-versions)\b` rule. Banner
    treatment matches the two `rfc-6-*.md` files (history preserved,
    superseded status flagged).
  - **No edits required** to `docs/vectis.md`, `docs/architecture.md`,
    or `README.md` — chunk 6's note already flagged them as
    conceptually-only references; a re-grep at the start of chunk 7
    confirmed zero `vectis-cli` / `target/release/vectis` /
    `cargo .* vectis-cli` / `crates/vectis-cli` /
    `templates/vectis/{core,ios,android}/` / build-command mentions in
    those files. The pre-existing `vectis` mentions are all conceptual
    (plugin schema URL, plugin description, doc cross-link) and remain
    accurate under the folded layout.

  Verification (both grep suites from chunk 7's plan section, run from
  `../specify`):
  - `grep -rEn '\bvectis[[:space:]]+(init|verify|add-shell|update-versions)\b' --include='*.md' . | grep -vE 'specify[[:space:]]+(--[a-z]+([[:space:]]+[^[:space:]]+)?[[:space:]]+)?vectis|specify$|:[[:space:]]*$|^\./rfcs/rfc-6-'`
    returns a single match —
    `plugins/vectis/skills/template-updater/SKILL.md:190` — which is the
    tail of a hard-wrapped `specify\nvectis update-versions --verify`
    sentence (the word "specify" sits at the end of line 189). The
    plan's published filter `rg -v 'specify\s+vectis'` is too narrow to
    catch `specify --format json vectis ...` (the SKILL's actual
    invocation shape) or this line wrap; semantically the file is clean.
    All other `vectis (verb)` hits live inside `rfcs/rfc-6-*.md` (under
    their banners) and inside the rewritten `roadmap.md` paragraph
    (`specify vectis init`, etc., which the filter correctly skips).
  - `grep -rEn 'vectis-cli|target/release/vectis|cargo .* vectis-cli|build-vectis|crates/vectis-cli|^/vectis$' --include='*.md' --include='Makefile' --include='*.toml' --include='*.gitignore' .`
    returns hits **only** inside `rfcs/rfc-6-vectis-bootstrap.md` and
    `rfcs/rfc-6-tasks.md` (now under their "superseded" banners). Every
    other file in the repo — including the four rewritten SKILL.md
    files, the rewritten `known-drift.md`, `roadmap.md`, `Makefile`, and
    `.gitignore` — is clean of these tokens.

  **Plan deviations / forward impact**:
  - `rfcs/roadmap.md` was not in the chunk's "exact set of remaining
    files" list (chunk 6's grep only inspected
    `vectis-cli|target/release/vectis|build-vectis|crates/vectis-cli`,
    none of which `roadmap.md` carried). Chunk 7's
    `\bvectis\s+(init|verify|add-shell|update-versions)\b` grep
    surfaced its RFC-6 paragraph as a fresh hit. The fix was a
    surgical paragraph-level rewrite + status banner; the broader
    roadmap structure was left intact. Future chunks adding new
    superseded RFCs should remember to either banner-mark the roadmap
    paragraph or update its example invocations alongside the RFC body.
  - The published verification regex (`rg -v ':\s*$|specify\s+vectis'`)
    is too tight to catch the SKILL files' `specify --format json
    vectis` invocations and any `specify\nvectis` line wraps. A more
    accurate filter is `grep -vE 'specify[[:space:]]+(--[a-z]+([[:space:]]+[^[:space:]]+)?[[:space:]]+)?vectis|specify$|:[[:space:]]*$'`,
    used here. Future "fold X into specify" plans that allow `--global
    flag` between `specify` and the subcommand should publish the
    looser filter from the start.
  - The plan is now end-to-end complete; no follow-up chunk is needed.
    The two repos are independently buildable: `specify-cli` produces
    the `specify` binary that satisfies every step the rewritten
    plugin SKILLs reference (verified via chunk 5's `tests/vectis.rs`),
    and `../specify` carries no Rust source, no build wiring, no
    committed binary, and no stale `vectis-cli` references outside the
    explicitly-banner-marked RFCs. If a future agent wants to clean up
    the stale `../specify/target/` directory left behind by chunk 6, it
    can `rm -rf target/` from that checkout — out of scope for this
    plan.
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
