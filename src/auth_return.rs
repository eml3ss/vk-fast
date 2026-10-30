//! Deliver a native OAuth return to the waiting process without a TCP listener.
//! OAuth state/PKCE validation belongs to browser_auth; this module only checks
//! the native URI envelope. Callback URLs are never logged or written to disk.

use std::io;

const MAX_BYTES: usize = 16 * 1024;

fn valid_url(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_BYTES
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
        && reqwest::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "vkfast"
                && url.host_str() == Some("oauth")
                && url.path() == "/callback"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.fragment().is_none()
                && url.query().is_some_and(|query| !query.is_empty())
        })
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid VK Fast authorization return",
    )
}

#[cfg(unix)]
mod platform {
    use super::{MAX_BYTES, invalid, valid_url};
    use std::{
        fs::{self, File, OpenOptions},
        io,
        os::unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixDatagram,
        },
        path::{Path, PathBuf},
    };
    use zeroize::Zeroizing;

    const SOCKET_NAME: &str = "auth-return.sock";
    const LOCK_NAME: &str = ".auth-return.lock";

    // Deliberately no Debug: a future transport implementation may hold a return.
    pub struct ReturnListener {
        socket: UnixDatagram,
        _lock: File,
        path: PathBuf,
        device: u64,
        inode: u64,
    }

    fn private_error() -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Authorization return directory is not private",
        )
    }

    fn current_uid() -> io::Result<u32> {
        #[cfg(target_os = "linux")]
        {
            // procfs reports this process's owner without unsafe FFI or trusting
            // mutable UID/USER/HOME environment variables.
            Ok(fs::metadata("/proc/self")?.uid())
        }
        #[cfg(not(target_os = "linux"))]
        {
            use std::process::{Command, Stdio};
            let output = Command::new("/usr/bin/id")
                .arg("-u")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()?;
            if !output.status.success() || output.stdout.len() > 32 {
                return Err(private_error());
            }
            std::str::from_utf8(&output.stdout)
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .ok_or_else(private_error)
        }
    }

    fn check_dir(path: &Path, uid: u32, strict: bool) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || (strict && metadata.permissions().mode() & 0o077 != 0)
        {
            return Err(private_error());
        }
        Ok(())
    }

    fn private_dir(path: &Path, uid: u32, create: bool) -> io::Result<()> {
        if create {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
            // Check ownership/type before changing the permissions of anything.
            check_dir(path, uid, false)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        check_dir(path, uid, true)
    }

    fn state_base() -> io::Result<PathBuf> {
        // Browser-launched helpers and terminal-launched apps may have different
        // XDG variables. One HOME-based address keeps the listener and lock
        // canonical for both processes.
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join(".local/state"))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User home directory unavailable")
            })
    }

    fn state_directory(uid: u32, create: bool) -> io::Result<PathBuf> {
        let base = state_base()?;
        if create {
            fs::create_dir_all(&base)?;
        }
        check_dir(&base, uid, false)?;
        let application = base.join("vk-fast");
        private_dir(&application, uid, create)?;
        let directory = application.join("runtime");
        private_dir(&directory, uid, create)?;
        Ok(directory)
    }

    fn socket_metadata(path: &Path, uid: u32) -> io::Result<fs::Metadata> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(private_error());
        }
        Ok(metadata)
    }

    fn acquire_lock(directory: &Path, uid: u32) -> io::Result<File> {
        let path = directory.join(LOCK_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == uid
                    && metadata.permissions().mode() & 0o077 == 0 => {}
            Ok(_) => return Err(private_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        lock.try_lock().map_err(io::Error::from)?;
        Ok(lock)
    }

    fn remove_stale_socket(path: &Path, uid: u32) -> io::Result<()> {
        let metadata = match socket_metadata(path, uid) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        // The lock covers current versions. Probe without sending any packet so
        // a live socket from an older version is never mistaken for stale data.
        let probe = UnixDatagram::unbound()?;
        probe.set_nonblocking(true)?;
        match probe.connect(path) {
            Ok(()) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "Authorization return listener is already active",
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) => {}
            Err(error) => return Err(error),
        }
        match socket_metadata(path, uid) {
            Ok(current) if current.ino() == metadata.ino() && current.dev() == metadata.dev() => {
                fs::remove_file(path)
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "Authorization return socket changed",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn bind_at(directory: &Path) -> io::Result<ReturnListener> {
        let uid = current_uid()?;
        private_dir(directory, uid, true)?;
        let lock = acquire_lock(directory, uid)?;
        let path = directory.join(SOCKET_NAME);
        remove_stale_socket(&path, uid)?;
        let socket = UnixDatagram::bind(&path)?;
        let setup = (|| {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            socket.set_nonblocking(true)?;
            socket_metadata(&path, uid)
        })();
        let metadata = match setup {
            Ok(metadata) => metadata,
            Err(error) => {
                // No credential has been received; leave no pathname behind on
                // an ordinary setup failure. Never remove a substituted file.
                if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket()) {
                    let _ = fs::remove_file(&path);
                }
                return Err(error);
            }
        };
        Ok(ReturnListener {
            socket,
            _lock: lock,
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub fn bind() -> io::Result<ReturnListener> {
        let uid = current_uid()?;
        let directory = state_directory(uid, true)?;
        bind_at(&directory)
    }

    impl ReturnListener {
        /// One bounded datagram per call. A missing return is normal while the
        /// user is approving access in the browser.
        pub fn try_receive(&self) -> io::Result<Option<Zeroizing<String>>> {
            let mut bytes = Zeroizing::new(vec![0_u8; MAX_BYTES + 1]);
            let count = loop {
                match self.socket.recv(&mut bytes) {
                    Ok(count) => break count,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            };
            if count == 0 || count > MAX_BYTES {
                return Err(invalid());
            }
            let value = std::str::from_utf8(&bytes[..count]).map_err(|_| invalid())?;
            if !valid_url(value) {
                return Err(invalid());
            }
            Ok(Some(Zeroizing::new(value.to_owned())))
        }
    }

    impl Drop for ReturnListener {
        fn drop(&mut self) {
            if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
                metadata.file_type().is_socket()
                    && metadata.dev() == self.device
                    && metadata.ino() == self.inode
            }) {
                let _ = fs::remove_file(&self.path);
            }
            // The advisory lock remains held until after this Drop body and
            // the listener socket have been dropped. The lock file stays empty.
        }
    }

    fn deliver_at(directory: &Path, value: &str) -> io::Result<()> {
        if !valid_url(value) {
            return Err(invalid());
        }
        let uid = current_uid()?;
        private_dir(directory, uid, false)?;
        let path = directory.join(SOCKET_NAME);
        socket_metadata(&path, uid)?;
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        socket.connect(&path)?;
        if socket.send(value.as_bytes())? != value.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Authorization return was not delivered",
            ));
        }
        Ok(())
    }

    pub fn deliver(value: &str) -> io::Result<()> {
        if !valid_url(value) {
            return Err(invalid());
        }
        let uid = current_uid()?;
        let directory = state_directory(uid, false)?;
        deliver_at(&directory, value)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const CALLBACK: &str = "vkfast://oauth/callback?code=synthetic-code&device_id=synthetic-device&state=synthetic-state";
        struct Directory(PathBuf);
        impl Directory {
            fn new() -> Self {
                let mut bytes = [0_u8; 12];
                getrandom::fill(&mut bytes).unwrap();
                let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                let path = std::env::temp_dir().join(format!("vk-fast-auth-return-{suffix}"));
                fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
                Self(path)
            }
        }
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn private_round_trip_is_nonblocking_and_does_not_persist_return() {
            let directory = Directory::new();
            let listener = bind_at(&directory.0).unwrap();
            assert!(listener.try_receive().unwrap().is_none());
            deliver_at(&directory.0, CALLBACK).unwrap();
            assert_eq!(listener.try_receive().unwrap().unwrap().as_str(), CALLBACK);
            assert!(listener.try_receive().unwrap().is_none());
            assert_eq!(
                fs::metadata(directory.0.join(SOCKET_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(fs::metadata(directory.0.join(LOCK_NAME)).unwrap().len(), 0);
            drop(listener);
            assert!(!directory.0.join(SOCKET_NAME).exists());
        }

        #[test]
        fn active_listener_is_exclusive_and_stale_socket_can_be_recovered() {
            let directory = Directory::new();
            let listener = bind_at(&directory.0).unwrap();
            assert_eq!(
                bind_at(&directory.0).err().unwrap().kind(),
                io::ErrorKind::WouldBlock
            );
            deliver_at(&directory.0, CALLBACK).unwrap();
            assert!(listener.try_receive().unwrap().is_some());
            drop(listener);
            let path = directory.0.join(SOCKET_NAME);
            let stale = UnixDatagram::bind(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            drop(stale);
            let recovered = bind_at(&directory.0).unwrap();
            deliver_at(&directory.0, CALLBACK).unwrap();
            assert!(recovered.try_receive().unwrap().is_some());
        }

        #[test]
        fn live_socket_without_lock_and_replaced_socket_are_not_removed() {
            let directory = Directory::new();
            let path = directory.0.join(SOCKET_NAME);
            let old_version = UnixDatagram::bind(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                bind_at(&directory.0).err().unwrap().kind(),
                io::ErrorKind::AddrInUse
            );
            drop(old_version);
            let listener = bind_at(&directory.0).unwrap();
            fs::remove_file(&path).unwrap();
            let replacement = UnixDatagram::bind(&path).unwrap();
            drop(listener);
            assert!(path.exists());
            drop(replacement);
        }

        #[test]
        fn invalid_oversized_and_non_socket_returns_fail_without_overwriting_files() {
            let directory = Directory::new();
            let listener = bind_at(&directory.0).unwrap();
            assert!(deliver_at(&directory.0, "https://example.test/callback?code=x").is_err());
            assert!(
                deliver_at(
                    &directory.0,
                    &format!("{CALLBACK}&extra={}", "x".repeat(MAX_BYTES))
                )
                .is_err()
            );
            let sender = UnixDatagram::unbound().unwrap();
            sender.connect(directory.0.join(SOCKET_NAME)).unwrap();
            sender.send(&vec![b'x'; MAX_BYTES + 1]).unwrap();
            assert!(listener.try_receive().is_err());
            drop(listener);
            let path = directory.0.join(SOCKET_NAME);
            fs::write(&path, b"keep existing file").unwrap();
            assert!(bind_at(&directory.0).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"keep existing file");
        }

        #[test]
        fn symlink_directories_lock_files_and_sockets_are_rejected() {
            use std::os::unix::fs::symlink;
            let directory = Directory::new();
            let real = directory.0.join("real");
            let listener = bind_at(&real).unwrap();
            let alias = directory.0.join("alias");
            symlink(&real, &alias).unwrap();
            assert!(bind_at(&alias).is_err());
            assert!(deliver_at(&alias, CALLBACK).is_err());
            fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(deliver_at(&real, CALLBACK).is_err());
            fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
            drop(listener);
            let target = directory.0.join("unchanged");
            fs::write(&target, b"synthetic").unwrap();
            symlink(&target, real.join(SOCKET_NAME)).unwrap();
            assert!(bind_at(&real).is_err());
            assert!(deliver_at(&real, CALLBACK).is_err());
            fs::remove_file(real.join(SOCKET_NAME)).unwrap();
            fs::remove_file(real.join(LOCK_NAME)).unwrap();
            symlink(&target, real.join(LOCK_NAME)).unwrap();
            assert!(bind_at(&real).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"synthetic");
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io;
    use zeroize::Zeroizing;
    pub struct ReturnListener;
    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Native authorization return is unsupported on this platform",
        )
    }
    pub fn bind() -> io::Result<ReturnListener> {
        Err(unsupported())
    }
    pub fn deliver(_: &str) -> io::Result<()> {
        Err(unsupported())
    }
    impl ReturnListener {
        pub fn try_receive(&self) -> io::Result<Option<Zeroizing<String>>> {
            Err(unsupported())
        }
    }
}

pub use platform::ReturnListener;

pub fn bind() -> io::Result<ReturnListener> {
    platform::bind()
}

pub fn deliver(value: &str) -> io::Result<()> {
    if !valid_url(value) {
        return Err(invalid());
    }
    platform::deliver(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_envelope_requires_exact_scheme_host_path_and_no_controls() {
        assert!(valid_url(
            "vkfast://oauth/callback?error=access_denied&state=synthetic"
        ));
        for value in [
            "https://oauth/callback?code=x",
            "vkfast://other/callback?code=x",
            "vkfast://oauth/other?code=x",
            "vkfast://user@oauth/callback?code=x",
            "vkfast://oauth:80/callback?code=x",
            "vkfast://oauth/callback?code=x#fragment",
            "vkfast://oauth/callback",
            "vkfast://oauth/callback?code=x\n",
        ] {
            assert!(!valid_url(value));
        }
    }
}
