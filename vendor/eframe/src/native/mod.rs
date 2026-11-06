mod app_icon;
mod epi_integration;
mod event_loop_context;
mod first_frame;
pub mod run;
mod ui_frame_clock;
#[cfg(any(target_os = "windows", test))]
pub(crate) mod windows_focus;
#[cfg(target_os = "windows")]
pub(crate) mod windows_menu;
#[cfg(any(target_os = "windows", test))]
mod windows_startup;

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// File storage which can be used by native backends.
#[cfg(feature = "persistence")]
pub mod file_storage;

pub(crate) mod winit_integration;

#[cfg(feature = "glow")]
mod glow_integration;

#[cfg(feature = "wgpu_no_default_features")]
mod wgpu_integration;
