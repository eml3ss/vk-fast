//! Native tray ownership. Callbacks only send commands and wake the resident
//! application; hiding and exiting remain explicit application decisions.
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};

use crate::{i18n::Language, repaint_target::RepaintTarget};

#[cfg(any(target_os = "windows", test))]
#[path = "system_tray/windows_input.rs"]
mod windows_input;

#[cfg(target_os = "linux")]
#[path = "system_tray/linux.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "system_tray/windows.rs"]
mod platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Show,
    TogglePlayback,
    Next,
    Quit,
    Unavailable,
}

#[derive(Debug)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

const URGENT_SHOW: u8 = 1;
const URGENT_QUIT: u8 = 2;
const URGENT_UNAVAILABLE: u8 = 4;

#[derive(Clone)]
struct Relay {
    sender: SyncSender<Command>,
    urgent: Arc<AtomicU8>,
    repaint: RepaintTarget,
}

impl Relay {
    fn emit(&self, command: Command) {
        match command {
            // Exit and restoring access must never be lost behind a burst of
            // media-button events in the bounded ordinary command channel.
            Command::Show => {
                self.urgent.fetch_or(URGENT_SHOW, Ordering::Release);
            }
            Command::Quit => {
                self.urgent.fetch_or(URGENT_QUIT, Ordering::Release);
            }
            Command::Unavailable => {
                self.urgent.fetch_or(URGENT_UNAVAILABLE, Ordering::Release);
            }
            _ => {
                let _ = self.sender.try_send(command);
            }
        }
        self.repaint.request_repaint();
    }
}

struct Labels {
    show: &'static str,
    playback: &'static str,
    next: &'static str,
    quit: &'static str,
}

fn labels(language: Language, playing: bool) -> Labels {
    Labels {
        show: language.text("Открыть VK Fast", "Open VK Fast"),
        playback: if playing {
            language.text("Пауза", "Pause")
        } else {
            language.text("Воспроизвести", "Play")
        },
        next: language.text("Следующий трек", "Next track"),
        quit: language.text("Выйти", "Exit"),
    }
}

pub struct SystemTray {
    backend: platform::Backend,
    receiver: Receiver<Command>,
    urgent: Arc<AtomicU8>,
    language: Language,
    playing: bool,
}

impl SystemTray {
    /// Windows must create/drop this on the Win32 event-loop thread. Linux's
    /// StatusNotifier service owns a separate worker and needs no GTK loop.
    pub fn new(repaint: RepaintTarget, language: Language) -> Result<Self, Error> {
        let (sender, receiver) = mpsc::sync_channel(128);
        let urgent = Arc::new(AtomicU8::new(0));
        let relay = Relay {
            sender,
            urgent: urgent.clone(),
            repaint,
        };
        Ok(Self {
            backend: platform::Backend::new(relay, language)?,
            receiver,
            urgent,
            language,
            playing: false,
        })
    }

    /// A Linux host must have accepted registration. An unavailable tray is
    /// never a reason to make the main window unreachable.
    pub fn available(&self) -> bool {
        self.backend.available()
    }

    pub fn drain_commands(&mut self) -> Vec<Command> {
        let urgent = self.urgent.swap(0, Ordering::AcqRel);
        let mut commands: Vec<_> = self.receiver.try_iter().collect();
        if urgent & URGENT_QUIT != 0 {
            return vec![Command::Quit];
        }
        if urgent & URGENT_UNAVAILABLE != 0 {
            commands.insert(0, Command::Unavailable);
        } else if urgent & URGENT_SHOW != 0 {
            commands.insert(0, Command::Show);
        }
        commands
    }

    pub fn set_state(&mut self, language: Language, playing: bool) {
        if self.language == language && self.playing == playing {
            return;
        }
        self.language = language;
        self.playing = playing;
        self.backend.set_state(language, playing);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use super::*;
    pub(super) struct Backend;
    impl Backend {
        pub(super) fn new(_: Relay, _: Language) -> Result<Self, Error> {
            Err(Error("System tray is unavailable on this platform".into()))
        }
        pub(super) fn available(&self) -> bool {
            false
        }
        pub(super) fn set_state(&mut self, _: Language, _: bool) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_and_restore_are_not_lost_when_media_commands_fill_the_channel() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let urgent = Arc::new(AtomicU8::new(0));
        let relay = Relay {
            sender,
            urgent: urgent.clone(),
            repaint: RepaintTarget::default(),
        };
        relay.emit(Command::Next);
        relay.emit(Command::TogglePlayback);
        relay.emit(Command::Show);
        relay.emit(Command::Unavailable);
        relay.emit(Command::Quit);
        assert_eq!(receiver.try_recv().unwrap(), Command::Next);
        assert_eq!(
            urgent.load(Ordering::Acquire),
            URGENT_SHOW | URGENT_UNAVAILABLE | URGENT_QUIT
        );
    }

    #[test]
    fn tray_labels_follow_language_and_actual_playback_state() {
        assert_eq!(labels(Language::English, true).playback, "Pause");
        assert_eq!(labels(Language::English, false).playback, "Play");
        assert_eq!(labels(Language::Russian, true).quit, "Выйти");
        assert_eq!(labels(Language::English, false).show, "Open VK Fast");
    }
}
