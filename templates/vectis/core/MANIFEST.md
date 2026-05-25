# Core Assembly Template Manifest

Human reference for the core assembly templates. The canonical source-to-target
registry is [`../manifest.yaml`](../manifest.yaml) (`assemblies.core`); `wasi-tools/vectis/build.rs` validates that manifest and emits the embedded `registry.rs` consumed by `specrun tool run vectis -- scaffold core`.

Source filenames are flat under `templates/vectis/core/`. Nested target paths (e.g. `shared/src/bin/codegen.rs`) are declared in `manifest.yaml`, not mirrored in this directory layout. The `gitignore` source has no leading dot because shipping a literal `.gitignore` inside the `templates/` tree would be silently honoured by git tools that walk the repo; the engine renames it on write.

Total: 13 files (matches RFC § File Manifests § Core Assembly).

## Placeholder reference

Render-only baseline -- always present:

| Placeholder              | Example value | Files                            |
| ------------------------ | ------------- | -------------------------------- |
| `__APP_NAME__`           | `Counter`     | `app.rs`                         |
| `__APP_STRUCT__`         | `Counter`     | `app.rs`, `ffi.rs`, `codegen.rs` |
| `__CRUX_CORE_VERSION__`  | `0.17.0`      | `workspace-cargo.toml`           |
| `__FACET_VERSION__`      | `=0.31`       | `workspace-cargo.toml`           |
| `__SERDE_VERSION__`      | `1.0`         | `workspace-cargo.toml`           |
| `__UNIFFI_VERSION__`     | `=0.29.4`     | `shared-cargo.toml`              |

Capability-version placeholders -- only meaningful when their CAP marker is kept
(see "Cap-marker reference" below):

| Placeholder                  | Example value | Files                  |
| ---------------------------- | ------------- | ---------------------- |
| `__CRUX_HTTP_VERSION__`      | `0.16.0`      | `workspace-cargo.toml` |
| `__CRUX_KV_VERSION__`        | `0.11.0`      | `workspace-cargo.toml` |
| `__CRUX_TIME_VERSION__`      | `0.15.0`      | `workspace-cargo.toml` |
| `__CRUX_PLATFORM_VERSION__`  | `0.8.0`       | `workspace-cargo.toml` |

Android-only placeholder -- referenced by the core because the codegen binary
emits Kotlin types into the package the Android shell expects:

| Placeholder              | Example value         | Files         |
| ------------------------ | --------------------- | ------------- |
| `__ANDROID_PACKAGE__`    | `com.vectis.counter`  | `codegen.rs`  |

The placeholder defaults to `com.vectis.<lower app name>` per RFC § CLI Surface §
`vectis init` -- the codegen binary still compiles for core-only and iOS-only
projects (Kotlin codegen just isn't wired up in those layouts).

## Cap-marker reference

Capability-conditional regions are wrapped with paired `<<<CAP:<name>` and
`CAP:<name>>>>` markers, each on their own line. The chunk-5 engine treats the
entire region (markers and content inclusive) as removable when the cap is not
selected. The render-only verification in this chunk strips them with a single
sed command.

| Cap        | Files                                       |
| ---------- | ------------------------------------------- |
| `http`     | `workspace-cargo.toml`, `shared-cargo.toml`, `app.rs` |
| `kv`       | `workspace-cargo.toml`, `shared-cargo.toml`, `app.rs` |
| `time`     | `workspace-cargo.toml`, `shared-cargo.toml`, `app.rs` |
| `platform` | `workspace-cargo.toml`, `shared-cargo.toml`, `app.rs` |
| `sse`      | `shared-cargo.toml`                         |

Notes for chunk 5/6:

- Marker open/close lines do not nest. Every `<<<CAP:foo` must be paired with the
  next `CAP:foo>>>` on its own line.
- The engine should drop both markers and content when the cap is absent, and
  drop only the marker lines (leaving content) when the cap is present.
- Indentation inside markers is preserved verbatim. A few markers in
  `shared-cargo.toml` deliberately sit inside an array literal so their content
  becomes inline list elements when retained -- the engine must not normalise
  whitespace inside or around marker lines.
- The `sse` cap appears only in `shared-cargo.toml` today. Chunk 6 decided
  **not** to add an `Sse(...)` Effect variant to `app.rs`: doing so would
  cascade into chunks 7/8 (matching `<<<CAP:sse` blocks in `Core.swift`,
  `Core.kt`, `libs.versions.toml`, `AndroidManifest.xml`) for no observable
  benefit on the render-only baseline. Writer skills (chunk 12) can
  introduce `Effect::Sse(...)` later if a real sse app needs it; if and
  when that happens, add the marker here and bump this manifest.

## Self-check

Orphan detection and file-count parity (13 files) run in `wasi-tools/vectis/build.rs` when the crate builds. After adding or renaming a template file, update [`../manifest.yaml`](../manifest.yaml) in the same change.
