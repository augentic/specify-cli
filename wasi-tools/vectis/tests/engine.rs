//! Integration tests for the vectis validation engine, split by concern
//! (paths, layout, tokens, assets, composition). Shared fixtures and
//! assertion helpers live in [`engine_support`].

mod engine_support;

#[path = "engine/assets.rs"]
mod assets;
#[path = "engine/composition.rs"]
mod composition;
#[path = "engine/infer.rs"]
mod infer;
#[path = "engine/layout.rs"]
mod layout;
#[path = "engine/materialize.rs"]
mod materialize;
#[path = "engine/materialize_illustrations.rs"]
mod materialize_illustrations;
#[path = "engine/materialize_app_icon.rs"]
mod materialize_app_icon;
#[path = "engine/materialize_acceptance_fixture.rs"]
mod materialize_acceptance_fixture;
#[path = "engine/paths.rs"]
mod paths;
#[path = "engine/tokens.rs"]
mod tokens;
