// Views are split `impl AppState` blocks, one file per page/surface.
// Each file is a pure cut-and-paste move out of app.rs (behavior unchanged).
pub(crate) mod chrome;
pub(crate) mod config;
pub(crate) mod connections;
pub(crate) mod groups;
pub(crate) mod home;
pub(crate) mod logs;
pub(crate) mod nodes;
pub(crate) mod palette;
pub(crate) mod rules;
pub(crate) mod settings;
