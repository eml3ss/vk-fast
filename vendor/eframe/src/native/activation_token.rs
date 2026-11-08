//! VK Fast patch: give the next native window the activation token of the
//! application that launched a second VK Fast process.
//!
//! On Wayland only a launcher's token lets the compositor focus a window; a
//! window's own request is treated as an attention hint. winit applies such
//! a token when a window is created, through its safe startup-notify API.

use std::sync::Mutex;

static PENDING: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set(token: String) {
    *PENDING.lock().unwrap_or_else(|error| error.into_inner()) = Some(token);
}

/// Attach and consume a pending token.
pub(crate) fn apply(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::{platform::startup_notify::WindowAttributesExtStartupNotify, window::ActivationToken};
    let token = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    match token {
        Some(token) => attributes.with_activation_token(ActivationToken::from_raw(token)),
        None => attributes,
    }
}
