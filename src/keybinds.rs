//! Player shortcuts. Focused widgets keep their own keyboard behavior; callers
//! also suppress this reader while an application modal or menu is open.
use eframe::egui::{self, Event, Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    TogglePause,
    Seek(f64),
    Volume(f32),
    Previous,
    Next,
    /// Ctrl+R or F5: refresh the open page, like its Refresh button.
    Reload,
    /// Page Down (1) or Page Up (-1): scroll the list under the pointer by
    /// about a screen.
    Page(f32),
    /// Home: back to the top of the list under the pointer.
    Top,
}

struct Binding {
    command: Command,
    repeat: bool,
}

fn binding(key: Key, modifiers: Modifiers) -> Option<Binding> {
    if modifiers.alt || modifiers.shift || (modifiers.ctrl && modifiers.mac_cmd) {
        return None;
    }
    if modifiers.ctrl || modifiers.mac_cmd || modifiers.command {
        let command = match key {
            Key::ArrowLeft => Command::Previous,
            Key::ArrowRight => Command::Next,
            Key::R => Command::Reload,
            _ => return None,
        };
        return Some(Binding {
            command,
            repeat: false,
        });
    }
    let command = match key {
        Key::Space => Command::TogglePause,
        Key::ArrowLeft => Command::Seek(-5.0),
        Key::ArrowRight => Command::Seek(5.0),
        Key::ArrowUp => Command::Volume(0.05),
        Key::ArrowDown => Command::Volume(-0.05),
        // F5 refreshes the open page, like Ctrl+R.
        Key::F5 => Command::Reload,
        Key::PageDown => Command::Page(1.0),
        Key::PageUp => Command::Page(-1.0),
        Key::Home => Command::Top,
        _ => return None,
    };
    Some(Binding {
        command,
        // A held Page key glides on instead of repeating page jumps.
        repeat: !matches!(
            key,
            Key::Space | Key::F5 | Key::Home | Key::PageDown | Key::PageUp
        ),
    })
}

/// Consume player shortcuts once per frame, in their original event order.
///
/// Pointer-origin focus on track rows and transport buttons should be released
/// by their caller, so playback keys work after clicking them. Keyboard focus
/// remains protected: sliders, switches and text fields must not receive both
/// their own action and a global player action from the same key.
pub fn read(context: &egui::Context) -> Vec<Command> {
    if context.current_pass_index() != 0
        || context.egui_wants_keyboard_input()
        || !context.input(|input| input.focused)
    {
        return Vec::new();
    }
    let frame = context.cumulative_frame_nr();
    let id = egui::Id::new("player-shortcuts-last-frame").with(context.viewport_id());
    let already_read = context.data_mut(|data| {
        if data.get_temp::<u64>(id) == Some(frame) {
            true
        } else {
            data.insert_temp(id, frame);
            false
        }
    });
    if already_read {
        return Vec::new();
    }
    let mut commands = Vec::new();
    context.input_mut(|input| {
        input.events.retain(|event| {
            let Event::Key {
                key,
                modifiers,
                pressed: true,
                repeat,
                ..
            } = event
            else {
                return true;
            };
            let Some(binding) = binding(*key, *modifiers) else {
                return true;
            };
            if !repeat || binding.repeat {
                commands.push(binding.command);
            }
            false
        });
    });
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, modifiers: Modifiers, pressed: bool, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat,
            modifiers,
        }
    }

    fn frame(
        context: &egui::Context,
        events: Vec<Event>,
        focused: bool,
        mut widgets: impl FnMut(&mut egui::Ui),
    ) -> (Vec<Command>, Vec<Event>) {
        let mut commands = Vec::new();
        let mut remaining = Vec::new();
        let mut output = context.run_ui(
            egui::RawInput {
                focused,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 160.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                widgets(ui);
                if context.current_pass_index() == 0 {
                    commands = read(context);
                    assert!(
                        read(context).is_empty(),
                        "Shortcuts fired twice in one frame"
                    );
                    remaining = context.input(|input| input.events.clone());
                }
            },
        );
        output.textures_delta.clear();
        (commands, remaining)
    }

    #[test]
    fn plain_playback_keys_map_to_exact_steps_and_are_consumed_once() {
        let context = egui::Context::default();
        let events = [
            Key::Space,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
        ]
        .into_iter()
        .map(|key_name| key(key_name, Modifiers::NONE, true, false))
        .collect();
        let (commands, remaining) = frame(&context, events, true, |_| {});
        assert_eq!(
            commands,
            [
                Command::TogglePause,
                Command::Seek(-5.0),
                Command::Seek(5.0),
                Command::Volume(0.05),
                Command::Volume(-0.05),
            ]
        );
        assert!(remaining.is_empty());
    }

    #[test]
    fn page_keys_scroll_and_home_goes_to_the_top_but_not_while_typing() {
        let context = egui::Context::default();
        let events = [Key::PageDown, Key::PageUp, Key::Home]
            .into_iter()
            .map(|key_name| key(key_name, Modifiers::NONE, true, false))
            .collect();
        let (commands, remaining) = frame(&context, events, true, |_| {});
        assert_eq!(
            commands,
            [Command::Page(1.0), Command::Page(-1.0), Command::Top]
        );
        assert!(remaining.is_empty());
        // Held keys do not repeat: a held Page key glides on instead.
        let (commands, remaining) = frame(
            &context,
            vec![
                key(Key::PageDown, Modifiers::NONE, true, true),
                key(Key::Home, Modifiers::NONE, true, true),
            ],
            true,
            |_| {},
        );
        assert!(commands.is_empty() && remaining.is_empty());
        // A focused text field keeps Home for its cursor.
        let mut text = String::from("search");
        let context = egui::Context::default();
        let mut field = |ui: &mut egui::Ui| {
            let response = ui.text_edit_singleline(&mut text);
            response.request_focus();
        };
        frame(&context, Vec::new(), true, &mut field);
        let (commands, _) = frame(
            &context,
            vec![key(Key::Home, Modifiers::NONE, true, false)],
            true,
            &mut field,
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn control_or_platform_command_changes_tracks_but_extra_modifiers_do_not() {
        for modifiers in [
            Modifiers::CTRL,
            Modifiers::CTRL | Modifiers::COMMAND,
            Modifiers::MAC_CMD | Modifiers::COMMAND,
            Modifiers::COMMAND,
        ] {
            let context = egui::Context::default();
            let (commands, _) = frame(
                &context,
                vec![
                    key(Key::ArrowLeft, modifiers, true, false),
                    key(Key::ArrowRight, modifiers, true, false),
                ],
                true,
                |_| {},
            );
            assert_eq!(commands, [Command::Previous, Command::Next]);
        }
        for (key_name, modifiers) in [
            (Key::ArrowLeft, Modifiers::ALT),
            (Key::ArrowRight, Modifiers::SHIFT),
            (Key::ArrowRight, Modifiers::CTRL | Modifiers::SHIFT),
            (Key::ArrowRight, Modifiers::CTRL | Modifiers::ALT),
            (Key::ArrowLeft, Modifiers::CTRL | Modifiers::MAC_CMD),
            (Key::ArrowUp, Modifiers::CTRL),
            (Key::ArrowDown, Modifiers::COMMAND),
            (Key::Space, Modifiers::COMMAND),
            (Key::A, Modifiers::NONE),
        ] {
            let context = egui::Context::default();
            let events = vec![key(key_name, modifiers, true, false)];
            let (commands, remaining) = frame(&context, events.clone(), true, |_| {});
            assert!(commands.is_empty());
            assert_eq!(remaining, events);
        }
    }

    #[test]
    fn only_seek_and_volume_repeat_while_holding_a_key() {
        for (key_name, modifiers, initial, repeated) in [
            (Key::Space, Modifiers::NONE, Command::TogglePause, None),
            (Key::ArrowRight, Modifiers::CTRL, Command::Next, None),
            (Key::ArrowLeft, Modifiers::CTRL, Command::Previous, None),
            (Key::R, Modifiers::CTRL, Command::Reload, None),
            (Key::F5, Modifiers::NONE, Command::Reload, None),
            (
                Key::ArrowRight,
                Modifiers::NONE,
                Command::Seek(5.0),
                Some(Command::Seek(5.0)),
            ),
            (
                Key::ArrowUp,
                Modifiers::NONE,
                Command::Volume(0.05),
                Some(Command::Volume(0.05)),
            ),
        ] {
            let context = egui::Context::default();
            let (commands, _) = frame(
                &context,
                vec![key(key_name, modifiers, true, false)],
                true,
                |_| {},
            );
            assert_eq!(commands, [initial]);
            let (commands, remaining) = frame(
                &context,
                vec![key(key_name, modifiers, true, true)],
                true,
                |_| {},
            );
            assert_eq!(commands, repeated.into_iter().collect::<Vec<_>>());
            assert!(remaining.is_empty());
            let (commands, remaining) = frame(
                &context,
                vec![key(key_name, modifiers, false, false)],
                true,
                |_| {},
            );
            assert!(commands.is_empty());
            assert_eq!(
                remaining.len(),
                1,
                "Key releases must stay available to egui"
            );
        }
    }

    #[test]
    fn focused_text_edit_keeps_typing_caret_shortcuts_and_space() {
        let context = egui::Context::default();
        let mut text = String::from("alpha beta");
        frame(&context, vec![], true, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("query"))
                .request_focus();
        });
        let events = vec![
            key(Key::Space, Modifiers::NONE, true, false),
            Event::Text(" ".into()),
            key(Key::ArrowLeft, Modifiers::CTRL, true, false),
        ];
        let mut before = Vec::new();
        let (commands, remaining) = frame(&context, events, true, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("query"));
            assert!(context.text_edit_focused());
            before = context.input(|input| input.events.clone());
        });
        assert!(commands.is_empty());
        assert_eq!(remaining, before, "A focused edit's events were stolen");
    }

    #[test]
    fn focused_slider_owns_its_arrows_and_other_focused_controls_keep_space() {
        let context = egui::Context::default();
        let mut value = 0.5_f32;
        frame(&context, vec![], true, |ui| {
            ui.add(egui::Slider::new(&mut value, 0.0..=1.0))
                .request_focus();
        });
        let (commands, _) = frame(
            &context,
            vec![key(Key::ArrowRight, Modifiers::NONE, true, false)],
            true,
            |ui| {
                ui.add(egui::Slider::new(&mut value, 0.0..=1.0));
            },
        );
        assert!(commands.is_empty());
        assert!(value > 0.5, "The focused slider should receive its own key");

        let context = egui::Context::default();
        frame(&context, vec![], true, |ui| {
            ui.button("A control").request_focus();
        });
        let (commands, _) = frame(
            &context,
            vec![key(Key::Space, Modifiers::NONE, true, false)],
            true,
            |ui| {
                let _ = ui.button("A control");
            },
        );
        assert!(commands.is_empty());
    }

    #[test]
    fn an_unfocused_window_does_not_act_on_or_consume_playback_keys() {
        let context = egui::Context::default();
        let events = vec![key(Key::Space, Modifiers::NONE, true, false)];
        let (commands, remaining) = frame(&context, events.clone(), false, |_| {});
        assert!(commands.is_empty());
        assert_eq!(remaining, events);
    }
}
