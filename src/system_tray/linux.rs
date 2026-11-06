use super::*;
use ksni::blocking::TrayMethods;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const PENDING: u8 = 0;
const ONLINE: u8 = 1;
const OFFLINE: u8 = 2;
const CLOSED: u8 = 3;

enum Update {
    StateChanged,
    Shutdown,
}

pub(super) struct Backend {
    sender: mpsc::SyncSender<Update>,
    status: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    latest: Arc<Mutex<(Language, bool)>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Backend {
    pub(super) fn new(relay: Relay, language: Language) -> Result<Self, Error> {
        let icon = crate::app_icon::native_icon();
        let pixels: Vec<_> = icon
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
            .collect();
        let status = Arc::new(AtomicU8::new(PENDING));
        let stop = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new((language, false)));
        let (sender, receiver) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_status = status.clone();
        let worker_stop = stop.clone();
        let worker_latest = latest.clone();
        let worker = thread::Builder::new()
            .name("vk-tray-sni".into())
            .spawn(move || {
                let data = TrayData {
                    relay: relay.clone(),
                    status: worker_status.clone(),
                    language,
                    playing: false,
                    icon: ksni::Icon {
                        width: icon.width as i32,
                        height: icon.height as i32,
                        data: pixels,
                    },
                };
                // Do not use assume_sni_available: accepting a non-existent host
                // would let the application hide with no way to restore it.
                match data.spawn() {
                    Ok(handle) => {
                        let _ = worker_status.compare_exchange(
                            PENDING,
                            ONLINE,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        let _ = ready_tx.send(Ok(()));
                        while !worker_stop.load(Ordering::Acquire) {
                            match receiver.recv() {
                                Ok(Update::StateChanged) => {
                                    let (language, playing) = worker_latest
                                        .lock()
                                        .map(|state| *state)
                                        .unwrap_or((language, false));
                                    if handle
                                        .update(|tray| {
                                            tray.language = language;
                                            tray.playing = playing;
                                        })
                                        .is_none()
                                    {
                                        worker_status.store(OFFLINE, Ordering::Release);
                                        relay.emit(Command::Unavailable);
                                        break;
                                    }
                                }
                                Ok(Update::Shutdown) | Err(_) => break,
                            }
                        }
                        handle.shutdown().wait();
                        worker_status.store(CLOSED, Ordering::Release);
                    }
                    Err(error) => {
                        worker_status.store(OFFLINE, Ordering::Release);
                        let _ = ready_tx.send(Err(error.to_string()));
                        relay.emit(Command::Unavailable);
                    }
                }
            })
            .map_err(|error| Error(error.to_string()))?;
        let backend = Self {
            sender,
            status,
            stop,
            latest,
            worker: Some(worker),
        };
        // Usually registration takes a few milliseconds. A stalled shell must
        // not block the event loop (and hence closing) for the D-Bus timeout.
        match ready_rx.recv_timeout(Duration::from_millis(750)) {
            Ok(Err(error)) => Err(Error(error)),
            _ => Ok(backend),
        }
    }

    pub(super) fn available(&self) -> bool {
        self.status.load(Ordering::Acquire) == ONLINE
    }

    pub(super) fn set_state(&mut self, language: Language, playing: bool) {
        if let Ok(mut state) = self.latest.lock() {
            *state = (language, playing);
        }
        // A queued notification always reads the latest state, so coalescing
        // never leaves a stale play/pause or language label behind.
        let _ = self.sender.try_send(Update::StateChanged);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.status.store(CLOSED, Ordering::Release);
        let _ = self.sender.try_send(Update::Shutdown);
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            let _ = worker.join();
        }
        // A shell that stopped answering during initialization must not hold
        // up process exit. The worker observes stop as soon as it returns.
    }
}

struct TrayData {
    relay: Relay,
    status: Arc<AtomicU8>,
    language: Language,
    playing: bool,
    icon: ksni::Icon,
}

impl ksni::Tray for TrayData {
    fn id(&self) -> String {
        "vk-fast".into()
    }
    fn title(&self) -> String {
        "VK Fast".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "VK Fast".into(),
            description: self
                .language
                .text("Музыкальный плеер VK", "VK music player")
                .into(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _: i32, _: i32) {
        self.relay.emit(Command::Show);
    }
    fn secondary_activate(&mut self, _: i32, _: i32) {
        self.relay.emit(Command::TogglePlayback);
    }
    fn watcher_online(&self) {
        self.status.store(ONLINE, Ordering::Release);
    }
    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        self.status.store(OFFLINE, Ordering::Release);
        self.relay.emit(Command::Unavailable);
        true
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let labels = labels(self.language, self.playing);
        [
            (labels.show, Command::Show),
            (labels.previous, Command::Previous),
            (labels.playback, Command::TogglePlayback),
            (labels.next, Command::Next),
            (labels.quit, Command::Quit),
        ]
        .into_iter()
        .enumerate()
        .flat_map(|(index, (label, command))| {
            let item = ksni::menu::StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Self| tray.relay.emit(command)),
                ..Default::default()
            }
            .into();
            if index == 4 {
                vec![ksni::MenuItem::Separator, item]
            } else {
                vec![item]
            }
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn losing_the_host_disables_hiding_and_preserves_a_restore_request() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let urgent = Arc::new(AtomicU8::new(0));
        let status = Arc::new(AtomicU8::new(ONLINE));
        let data = TrayData {
            relay: Relay {
                sender,
                urgent: urgent.clone(),
                repaint: RepaintTarget::default(),
            },
            status: status.clone(),
            language: Language::English,
            playing: false,
            icon: ksni::Icon {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 255],
            },
        };
        assert!(ksni::Tray::watcher_offline(&data, ksni::OfflineReason::No));
        assert_eq!(status.load(Ordering::Acquire), OFFLINE);
        assert_ne!(urgent.load(Ordering::Acquire) & URGENT_UNAVAILABLE, 0);
        assert!(receiver.try_recv().is_err());
        ksni::Tray::watcher_online(&data);
        assert_eq!(status.load(Ordering::Acquire), ONLINE);
    }

    #[test]
    fn sni_menu_and_activation_send_intents_without_closing_the_process() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let urgent = Arc::new(AtomicU8::new(0));
        let mut data = TrayData {
            relay: Relay {
                sender,
                urgent: urgent.clone(),
                repaint: RepaintTarget::default(),
            },
            status: Arc::new(AtomicU8::new(ONLINE)),
            language: Language::English,
            playing: true,
            icon: ksni::Icon {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 255],
            },
        };
        ksni::Tray::activate(&mut data, 0, 0);
        assert_eq!(urgent.swap(0, Ordering::AcqRel), URGENT_SHOW);
        let menu = ksni::Tray::menu(&data);
        assert_eq!(menu.len(), 6);
        for (index, expected) in [
            (1, Command::Previous),
            (2, Command::TogglePlayback),
            (3, Command::Next),
        ] {
            let ksni::MenuItem::Standard(item) = &menu[index] else {
                panic!("Menu item")
            };
            (item.activate)(&mut data);
            assert_eq!(receiver.try_recv().unwrap(), expected);
        }
        let ksni::MenuItem::Standard(exit) = &menu[5] else {
            panic!("Exit")
        };
        (exit.activate)(&mut data);
        assert_ne!(urgent.load(Ordering::Acquire) & URGENT_QUIT, 0);
    }

    #[test]
    #[ignore = "requires VK_FAST_TRAY_ISOLATED_TEST=1, dbus-run-session, busctl and Python PyGObject"]
    fn isolated_status_notifier_roundtrip_detects_host_loss_and_never_assumes_a_missing_tray() {
        if std::env::var("VK_FAST_TRAY_ISOLATED_TEST").as_deref() != Ok("1") {
            return;
        }
        use std::{
            io::{BufRead, BufReader},
            process::{Child, Command as ProcessCommand, Stdio},
            time::Instant,
        };
        struct Watcher(Child);
        impl Drop for Watcher {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let script = r#"
from gi.repository import Gio, GLib
connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
items = []
xml = '<node><interface name="org.kde.StatusNotifierWatcher"><method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method><method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method><property name="IsStatusNotifierHostRegistered" type="b" access="read"/><property name="RegisteredStatusNotifierItems" type="as" access="read"/><property name="ProtocolVersion" type="i" access="read"/></interface></node>'
info = Gio.DBusNodeInfo.new_for_xml(xml).interfaces[0]
def method(conn, sender, path, interface, name, parameters, invocation):
    if name == 'RegisterStatusNotifierItem':
        items.append(parameters.unpack()[0])
    invocation.return_value(None)
def property_get(conn, sender, path, interface, name):
    if name == 'IsStatusNotifierHostRegistered': return GLib.Variant('b', True)
    if name == 'RegisteredStatusNotifierItems': return GLib.Variant('as', items)
    if name == 'ProtocolVersion': return GLib.Variant('i', 0)
connection.register_object('/StatusNotifierWatcher', info, method, property_get, None)
reply = connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'RequestName', GLib.Variant('(su)', ('org.kde.StatusNotifierWatcher', 4)), GLib.VariantType.new('(u)'), Gio.DBusCallFlags.NONE, 1000, None)
assert reply.unpack()[0] == 1
print('READY', flush=True)
GLib.MainLoop().run()
"#;
        let child = ProcessCommand::new("python3")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut watcher = Watcher(child);
        let mut line = String::new();
        BufReader::new(watcher.0.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "READY");
        let bus = |args: &[&str]| {
            let output = ProcessCommand::new("busctl")
                .arg("--user")
                .arg("--")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "busctl: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        let wait = |condition: &mut dyn FnMut() -> bool| {
            let until = Instant::now() + Duration::from_secs(2);
            while !condition() {
                assert!(Instant::now() < until, "StatusNotifier condition timed out");
                thread::sleep(Duration::from_millis(10));
            }
        };
        let mut tray = SystemTray::new(RepaintTarget::default(), Language::English).unwrap();
        wait(&mut || tray.available());
        let registered = bus(&[
            "get-property",
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
            "RegisteredStatusNotifierItems",
        ]);
        let name = registered
            .split('"')
            .nth(1)
            .expect("Registered tray name")
            .to_owned();
        assert!(name.starts_with("org.kde.StatusNotifierItem-"));
        let title = bus(&[
            "get-property",
            &name,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "Title",
        ]);
        assert!(title.contains("VK Fast"));
        bus(&[
            "call",
            &name,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "Activate",
            "ii",
            "0",
            "0",
        ]);
        wait(&mut || tray.drain_commands().contains(&super::super::Command::Show));
        tray.set_state(Language::English, true);
        wait(&mut || {
            bus(&[
                "call",
                &name,
                "/MenuBar",
                "com.canonical.dbusmenu",
                "GetLayout",
                "iias",
                "0",
                "-1",
                "0",
            ])
            .contains("Pause")
        });
        watcher.0.kill().unwrap();
        watcher.0.wait().unwrap();
        wait(&mut || !tray.available());
        wait(&mut || {
            tray.drain_commands()
                .contains(&super::super::Command::Unavailable)
        });
        drop(tray);
        wait(&mut || {
            bus(&[
                "call",
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                "s",
                &name,
            ])
            .trim()
                == "b false"
        });
        assert!(SystemTray::new(RepaintTarget::default(), Language::English).is_err());
    }
}
