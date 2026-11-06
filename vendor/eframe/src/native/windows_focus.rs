//! Windows activation must never synthesize keyboard input: those events reach
//! global hotkeys in unrelated applications, including right-Alt/AltGr binds.

pub(super) trait Target {
    fn visible(&self) -> bool;
    fn minimized(&self) -> bool;
    fn foreground(&self) -> bool;
    fn activate(&mut self) -> bool;
    fn attention(&mut self);
}

fn request(target: &mut impl Target) -> bool {
    if !target.visible() || target.minimized() {
        return false;
    }
    if target.foreground() {
        return true;
    }
    if target.activate() {
        true
    } else {
        target.attention();
        false
    }
}

/// Keep command order, intercepting focus before egui-winit can enter winit's
/// keyboard-based foreground workaround. Other platforms retain their path.
pub(super) fn dispatch(
    commands: impl IntoIterator<Item = egui::ViewportCommand>,
    intercept: bool,
    mut focus: impl FnMut(),
    mut ordinary: impl FnMut(egui::ViewportCommand),
) {
    for command in commands {
        if intercept && matches!(command, egui::ViewportCommand::Focus) {
            focus();
        } else {
            ordinary(command);
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(crate) fn focus_hwnd(handle: isize) -> bool {
    use windows_sys::Win32::{
        Foundation::HWND,
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{
            FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GetForegroundWindow, GetWindowThreadProcessId,
            IsIconic, IsWindowVisible, SetForegroundWindow,
        },
    };
    let hwnd = handle as HWND;
    if hwnd.is_null() {
        return false;
    }
    let mut owner = 0;
    // HWND is an opaque OS identifier. Windows validates it; the only pointer
    // passed here is a live four-byte PID output on this stack.
    if unsafe { GetWindowThreadProcessId(hwnd, &mut owner) } == 0
        || owner != unsafe { GetCurrentProcessId() }
    {
        return false;
    }
    struct Native(HWND);
    impl Target for Native {
        fn visible(&self) -> bool {
            unsafe { IsWindowVisible(self.0) != 0 }
        }
        fn minimized(&self) -> bool {
            unsafe { IsIconic(self.0) != 0 }
        }
        fn foreground(&self) -> bool {
            unsafe { GetForegroundWindow() == self.0 }
        }
        fn activate(&mut self) -> bool {
            unsafe { SetForegroundWindow(self.0) != 0 }
        }
        fn attention(&mut self) {
            // Respect Windows' foreground policy without faking Alt, resetting
            // keyboard state or attaching another application's input queue.
            let info = FLASHWINFO {
                cbSize: size_of::<FLASHWINFO>() as u32,
                hwnd: self.0,
                dwFlags: FLASHW_TRAY,
                uCount: 3,
                dwTimeout: 0,
            };
            unsafe {
                FlashWindowEx(&info);
            }
        }
    }
    request(&mut Native(hwnd))
}

#[cfg(target_os = "windows")]
pub(super) fn focus_window(window: &winit::window::Window) {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    if let Ok(handle) = window.window_handle()
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        focus_hwnd(handle.hwnd.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        visible: bool,
        minimized: bool,
        foreground: bool,
        allowed: bool,
        actions: Vec<&'static str>,
    }
    impl Target for Fake {
        fn visible(&self) -> bool {
            self.visible
        }
        fn minimized(&self) -> bool {
            self.minimized
        }
        fn foreground(&self) -> bool {
            self.foreground
        }
        fn activate(&mut self) -> bool {
            self.actions.push("activate");
            self.allowed
        }
        fn attention(&mut self) {
            self.actions.push("attention");
        }
    }

    #[test]
    fn foreground_hidden_and_minimized_windows_do_not_trigger_an_activation_workaround() {
        for (visible, minimized, foreground) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            let mut target = Fake {
                visible,
                minimized,
                foreground,
                allowed: true,
                actions: Vec::new(),
            };
            assert_eq!(request(&mut target), visible && !minimized && foreground);
            assert!(target.actions.is_empty());
        }
    }

    #[test]
    fn denied_activation_only_requests_attention_without_retries_or_keyboard_input() {
        for allowed in [true, false] {
            let mut target = Fake {
                visible: true,
                minimized: false,
                foreground: false,
                allowed,
                actions: Vec::new(),
            };
            assert_eq!(request(&mut target), allowed);
            assert_eq!(
                target.actions,
                if allowed {
                    vec!["activate"]
                } else {
                    vec!["activate", "attention"]
                }
            );
        }
    }

    #[test]
    fn windows_focus_never_reaches_the_generic_window_backend_and_restore_order_is_preserved() {
        use egui::ViewportCommand as C;
        for intercept in [true, false] {
            let actions = RefCell::new(Vec::new());
            dispatch(
                [
                    C::Visible(true),
                    C::Minimized(false),
                    C::Focus,
                    C::Title("unchanged".into()),
                ],
                intercept,
                || actions.borrow_mut().push("native-focus"),
                |command| {
                    actions.borrow_mut().push(match command {
                        C::Visible(true) => "show",
                        C::Minimized(false) => "restore",
                        C::Focus => "generic-focus",
                        C::Title(_) => "title",
                        _ => panic!("Unexpected command"),
                    })
                },
            );
            assert_eq!(
                *actions.borrow(),
                [
                    "show",
                    "restore",
                    if intercept {
                        "native-focus"
                    } else {
                        "generic-focus"
                    },
                    "title"
                ]
            );
        }
    }
}
