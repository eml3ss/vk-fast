use super::windows_input::{ActivationGate, LeftClick, menu_command};
use super::*;
use std::sync::{
    Mutex, Once,
    atomic::{AtomicU64, Ordering},
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent, TrayIconId,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

static ROUTE: Mutex<Option<Route>> = Mutex::new(None);
static HANDLERS: Once = Once::new();
static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct Route {
    id: String,
    relay: Relay,
    activation: ActivationGate,
}

fn register_handlers() {
    HANDLERS.call_once(|| {
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            let input = match &event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => LeftClick::Release,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Down,
                    ..
                } => LeftClick::Press,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => LeftClick::DoublePress,
                _ => return,
            };
            let relay = ROUTE.lock().ok().and_then(|mut route| {
                route
                    .as_mut()
                    .filter(|route| route.id == event.id().as_ref())
                    .and_then(|route| route.activation.input(input).then(|| route.relay.clone()))
            });
            if let Some(relay) = relay {
                crate::diagnostics::event("tray.windows", "left_button_released_show");
                relay.emit(Command::Show);
            }
        }));
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            let delivery = ROUTE.lock().ok().and_then(|route| {
                route.as_ref().and_then(|route| {
                    let command = menu_command(&route.id, event.id().as_ref())?;
                    Some((route.relay.clone(), command))
                })
            });
            if let Some((relay, command)) = delivery {
                crate::diagnostics::event(
                    "tray.windows",
                    match command {
                        Command::Show => "menu_show",
                        Command::TogglePlayback => "menu_toggle_playback",
                        Command::Next => "menu_next",
                        Command::Quit => "menu_quit",
                        Command::Unavailable => "menu_unavailable",
                    },
                );
                relay.emit(command);
            }
        }));
    });
}

pub(super) struct Backend {
    _icon: TrayIcon,
    id: String,
    show: MenuItem,
    playback: MenuItem,
    next: MenuItem,
    quit: MenuItem,
}

impl Backend {
    pub(super) fn new(relay: Relay, language: Language) -> Result<Self, Error> {
        register_handlers();
        let id = format!(
            "vk-fast-tray-{}",
            GENERATION.fetch_add(1, Ordering::Relaxed)
        );
        let labels = labels(language, false);
        let show = MenuItem::with_id(format!("{id}.show"), labels.show, true, None);
        let playback = MenuItem::with_id(format!("{id}.play"), labels.playback, true, None);
        let next = MenuItem::with_id(format!("{id}.next"), labels.next, true, None);
        let quit = MenuItem::with_id(format!("{id}.quit"), labels.quit, true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &show,
            &playback,
            &next,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|error| Error(error.to_string()))?;
        let image = crate::app_icon::native_icon();
        let icon = Icon::from_rgba(image.rgba.clone(), image.width, image.height)
            .map_err(|error| Error(error.to_string()))?;
        let icon = TrayIconBuilder::new()
            .with_id(TrayIconId::new(&id))
            .with_icon(icon)
            .with_tooltip("VK Fast")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| Error(error.to_string()))?;
        if let Ok(mut route) = ROUTE.lock() {
            *route = Some(Route {
                id: id.clone(),
                relay,
                activation: ActivationGate::default(),
            });
        }
        Ok(Self {
            _icon: icon,
            id,
            show,
            playback,
            next,
            quit,
        })
    }

    pub(super) fn available(&self) -> bool {
        true
    }

    pub(super) fn set_state(&mut self, language: Language, playing: bool) {
        let labels = labels(language, playing);
        self.show.set_text(labels.show);
        self.playback.set_text(labels.playback);
        self.next.set_text(labels.next);
        self.quit.set_text(labels.quit);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Ok(mut route) = ROUTE.lock()
            && route.as_ref().is_some_and(|route| route.id == self.id)
        {
            *route = None;
        }
    }
}
