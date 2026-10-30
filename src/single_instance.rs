//! A user-local activation channel, acquired before any application state opens.
//! The guard survives native-window recreation; activation survives UI startup.
use std::{
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
const CONNECT_BUDGET: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_millis(10);

pub enum Acquisition {
    Primary(Primary),
    AlreadyRunning,
}

#[derive(Default)]
struct ActivationState {
    pending: AtomicBool,
    repaint: Mutex<Option<RepaintTarget>>,
}

/// Clone this into the resident app; keep the Primary guard in main.
#[derive(Clone, Default)]
pub struct Activation(Arc<ActivationState>);

impl Activation {
    pub fn take_pending(&self) -> bool {
        self.0.pending.swap(false, Ordering::AcqRel)
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
pub fn acquire(grant_foreground: impl Fn(u32)) -> io::Result<Acquisition> {
    platform::acquire(&platform::default_address()?, &grant_foreground)
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

    pub fn acquire(address: &Address, _: &impl Fn(u32)) -> io::Result<Acquisition> {
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
                            let mut message = [0_u8; 64];
                            while !stopping.load(Ordering::Acquire) {
                                match receive.recv(&mut message) {
                                    Ok(length) if &message[..length] == ACTIVATE => {
                                        if !stopping.load(Ordering::Acquire) {
                                            activation.notify();
                                        }
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
                    match sender.send_to(ACTIVATE, &path) {
                        Ok(length) if length == ACTIVATE.len() => {
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
    use std::io::{Read, Write};

    type Stream = DuplexPipeStream<pipe_mode::Bytes>;
    type Listener = PipeListener<pipe_mode::Bytes, pipe_mode::Bytes>;
    pub type Address = String;

    pub struct Control {
        _listener: Arc<Listener>,
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

    pub(super) fn activate_existing(mut stream: Stream, grant: &impl Fn(u32)) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        grant(stream.server_process_id()?);
        write(&mut stream, ACTIVATE)?;
        let mut ack = [0];
        read(&mut stream, &mut ack)?;
        if ack == *b"1" {
            Ok(())
        } else {
            Err(invalid("Invalid instance activation response"))
        }
    }

    pub fn acquire(address: &Address, grant: &impl Fn(u32)) -> io::Result<Acquisition> {
        let deadline = Instant::now() + CONNECT_BUDGET;
        loop {
            // Interprocess sets FILE_FLAG_FIRST_PIPE_INSTANCE for initial
            // creation. The kernel is the lock; a crashed process leaves none.
            // Contract: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-createnamedpipea
            match PipeListenerOptions::new()
                .path(address.as_str())
                .create_duplex::<pipe_mode::Bytes>()
            {
                Ok(listener) => {
                    let listener = Arc::new(listener);
                    let accept = listener.clone();
                    return primary(
                        Control {
                            _listener: listener,
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
                                let mut message = [0; ACTIVATE.len()];
                                if read(&mut stream, &mut message).is_ok() && message == ACTIVATE {
                                    activation.notify();
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
                            activate_existing(stream, grant)?;
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
        match platform::acquire(&namespace.address(), &|_| {}).unwrap() {
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
        let acquired = platform::acquire(&address(directory.to_str().unwrap()), &|_| {}).unwrap();
        match (action.as_str(), acquired) {
            ("signal", Acquisition::AlreadyRunning) => (),
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
    fn a_crashed_primary_releases_exclusion_and_stale_endpoint_is_recovered() {
        let namespace = Namespace::new();
        let mut child = child(&namespace, "hold");
        wait(|| namespace.0.join("ready").exists());
        assert!(matches!(
            platform::acquire(&namespace.address(), &|_| {}).unwrap(),
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
