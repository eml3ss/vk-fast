//! A user-local activation channel, acquired before any application state opens.
//! The guard survives native-window recreation; activation survives UI startup.
use std::{
    collections::VecDeque,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::repaint_target::RepaintTarget;

const ACTIVATE: &[u8] = b"VKFAST_ACTIVATE_V1";
/// Activation that also carries one link to open, followed by the link.
const OPEN_LINK: &[u8] = b"VKFAST_OPENLINK_V1";
/// Activation with the launcher's Wayland activation token, then `\n` and
/// an optional link.
const TOKEN: &[u8] = b"VKFAST_ACTTOKEN_V1";
const MAX_LINKS: usize = 8;
const CONNECT_BUDGET: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_millis(10);

pub enum Acquisition {
    Primary(Primary),
    AlreadyRunning,
}

#[derive(Default)]
struct ActivationState {
    pending: AtomicBool,
    links: Mutex<VecDeque<String>>,
    token: Mutex<Option<String>>,
    repaint: Mutex<Option<RepaintTarget>>,
}

/// Clone this into the resident app; keep the Primary guard in main.
#[derive(Clone, Default)]
pub struct Activation(Arc<ActivationState>);

impl Activation {
    pub fn take_pending(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
    }

    /// Links sent by later launches, oldest first.
    pub fn take_links(&self) -> Vec<String> {
        self.0
            .links
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .drain(..)
            .collect()
    }

    /// Queue a link and wake the window, as a second launch does.
    pub fn open_link(&self, link: &str) {
        if link.is_empty() || link.len() > crate::link_router::MAX_URL {
            return;
        }
        {
            let mut links = self
                .0
                .links
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if links.len() == MAX_LINKS {
                links.pop_front();
            }
            links.push_back(link.to_owned());
        }
        self.notify();
    }

    /// The newest launcher activation token, for showing the window.
    pub fn take_token(&self) -> Option<String> {
        self.0
            .token
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    /// Handle one activation message; false for an unknown one.
    fn receive(&self, message: &[u8]) -> bool {
        if let Some(payload) = message.strip_prefix(TOKEN)
            && let Ok(payload) = std::str::from_utf8(payload)
            && let Some((token, link)) = payload.split_once('\n')
            && crate::window_focus::valid_token(token)
            && link.len() <= crate::link_router::MAX_URL
        {
            *self
                .0
                .token
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(token.to_owned());
            if link.is_empty() {
                self.notify();
            } else {
                self.open_link(link);
            }
            true
        } else if message == ACTIVATE {
            self.notify();
            true
        } else if let Some(link) = message.strip_prefix(OPEN_LINK)
            && let Ok(link) = std::str::from_utf8(link)
            && !link.is_empty()
            && link.len() <= crate::link_router::MAX_URL
        {
            self.open_link(link);
            true
        } else {
            false
        }
    }

    pub fn set_repaint_target(&self, target: RepaintTarget) {
        *self
            .0
            .repaint
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(target.clone());
        if self.0.pending.load(Ordering::Acquire) {
            target.request_repaint();
        }
    }

    #[cfg(test)]
    pub(crate) fn request_for_test(&self) {
        self.notify();
    }

    fn notify(&self) {
        self.0.pending.store(true, Ordering::Release);
        let target = self
            .0
            .repaint
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(target) = target {
            target.request_repaint();
        }
    }
}

pub struct Primary {
    activation: Activation,
    stopping: Arc<AtomicBool>,
    control: platform::Control,
    worker: Option<JoinHandle<()>>,
}

impl Primary {
    pub fn activations(&self) -> Activation {
        self.activation.clone()
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // Windows keeps the wake connection alive until accept returns; closing
        // it earlier could make the pipe discard it and wait for another client.
        #[cfg(windows)]
        let _wake = self.control.wake();
        #[cfg(not(windows))]
        self.control.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Windows calls the supplied foreground-permission grant with the real server
/// PID before sending activation. Linux does not need that policy handoff.
/// `link` and the launcher's activation `token` travel to an already running
/// instance with its activation.
pub fn acquire(
    grant_foreground: impl Fn(u32),
    link: Option<&str>,
    token: Option<&str>,
) -> io::Result<Acquisition> {
    platform::acquire(
        &platform::default_address()?,
        &grant_foreground,
        &message(link, token),
    )
}

fn message(link: Option<&str>, token: Option<&str>) -> Vec<u8> {
    let link = link.filter(|link| !link.is_empty() && link.len() <= crate::link_router::MAX_URL);
    match (
        token.filter(|token| crate::window_focus::valid_token(token)),
        link,
    ) {
        (Some(token), link) => [
            TOKEN,
            token.as_bytes(),
            b"\n",
            link.unwrap_or_default().as_bytes(),
        ]
        .concat(),
        (None, Some(link)) => [OPEN_LINK, link.as_bytes()].concat(),
        (None, None) => ACTIVATE.to_vec(),
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(any(windows, test))]
fn transfer(
    operation: impl FnMut(&mut [u8]) -> io::Result<usize>,
    bytes: &mut [u8],
) -> io::Result<()> {
    let mut operation = operation;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut offset = 0;
    while offset < bytes.len() {
        match operation(&mut bytes[offset..]) {
            // A PIPE_NOWAIT read can report zero bytes before the peer
            // writes. Interprocess also maps broken-pipe statuses to zero;
            // either case stays bounded by this operation's deadline.
            Ok(0) => (),
            Ok(length) => offset += length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        if offset < bytes.len() {
            thread::sleep(RETRY);
        }
    }
    Ok(())
}

fn primary(
    control: platform::Control,
    serve: impl FnOnce(Activation, Arc<AtomicBool>) + Send + 'static,
) -> io::Result<Acquisition> {
    let activation = Activation::default();
    let stopping = Arc::new(AtomicBool::new(false));
    let received = activation.clone();
    let stop = stopping.clone();
    let worker = thread::Builder::new()
        .name("vk-instance".into())
        .spawn(move || serve(received, stop))?;
    Ok(Acquisition::Primary(Primary {
        activation,
        stopping,
        control,
        worker: Some(worker),
    }))
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions, TryLockError},
        os::unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixDatagram,
        },
        path::{Path, PathBuf},
    };

    pub type Address = PathBuf;

    pub struct Control {
        _lock: File,
        socket: Arc<UnixDatagram>,
        path: PathBuf,
        identity: (u64, u64),
    }

    impl Control {
        pub fn wake(&self) {
            let _ = self.socket.send_to(b"STOP", &self.path);
        }
    }

    impl Drop for Control {
        fn drop(&mut self) {
            if fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity)
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    fn uid() -> io::Result<u32> {
        Ok(fs::metadata("/proc/self")?.uid())
    }

    pub fn default_address() -> io::Result<Address> {
        let uid = uid()?;
        // Fixed UID-based locations work across terminal/desktop launches with
        // different HOME, TMPDIR or XDG overrides and across copied binaries.
        let runtime = PathBuf::from(format!("/run/user/{uid}"));
        if fs::symlink_metadata(&runtime).is_ok_and(|metadata| {
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
        }) {
            Ok(runtime.join("vk-fast-instance"))
        } else {
            Ok(PathBuf::from(format!("/tmp/vk-fast-instance-{uid}")))
        }
    }

    fn private_directory(directory: &Path, uid: u32) -> io::Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(invalid("Instance directory is not private"));
        }
        Ok(())
    }

    pub fn acquire(address: &Address, _: &impl Fn(u32), message: &[u8]) -> io::Result<Acquisition> {
        let uid = uid()?;
        private_directory(address, uid)?;
        let lock_path = address.join("instance.lock");
        if let Ok(metadata) = fs::symlink_metadata(&lock_path)
            && (!metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != uid
                || metadata.permissions().mode() & 0o077 != 0)
        {
            return Err(invalid("Instance lock is not private"));
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        let path = address.join("instance.sock");
        let deadline = Instant::now() + CONNECT_BUDGET;
        loop {
            match lock.try_lock() {
                Ok(()) => {
                    match fs::symlink_metadata(&path) {
                        Ok(metadata)
                            if metadata.file_type().is_socket() && metadata.uid() == uid =>
                        {
                            fs::remove_file(&path)?
                        }
                        Ok(_) => return Err(invalid("Unexpected instance socket file")),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error),
                    }
                    let socket = Arc::new(UnixDatagram::bind(&path)?);
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
                    let metadata = fs::symlink_metadata(&path)?;
                    let receive = socket.clone();
                    return primary(
                        Control {
                            _lock: lock,
                            socket,
                            path,
                            identity: (metadata.dev(), metadata.ino()),
                        },
                        move |activation, stopping| {
                            let mut message =
                                vec![0_u8; TOKEN.len() + 512 + crate::link_router::MAX_URL];
                            while !stopping.load(Ordering::Acquire) {
                                match receive.recv(&mut message) {
                                    Ok(length) if !stopping.load(Ordering::Acquire) => {
                                        activation.receive(&message[..length]);
                                    }
                                    Ok(_) => (),
                                    Err(error)
                                        if matches!(
                                            error.kind(),
                                            io::ErrorKind::WouldBlock
                                                | io::ErrorKind::TimedOut
                                                | io::ErrorKind::Interrupted
                                        ) => {}
                                    Err(_) => return,
                                }
                            }
                        },
                    );
                }
                Err(TryLockError::Error(error)) => return Err(error),
                Err(TryLockError::WouldBlock) => {
                    let sender = UnixDatagram::unbound()?;
                    sender.set_nonblocking(true)?;
                    match sender.send_to(message, &path) {
                        Ok(length) if length == message.len() => {
                            return Ok(Acquisition::AlreadyRunning);
                        }
                        Ok(_) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::NotFound
                                    | io::ErrorKind::ConnectionRefused
                                    | io::ErrorKind::WouldBlock
                            ) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Existing instance did not accept activation",
                ));
            }
            thread::sleep(RETRY);
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use interprocess::{
        ConnectWaitMode,
        os::windows::named_pipe::{DuplexPipeStream, PipeListener, PipeListenerOptions, pipe_mode},
    };
    use sha2::{Digest, Sha256};
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Write},
        os::windows::fs::{MetadataExt, OpenOptionsExt},
    };

    type Stream = DuplexPipeStream<pipe_mode::Bytes>;
    type Listener = PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>;
    pub type Address = String;

    pub struct Control {
        _listener: Arc<Listener>,
        _exclusion: File,
        address: Address,
    }

    impl Control {
        pub fn wake(&self) -> Option<Stream> {
            Stream::connect_by_path_with_wait_mode(
                self.address.as_str(),
                ConnectWaitMode::Timeout(Duration::from_millis(500)),
            )
            .ok()
        }
    }

    pub fn default_address() -> io::Result<Address> {
        // A logical per-user name, independent of installation location. This
        // does not read or create AppData/config files. The pipe uses Windows'
        // process-token default ACL and rejects remote clients.
        let mut hash = Sha256::new();
        for variable in ["USERDOMAIN", "USERNAME", "USERPROFILE"] {
            let value = std::env::var_os(variable)
                .ok_or_else(|| invalid("Windows user identity is unavailable"))?;
            hash.update(value.to_string_lossy().to_lowercase().as_bytes());
            hash.update([0]);
        }
        let digest = hash.finalize();
        let name = digest[..16]
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        Ok(format!(r"\\.\pipe\vk-fast-instance-v1-{name}"))
    }

    fn read(stream: &mut Stream, bytes: &mut [u8]) -> io::Result<()> {
        transfer(|bytes| stream.read(bytes), bytes)
    }

    fn write(stream: &mut Stream, bytes: &[u8]) -> io::Result<()> {
        transfer(|bytes| stream.write(bytes), &mut bytes.to_vec())
    }

    /// The pipe carries a header, then for a link its u16 length and bytes.
    fn framed(message: &[u8]) -> Vec<u8> {
        if message.len() <= ACTIVATE.len() {
            return message.to_vec();
        }
        let (header, payload) = message.split_at(ACTIVATE.len());
        let length = u16::try_from(payload.len()).unwrap_or(u16::MAX);
        [header, length.to_le_bytes().as_slice(), payload].concat()
    }

    pub(super) fn activate_existing(
        mut stream: Stream,
        grant: &impl Fn(u32),
        message: &[u8],
    ) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        grant(stream.server_process_id()?);
        write(&mut stream, &framed(message))?;
        let mut ack = [0];
        read(&mut stream, &mut ack)?;
        if ack == *b"1" {
            Ok(())
        } else {
            Err(invalid("Invalid instance activation response"))
        }
    }

    pub fn acquire(
        address: &Address,
        grant: &impl Fn(u32),
        message: &[u8],
    ) -> io::Result<Acquisition> {
        let name = format!(
            "vk-fast-instance-{:x}.lock",
            Sha256::digest(address.as_bytes())
        );
        let exclusion = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(3) // Read/write sharing, with deletion excluded while held.
            .custom_flags(0x00200000) // FILE_FLAG_OPEN_REPARSE_POINT.
            .open(std::env::temp_dir().join(name))?;
        let metadata = exclusion.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid("Invalid instance lock"));
        }
        let mut owned = false;
        let deadline = Instant::now() + CONNECT_BUDGET;
        loop {
            if !owned {
                match exclusion.try_lock() {
                    Ok(()) => owned = true,
                    Err(std::fs::TryLockError::WouldBlock) => {}
                    Err(std::fs::TryLockError::Error(error)) => return Err(error),
                }
            }
            // Hold ownership across pipe reconnections. The OS releases both
            // the pipe and the file lock if the primary process crashes.
            let listener = if owned {
                PipeListenerOptions::new()
                    .path(address.as_str())
                    .create_duplex::<pipe_mode::Bytes>()
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Another instance owns activation",
                ))
            };
            match listener {
                Ok(listener) => {
                    let listener = Arc::new(listener);
                    let accept = listener.clone();
                    return primary(
                        Control {
                            _listener: listener,
                            _exclusion: exclusion,
                            address: address.clone(),
                        },
                        move |activation, stopping| {
                            while !stopping.load(Ordering::Acquire) {
                                let Ok(mut stream) = accept.accept() else {
                                    return;
                                };
                                if stopping.load(Ordering::Acquire) {
                                    return;
                                }
                                if stream.set_nonblocking(true).is_err() {
                                    continue;
                                }
                                let mut header = [0; ACTIVATE.len()];
                                if read(&mut stream, &mut header).is_err() {
                                    continue;
                                }
                                let accepted = if header.as_slice() == OPEN_LINK
                                    || header.as_slice() == TOKEN
                                {
                                    let mut length = [0; 2];
                                    let length = read(&mut stream, &mut length)
                                        .ok()
                                        .map(|()| usize::from(u16::from_le_bytes(length)))
                                        .filter(|length| {
                                            (1..=crate::link_router::MAX_URL + 512).contains(length)
                                        });
                                    length.is_some_and(|length| {
                                        let mut link = vec![0; length];
                                        read(&mut stream, &mut link).is_ok()
                                            && activation.receive(
                                                &[header.as_slice(), link.as_slice()].concat(),
                                            )
                                    })
                                } else {
                                    activation.receive(&header)
                                };
                                if accepted {
                                    let _ = write(&mut stream, b"1");
                                }
                            }
                        },
                    );
                }
                Err(bind_error) => {
                    match Stream::connect_by_path_with_wait_mode(
                        address.as_str(),
                        ConnectWaitMode::Timeout(Duration::from_millis(100)),
                    ) {
                        Ok(stream) => {
                            activate_existing(stream, grant, message)?;
                            return Ok(Acquisition::AlreadyRunning);
                        }
                        Err(_) if Instant::now() < deadline => (),
                        Err(_) => return Err(bind_error),
                    }
                }
            }
            thread::sleep(RETRY);
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
compile_error!("Single-instance transport supports Linux and Windows");

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::atomic::AtomicUsize,
    };

    struct Namespace(PathBuf);
    impl Namespace {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "vk-fast-instance-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path)
        }
        fn address(&self) -> platform::Address {
            address(self.0.to_str().unwrap())
        }
    }
    impl Drop for Namespace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn address(value: &str) -> platform::Address {
        #[cfg(target_os = "linux")]
        {
            PathBuf::from(value)
        }
        #[cfg(windows)]
        {
            format!(
                r"\\.\pipe\vk-fast-test-{}",
                value.replace(['\\', '/', ':', ' '], "-")
            )
        }
    }

    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "Instance test timed out");
            thread::sleep(RETRY);
        }
    }

    fn primary_at(namespace: &Namespace) -> Primary {
        match platform::acquire(&namespace.address(), &|_| {}, ACTIVATE).unwrap() {
            Acquisition::Primary(primary) => primary,
            Acquisition::AlreadyRunning => panic!("Unexpected existing test instance"),
        }
    }

    struct Process(Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn child(namespace: &Namespace, action: &str) -> Process {
        Process(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "single_instance::tests::process_helper",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VK_FAST_INSTANCE_TEST_DIRECTORY", &namespace.0)
                .env("VK_FAST_INSTANCE_TEST_ACTION", action)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    #[ignore = "isolated subprocess entry point; invoked by single-instance tests"]
    fn process_helper() {
        let directory = PathBuf::from(
            std::env::var_os("VK_FAST_INSTANCE_TEST_DIRECTORY").expect("test directory"),
        );
        let action = std::env::var("VK_FAST_INSTANCE_TEST_ACTION").unwrap();
        let link = "https://vk.ru/music/album/-2000_1_ab";
        let message = message(
            matches!(action.as_str(), "link" | "token").then_some(link),
            (action == "token").then_some("launcher-token_1"),
        );
        let acquired =
            platform::acquire(&address(directory.to_str().unwrap()), &|_| {}, &message).unwrap();
        match (action.as_str(), acquired) {
            ("signal" | "link" | "token", Acquisition::AlreadyRunning) => (),
            ("hold", Acquisition::Primary(_primary)) => {
                fs::write(directory.join("ready"), b"ready").unwrap();
                loop {
                    thread::park();
                }
            }
            _ => panic!("Unexpected acquisition result"),
        }
    }

    #[test]
    fn another_process_activates_once_before_a_window_exists_and_exits() {
        let namespace = Namespace::new();
        let primary = primary_at(&namespace);
        let activation = primary.activations();
        let mut child = child(&namespace, "signal");
        wait(|| child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait().unwrap().success());
        wait(|| activation.0.pending.load(Ordering::Acquire));
        assert!(activation.take_pending());
        assert!(!activation.take_pending());
        drop(primary);
        drop(primary_at(&namespace));
    }

    #[test]
    fn a_second_launch_hands_its_link_to_the_running_instance() {
        let namespace = Namespace::new();
        let primary = primary_at(&namespace);
        let activation = primary.activations();
        let mut child = child(&namespace, "link");
        wait(|| child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait().unwrap().success());
        wait(|| activation.0.pending.load(Ordering::Acquire));
        assert!(activation.take_pending());
        assert_eq!(
            activation.take_links(),
            ["https://vk.ru/music/album/-2000_1_ab"]
        );
        assert!(activation.take_links().is_empty());
        assert_eq!(activation.take_token(), None);
        // A launcher's activation token travels with the link.
        let mut child = self::child(&namespace, "token");
        wait(|| child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait().unwrap().success());
        wait(|| activation.0.pending.load(Ordering::Acquire));
        assert!(activation.take_pending());
        assert_eq!(activation.take_token().as_deref(), Some("launcher-token_1"));
        assert_eq!(
            activation.take_links(),
            ["https://vk.ru/music/album/-2000_1_ab"]
        );
        assert!(!activation.receive(b"VKFAST_ACTTOKEN_V1bad token\n"));
        assert!(activation.receive(b"VKFAST_ACTTOKEN_V1ok\n"));
        assert_eq!(activation.take_token().as_deref(), Some("ok"));
        assert!(activation.take_links().is_empty());
        assert_eq!(activation.take_token(), None);
        // A launcher's activation token travels with the link.
        let mut child = self::child(&namespace, "token");
        wait(|| child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait().unwrap().success());
        wait(|| activation.0.pending.load(Ordering::Acquire));
        assert!(activation.take_pending());
        assert_eq!(activation.take_token().as_deref(), Some("launcher-token_1"));
        assert_eq!(
            activation.take_links(),
            ["https://vk.ru/music/album/-2000_1_ab"]
        );
        assert!(!activation.receive(b"VKFAST_ACTTOKEN_V1bad token\n"));
        assert!(activation.receive(b"VKFAST_ACTTOKEN_V1ok\n"));
        assert_eq!(activation.take_token().as_deref(), Some("ok"));
        assert!(activation.take_links().is_empty());
        // Unknown or oversized messages are ignored; the queue stays bounded.
        assert!(!activation.receive(b"VKFAST_OPENLINK_V1"));
        assert!(!activation.receive(b"hello"));
        for index in 0..20 {
            activation.open_link(&format!("https://vk.ru/audio1_{}", index + 1));
        }
        let links = activation.take_links();
        assert_eq!(links.len(), MAX_LINKS);
        assert_eq!(links.last().unwrap(), "https://vk.ru/audio1_20");
        activation.open_link(&"a".repeat(crate::link_router::MAX_URL + 1));
        assert!(activation.take_links().is_empty());
        drop(primary);
    }

    #[test]
    fn a_crashed_primary_releases_exclusion_and_stale_endpoint_is_recovered() {
        let namespace = Namespace::new();
        let mut child = child(&namespace, "hold");
        wait(|| namespace.0.join("ready").exists());
        assert!(matches!(
            platform::acquire(&namespace.address(), &|_| {}, ACTIVATE).unwrap(),
            Acquisition::AlreadyRunning
        ));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        drop(primary_at(&namespace));
    }

    #[test]
    fn attaching_a_repaint_target_wakes_an_activation_queued_during_startup() {
        let activation = Activation::default();
        activation.notify();
        let context = eframe::egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        context.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        activation.set_repaint_target(RepaintTarget::new(&context));
        assert!(calls.load(Ordering::Relaxed) > 0);
        assert!(activation.take_pending());
        assert!(!activation.take_pending());
    }

    #[test]
    fn nonblocking_pipe_transfer_retries_zero_and_partial_progress_without_duplication() {
        let mut calls = 0;
        let mut received = [0; 4];
        transfer(
            |remaining| {
                calls += 1;
                match calls {
                    1 => Ok(0),
                    2 => Err(io::ErrorKind::WouldBlock.into()),
                    3 => {
                        remaining[0] = b'V';
                        Ok(1)
                    }
                    4 => {
                        remaining.copy_from_slice(b"KOK");
                        Ok(3)
                    }
                    _ => panic!("Bytes were transferred more than once"),
                }
            },
            &mut received,
        )
        .unwrap();
        assert_eq!(&received, b"VKOK");
        assert_eq!(calls, 4);
    }

    #[test]
    fn an_unresponsive_pipe_peer_is_bounded_and_cannot_block_shutdown_forever() {
        let start = Instant::now();
        let error = transfer(|_| Ok(0), &mut [0]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn activation_wakes_the_rebound_headless_or_recreated_window_context() {
        let original = eframe::egui::Context::default();
        let repaint = RepaintTarget::new(&original);
        let activation = Activation::default();
        activation.set_repaint_target(repaint.clone());
        original.request_repaint();
        let current = eframe::egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        current.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        repaint.set_context(&current);
        activation.notify();
        assert!(calls.load(Ordering::Relaxed) > 0);
        assert!(activation.take_pending());
        assert!(!activation.take_pending());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn second_launch_waits_for_the_socket_when_the_first_holds_the_startup_lock() {
        use std::{
            fs::OpenOptions,
            os::unix::{fs::OpenOptionsExt, net::UnixDatagram},
        };
        let namespace = Namespace::new();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(namespace.0.join("instance.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        let mut child = child(&namespace, "signal");
        // The missing endpoint cannot make the competing process become a
        // second primary or exit before its activation can be delivered.
        thread::sleep(Duration::from_millis(60));
        assert!(child.0.try_wait().unwrap().is_none());
        let socket = UnixDatagram::bind(namespace.0.join("instance.sock")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut message = [0; 64];
        let length = socket.recv(&mut message).unwrap();
        assert_eq!(&message[..length], ACTIVATE);
        wait(|| child.0.try_wait().unwrap().is_some());
        assert!(child.0.wait().unwrap().success());
    }
}
