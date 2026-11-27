//! The Flatpak build runs sandboxed under its own application ID: the window
//! must carry that ID so the desktop finds its icon and launcher, and features
//! that change the host system (the link handler) or reach host programs
//! (compositor IPC) are left out.
use std::sync::OnceLock;

/// The Flatpak application ID, shared with `flatpak/` and Flathub.
pub const ID: &str = "io.github.eml3ss.VkFast";

/// Whether this process runs inside a Flatpak sandbox. Tests never do.
pub fn active() -> bool {
    static ACTIVE: OnceLock<bool> = OnceLock::new();
    !cfg!(test) && *ACTIVE.get_or_init(|| std::path::Path::new("/.flatpak-info").is_file())
}
