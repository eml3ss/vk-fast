//! Ctrl + the keyboard's volume keys change VK Fast's own volume, anywhere,
//! with the window in the background too. Only those two combinations are
//! taken from the system, so the system volume does not change with them;
//! the volume keys alone keep working as before. Windows registers them as
//! system hotkeys; an X11 session grabs them on the root window. A Wayland
//! session lets no application take keys globally, so it is not offered.
use std::sync::{
    Mutex,
    atomic::{AtomicI32, Ordering},
};

use crate::repaint_target::RepaintTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Available,
    /// A Wayland session: no global keys for applications.
    Wayland,
    /// Neither Windows nor an X11 session.
    Unsupported,
}

/// The volume change of one press.
pub const STEP: f32 = 0.05;

/// Presses not yet applied: + for louder, - for quieter.
static PENDING: AtomicI32 = AtomicI32::new(0);
/// Wakes the application when a press arrives.
static WAKE: Mutex<Option<RepaintTarget>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// Tests choose the platform instead of reading the real session.
    pub static TEST_SUPPORT: std::cell::Cell<Support> =
        const { std::cell::Cell::new(Support::Available) };
}

pub fn support() -> Support {
    #[cfg(test)]
    return TEST_SUPPORT.with(std::cell::Cell::get);
    #[cfg(not(test))]
    support_in(cfg!(windows), cfg!(target_os = "linux"), |name| {
        std::env::var(name).ok()
    })
}

/// Windows always offers global keys. On Linux an X11 session does; a
/// Wayland one never does, also for an application running under XWayland,
/// whose grabs only see keys while an X11 window has focus.
fn support_in(windows: bool, linux: bool, env: impl Fn(&str) -> Option<String>) -> Support {
    if windows {
        return Support::Available;
    }
    if !linux {
        return Support::Unsupported;
    }
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    let wayland = set("WAYLAND_DISPLAY")
        || env("XDG_SESSION_TYPE").is_some_and(|kind| kind.eq_ignore_ascii_case("wayland"));
    if wayland {
        Support::Wayland
    } else if set("DISPLAY") {
        Support::Available
    } else {
        Support::Unsupported
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub enum Error {
    /// Another program already holds the combinations.
    Taken,
    Failed,
}

#[cfg(all(any(windows, target_os = "linux"), not(test)))]
mod native {
    use std::sync::Once;

    use global_hotkey::{
        GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
        hotkey::{Code, HotKey, Modifiers},
    };

    use super::{Error, PENDING, WAKE};

    fn keys() -> [HotKey; 2] {
        [
            HotKey::new(Some(Modifiers::CONTROL), Code::AudioVolumeUp),
            HotKey::new(Some(Modifiers::CONTROL), Code::AudioVolumeDown),
        ]
    }

    pub struct Active {
        manager: GlobalHotKeyManager,
    }

    impl Active {
        pub fn start() -> Result<Self, Error> {
            static HANDLER: Once = Once::new();
            HANDLER.call_once(|| {
                let [up, down] = keys().map(|key| key.id());
                GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                    if event.state != HotKeyState::Pressed {
                        return;
                    }
                    let step = if event.id == up {
                        1
                    } else if event.id == down {
                        -1
                    } else {
                        return;
                    };
                    PENDING.fetch_add(step, std::sync::atomic::Ordering::AcqRel);
                    if let Ok(wake) = WAKE.lock()
                        && let Some(wake) = wake.as_ref()
                    {
                        wake.request_repaint();
                    }
                }));
            });
            let manager = GlobalHotKeyManager::new().map_err(|_| Error::Failed)?;
            manager.register_all(&keys()).map_err(|error| match error {
                global_hotkey::Error::AlreadyRegistered(_) => Error::Taken,
                _ => Error::Failed,
            })?;
            Ok(Self { manager })
        }
    }

    impl Drop for Active {
        fn drop(&mut self) {
            let _ = self.manager.unregister_all(&keys());
        }
    }
}

#[derive(Default)]
pub struct VolumeKeys {
    #[cfg(all(any(windows, target_os = "linux"), not(test)))]
    active: Option<native::Active>,
    /// Whether the combinations are held now.
    held: bool,
    error: Option<Error>,
}

impl VolumeKeys {
    /// Takes or releases the combinations to match `enabled`. Runs on the
    /// main thread, where Windows delivers its hotkey messages.
    pub fn sync(&mut self, enabled: bool, wake: &RepaintTarget) {
        let wanted = enabled && support() == Support::Available;
        if wanted == self.held {
            return;
        }
        if let Ok(mut slot) = WAKE.lock() {
            *slot = wanted.then(|| wake.clone());
        }
        self.held = wanted;
        self.error = None;
        PENDING.store(0, Ordering::Release);
        #[cfg(all(any(windows, target_os = "linux"), not(test)))]
        {
            self.active = None;
            if wanted {
                match native::Active::start() {
                    Ok(active) => self.active = Some(active),
                    Err(error) => {
                        crate::diagnostics::event(
                            "volume_keys",
                            &format!("start_failed {error:?}"),
                        );
                        self.error = Some(error);
                    }
                }
            }
        }
    }

    /// The presses since last asked, as a volume change; none count while
    /// the combinations are not held.
    pub fn take_change(&self) -> f32 {
        let presses = PENDING.swap(0, Ordering::AcqRel);
        if self.held {
            presses as f32 * STEP
        } else {
            0.0
        }
    }

    pub fn error(&self) -> Option<Error> {
        self.error
    }
}

#[cfg(test)]
pub fn press_for_test(step: i32) {
    PENDING.fetch_add(step, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_volume_keys_are_offered_on_windows_and_x11_but_never_on_wayland() {
        let session = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(support_in(true, false, session(&[])), Support::Available);
        assert_eq!(support_in(false, false, session(&[])), Support::Unsupported);
        assert_eq!(
            support_in(false, true, session(&[("DISPLAY", ":0")])),
            Support::Available
        );
        // XWayland sets DISPLAY too; the session is still Wayland.
        for wayland in [
            &[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-1")][..],
            &[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "wayland")][..],
            &[("WAYLAND_DISPLAY", "wayland-0")][..],
        ] {
            assert_eq!(support_in(false, true, session(wayland)), Support::Wayland);
        }
        assert_eq!(
            support_in(false, true, session(&[("DISPLAY", "")])),
            Support::Unsupported
        );
    }
}
