//! Bring this process's existing window forward on tiling Wayland compositors.
//!
//! Wayland ignores a window's own focus request, and re-creating the surface
//! makes a tiling compositor place it again. niri, Hyprland and sway focus an
//! existing window by its process through their IPC instead (as Spotifast
//! does on niri). Other desktops use an xdg-activation token, see
//! `eframe::set_wayland_activation_token`.
use std::process::{Command, Output, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compositor {
    Niri,
    Hyprland,
    Sway,
}

fn set(variable: &str) -> bool {
    std::env::var_os(variable).is_some_and(|value| !value.is_empty())
}

/// The running compositor with a focus IPC. Tests never talk to the user's
/// compositor.
pub fn compositor() -> Option<Compositor> {
    if cfg!(test) {
        None
    } else if set("NIRI_SOCKET") {
        Some(Compositor::Niri)
    } else if set("HYPRLAND_INSTANCE_SIGNATURE") {
        Some(Compositor::Hyprland)
    } else if set("SWAYSOCK") {
        Some(Compositor::Sway)
    } else {
        None
    }
}

fn run(program: &str, args: &[&str]) -> Option<Output> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
}

/// Focus the window owned by `pid`. Spawns the compositor's client; call off
/// the UI thread.
pub fn focus(compositor: Compositor, pid: u32) -> bool {
    match compositor {
        Compositor::Niri => run("niri", &["msg", "--json", "windows"])
            .and_then(|output| niri_window_id(&output.stdout, pid))
            .is_some_and(|id| {
                run(
                    "niri",
                    &["msg", "action", "focus-window", "--id", &id.to_string()],
                )
                .is_some()
            }),
        Compositor::Hyprland => run(
            "hyprctl",
            &["dispatch", "focuswindow", &format!("pid:{pid}")],
        )
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "ok"),
        Compositor::Sway => run("swaymsg", &[&format!("[pid={pid}]"), "focus"]).is_some(),
    }
}

/// The id of the first window of `pid` in `niri msg --json windows`.
fn niri_window_id(json: &[u8], pid: u32) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(json)
        .ok()?
        .as_array()?
        .iter()
        .find(|window| window["pid"].as_u64() == Some(u64::from(pid)))?["id"]
        .as_u64()
}

/// A launcher's `XDG_ACTIVATION_TOKEN`: a short printable string, never a
/// path or a command.
pub fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 256 && token.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_niri_window_of_this_process_only() {
        let json = br#"[{"id":3,"pid":10,"app_id":"other"},{"id":7,"pid":42,"app_id":"vk-fast"},{"id":9,"pid":42}]"#;
        assert_eq!(niri_window_id(json, 42), Some(7));
        assert_eq!(niri_window_id(json, 11), None);
        assert_eq!(niri_window_id(b"not json", 42), None);
        assert_eq!(niri_window_id(br#"{"id":1,"pid":42}"#, 42), None);
        assert_eq!(compositor(), None, "tests never focus real windows");
        assert!(valid_token("niri-123_abc:42"));
        assert!(!valid_token(""));
        assert!(!valid_token("a b"));
        assert!(!valid_token(&"a".repeat(257)));
    }
}
