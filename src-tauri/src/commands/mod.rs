//! Tauri IPC facade.
//!
//! Command handlers stay deliberately thin: they validate/translate the wire
//! request and delegate to an `AppState` service. Keeping handlers grouped by
//! domain prevents native startup and registration from becoming another state
//! coordinator.

mod ai;
mod app;
mod documents;
mod settings;
mod workspace;

pub(crate) use ai::*;
pub(crate) use app::*;
pub(crate) use documents::*;
pub(crate) use settings::*;
pub(crate) use workspace::*;
