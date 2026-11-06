//! Shell double-click notifications arrive on button-down. Never restore an
//! application from that phase or dispatch the following release twice.
use super::Command;

#[derive(Clone, Copy, Debug)]
pub(super) enum LeftClick {
    Press,
    DoublePress,
    Release,
}

#[derive(Clone, Default)]
pub(super) struct ActivationGate {
    suppress_release: bool,
}

impl ActivationGate {
    pub(super) fn input(&mut self, input: LeftClick) -> bool {
        match input {
            LeftClick::Press => {
                self.suppress_release = false;
                false
            }
            LeftClick::DoublePress => {
                self.suppress_release = true;
                false
            }
            LeftClick::Release => !std::mem::take(&mut self.suppress_release),
        }
    }
}

pub(super) fn menu_command(route_id: &str, event_id: &str) -> Option<Command> {
    let suffix = event_id.strip_prefix(route_id)?.strip_prefix('.')?;
    match suffix {
        "show" => Some(Command::Show),
        "play" => Some(Command::TogglePlayback),
        "previous" => Some(Command::Previous),
        "next" => Some(Command::Next),
        "quit" => Some(Command::Quit),
        _ => None,
    }
}

pub(super) fn dark_menu(apps_light: Option<u32>, system_light: Option<u32>) -> bool {
    apps_light.or(system_light).is_some_and(|light| light == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_click_activates_only_after_button_release() {
        let mut gate = ActivationGate::default();
        assert!(!gate.input(LeftClick::Press));
        assert!(gate.input(LeftClick::Release));
    }

    #[test]
    fn a_double_click_never_activates_while_pressed_or_repeats_the_first_show() {
        let mut gate = ActivationGate::default();
        let actions: Vec<_> = [
            LeftClick::Press,
            LeftClick::Release,
            LeftClick::DoublePress,
            LeftClick::Release,
        ]
        .into_iter()
        .map(|input| gate.input(input))
        .collect();
        assert_eq!(actions, [false, true, false, false]);
        assert!(!gate.input(LeftClick::Press));
        assert!(gate.input(LeftClick::Release));
    }

    #[test]
    fn a_lost_double_click_release_cannot_disable_the_next_normal_click() {
        let mut gate = ActivationGate::default();
        assert!(!gate.input(LeftClick::DoublePress));
        assert!(!gate.input(LeftClick::Press));
        assert!(gate.input(LeftClick::Release));
    }

    #[test]
    fn only_the_current_routes_exact_menu_actions_are_accepted() {
        for (suffix, expected) in [
            ("show", Command::Show),
            ("play", Command::TogglePlayback),
            ("previous", Command::Previous),
            ("next", Command::Next),
            ("quit", Command::Quit),
        ] {
            assert_eq!(
                menu_command("vk-fast-tray-2", &format!("vk-fast-tray-2.{suffix}")),
                Some(expected)
            );
        }
        for rejected in [
            "vk-fast-tray-1.play",
            "other.play",
            "vk-fast-tray-20.play",
            "vk-fast-tray-2.play.extra",
            "vk-fast-tray-2.mute",
            "vk-fast-tray-2play",
            "vk-fast-tray-2.",
        ] {
            assert_eq!(menu_command("vk-fast-tray-2", rejected), None, "{rejected}");
        }
    }

    #[test]
    fn native_menu_follows_windows_app_theme_with_safe_fallbacks() {
        assert!(dark_menu(Some(0), Some(1)));
        assert!(!dark_menu(Some(1), Some(0)));
        assert!(dark_menu(None, Some(0)));
        assert!(!dark_menu(None, Some(1)));
        assert!(!dark_menu(None, None));
    }
}
