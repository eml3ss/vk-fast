//! The helper is a copy of the running, trusted executable. It waits for the
//! parent process to exit, replaces only program files and rolls back on error.
use super::*;
use std::{fs::OpenOptions, sync::Mutex, thread, time::Instant};

// Statics are intentionally not dropped by Rust at normal shutdown. Windows
// releases this lock when the *process* exits, after audio, saves and the
// single-instance guard have all finished. A PID delay would race those tasks.
static PARENT_LOCK: Mutex<Option<File>> = Mutex::new(None);

pub fn launch(mut stage: StagedUpdate, english: bool) -> Result<(), Error> {
    let mut job = read_job(&stage.path)?;
    job.english = english;
    write_job(&stage.path, &job)?;
    let lock = File::create_new(stage.path.join("parent.lock"))?;
    lock.try_lock().map_err(|_| Error::Busy)?;
    {
        let mut parent = PARENT_LOCK.lock().map_err(|_| Error::Helper)?;
        if parent.is_some() {
            return Err(Error::Busy);
        }
        *parent = Some(lock);
    }
    let result = (|| {
        let mut command = std::process::Command::new(stage.path.join("runner.exe"));
        command
            .arg("--apply-update")
            .arg(&stage.path)
            .current_dir(&stage.path);
        hide_console(&mut command);
        let mut child = command.spawn().map_err(|_| Error::Helper)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if marker_matches(&stage.path.join("ready"), &job.nonce) {
                return Ok(());
            }
            if child.try_wait().map_err(|_| Error::Helper)?.is_some() {
                return Err(Error::Helper);
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(Error::Helper)
    })();
    if result.is_ok() {
        stage.handed_off = true;
    } else if let Ok(mut parent) = PARENT_LOCK.lock() {
        *parent = None;
    }
    result
}

fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(windows)]
pub fn run_helper(stage: &Path) -> Result<(), Error> {
    run_helper_with(stage, restart_and_confirm)
}

#[cfg(windows)]
fn run_helper_with(
    stage: &Path,
    restart: impl FnOnce(&Path, &Job) -> Result<(), Error>,
) -> Result<(), Error> {
    let stage = fs::canonicalize(stage)?;
    if fs::canonicalize(std::env::current_exe()?)? != stage.join("runner.exe") {
        return Err(Error::Helper);
    }
    let job = read_job(&stage)?;
    let helper = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(stage.join("helper.lock"))?;
    helper.try_lock().map_err(|_| Error::Busy)?;
    let parent = OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("parent.lock"))?;
    write_marker(&stage.join("ready"), &job.nonce)?;
    wait_for_parent(&parent, Duration::from_secs(60))?;
    let program_lock_path = job.install_dir.join(".vk-fast-update.lock");
    if program_lock_path.exists() {
        plain(&program_lock_path, false)?;
    }
    let program_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&program_lock_path)?;
    program_lock.try_lock().map_err(|_| Error::Busy)?;
    let result = apply(&stage, &job, |job| restart(&stage, job));
    if result.is_ok() {
        update_installer_version(&job);
    }
    drop(program_lock);
    let _ = fs::remove_file(program_lock_path);
    if result.is_err() && result != Err(Error::Rollback) {
        // This is our own previous executable, never an arbitrary command or
        // one taken from release notes. User data and storage selection remain.
        let mut command = std::process::Command::new(job.install_dir.join("vk-fast.exe"));
        command.current_dir(&job.install_dir);
        hide_console(&mut command);
        let _ = command.spawn();
    }
    result
}

#[cfg(windows)]
fn restart_and_confirm(stage: &Path, job: &Job) -> Result<(), Error> {
    let mut command = std::process::Command::new(job.install_dir.join("vk-fast.exe"));
    command
        .arg("--update-complete")
        .arg(stage)
        .current_dir(&job.install_dir);
    hide_console(&mut command);
    let mut child = command.spawn().map_err(|_| Error::Helper)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if marker_matches(&stage.join("restarted"), &job.nonce) {
            return Ok(());
        }
        if child.try_wait().map_err(|_| Error::Helper)?.is_some() {
            return Err(Error::Helper);
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(Error::Helper)
}

#[cfg(windows)]
pub fn complete_restart(stage: &Path) -> Result<(), Error> {
    let stage = fs::canonicalize(stage)?;
    let job = read_job(&stage)?;
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    if executable != job.install_dir.join("vk-fast.exe")
        || job.version != env!("CARGO_PKG_VERSION")
        || hash_file(&executable, MAX_EXECUTABLE)? != job.executable
    {
        return Err(Error::Integrity);
    }
    write_marker(&stage.join("restarted"), &job.nonce)?;
    // The helper still holds its own executable and backup files. Cleanup waits
    // for its lock, and retries deletion after the Windows image is unmapped.
    let _ = thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(move || {
            for _ in 0..100 {
                if let Ok(lock) = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(stage.join("helper.lock"))
                    && lock.try_lock().is_ok()
                {
                    drop(lock);
                    if fs::remove_dir_all(&stage).is_ok() {
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
    Ok(())
}

#[cfg(windows)]
fn update_installer_version(job: &Job) {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_64KEY},
    };
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\VKFast",
        KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
    ) else {
        return;
    };
    let Ok(location) = key.get_value::<String, _>("InstallLocation") else {
        return;
    };
    if fs::canonicalize(location).ok().as_ref() == Some(&job.install_dir) {
        let _ = key.set_value("DisplayVersion", &job.version);
    }
}

#[cfg(windows)]
pub fn report_error(stage: &Path, error: Error) {
    let language = if read_job(stage).is_ok_and(|job| job.english) {
        crate::i18n::Language::English
    } else {
        crate::i18n::Language::Russian
    };
    rfd::MessageDialog::new()
        .set_title("VK Fast")
        .set_description(error.message(language))
        .set_level(rfd::MessageLevel::Error)
        .show();
}

#[cfg(any(windows, test))]
fn wait_for_parent(lock: &File, timeout: Duration) -> Result<(), Error> {
    let deadline = Instant::now() + timeout;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            _ => return Err(Error::Busy),
        }
    }
}

fn marker_matches(path: &Path, nonce: &str) -> bool {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(64).read_to_end(&mut bytes))
        .is_ok()
        && bytes == nonce.as_bytes()
}

#[cfg(any(windows, test))]
fn write_marker(path: &Path, nonce: &str) -> Result<(), Error> {
    let mut file = File::create_new(path)?;
    file.write_all(nonce.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

#[cfg(any(windows, test))]
struct Replacement {
    target: PathBuf,
    pending: PathBuf,
    backup: PathBuf,
    digest: String,
    previous_digest: String,
    backed_up: bool,
    installed: bool,
    pending_owned: bool,
}

#[cfg(any(windows, test))]
impl Replacement {
    fn stage(
        source: &Path,
        target: PathBuf,
        digest: &str,
        previous_digest: &str,
        nonce: &str,
    ) -> Result<Self, Error> {
        plain(&target, false)?;
        let folder = target.parent().ok_or(Error::Disk)?;
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::Disk)?;
        let pending = folder.join(format!(".{name}-{nonce}.new"));
        let backup = folder.join(format!(".{name}-{nonce}.previous"));
        if backup.try_exists()? {
            return Err(Error::Changed);
        }
        let mut replacement = Self {
            target,
            pending,
            backup,
            digest: digest.into(),
            previous_digest: previous_digest.into(),
            backed_up: false,
            installed: false,
            pending_owned: false,
        };
        let mut from = File::open(source)?;
        let mut to = File::create_new(&replacement.pending)?;
        replacement.pending_owned = true;
        if io::copy(&mut (&mut from).take(MAX_EXECUTABLE + 1), &mut to)? > MAX_EXECUTABLE {
            return Err(Error::Integrity);
        }
        to.sync_all()?;
        drop(to);
        if hash_file(&replacement.pending, MAX_EXECUTABLE)? != digest {
            return Err(Error::Integrity);
        }
        Ok(replacement)
    }

    fn replace(&mut self) -> Result<(), Error> {
        // Keep the destination present throughout the update. Each final rename
        // atomically replaces an existing file; a crash cannot leave a gap
        // between moving the executable away and moving the new one into place.
        let mut backup = File::create_new(&self.backup)?;
        let copied = (|| -> Result<(), Error> {
            plain(&self.target, false)?;
            if io::copy(
                &mut File::open(&self.target)?.take(MAX_EXECUTABLE + 1),
                &mut backup,
            )? > MAX_EXECUTABLE
            {
                return Err(Error::Changed);
            }
            backup.sync_all()?;
            Ok(())
        })();
        drop(backup);
        if let Err(error) = copied {
            let _ = fs::remove_file(&self.backup);
            return Err(error);
        }
        if hash_file(&self.backup, MAX_EXECUTABLE)? != self.previous_digest {
            let _ = fs::remove_file(&self.backup);
            return Err(Error::Changed);
        }
        self.backed_up = true;
        retry_rename(&self.pending, &self.target)?;
        self.installed = true;
        self.pending_owned = false;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        if self.installed {
            if hash_file(&self.target, MAX_EXECUTABLE)? != self.digest {
                return Err(Error::Rollback);
            }
            retry_rename(&self.backup, &self.target).map_err(|_| Error::Rollback)?;
            self.backed_up = false;
            self.installed = false;
        } else if self.backed_up {
            fs::remove_file(&self.backup).map_err(|_| Error::Rollback)?;
            self.backed_up = false;
        }
        Ok(())
    }
}

#[cfg(any(windows, test))]
impl Drop for Replacement {
    fn drop(&mut self) {
        if self.pending_owned {
            let _ = fs::remove_file(&self.pending);
        }
        // Never delete a recovery backup while unwinding or after a failed
        // rollback. Only a confirmed restart explicitly removes old files.
    }
}

#[cfg(any(windows, test))]
fn retry_rename(source: &Path, target: &Path) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
                ) && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(100))
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(Error::Busy);
            }
            Err(_) => return Err(Error::Disk),
        }
    }
}

#[cfg(any(windows, test))]
fn apply(
    stage: &Path,
    job: &Job,
    restart: impl FnOnce(&Job) -> Result<(), Error>,
) -> Result<(), Error> {
    let exe = job.install_dir.join("vk-fast.exe");
    plain(&job.install_dir, true)?;
    if hash_file(&exe, MAX_EXECUTABLE)? != job.previous_exe {
        return Err(Error::Changed);
    }
    if hash_file(&stage.join("next-vk-fast.exe"), MAX_EXECUTABLE)? != job.executable {
        return Err(Error::Integrity);
    }
    let mut executable = Replacement::stage(
        &stage.join("next-vk-fast.exe"),
        exe,
        &job.executable,
        &job.previous_exe,
        &job.nonce,
    )?;
    let result = (|| {
        executable.replace()?;
        restart(job)
    })();
    if let Err(error) = result {
        let restored_exe = executable.rollback();
        if restored_exe.is_err() {
            return Err(Error::Rollback);
        }
        return Err(error);
    }
    let _ = fs::remove_file(executable.backup.clone());
    Ok(())
}

#[cfg(test)]
mod tests;
