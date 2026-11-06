use super::*;
use crate::updater::tests::pe;

struct Fixture {
    stage: StagedUpdate,
    job: Job,
    install_root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let stage = StagedUpdate::new().unwrap();
        let install_root = stage.path.with_file_name(format!(
            "VK-Fast-install-test-{}",
            stage_nonce(&stage.path).unwrap()
        ));
        let install_dir = install_root.join("Installed VK Fast тест");
        fs::create_dir_all(install_dir.join("bin")).unwrap();
        for folder in ["config", "cache", "config/webview"] {
            fs::create_dir_all(install_dir.join(folder)).unwrap();
        }
        for (path, bytes) in [
            ("vk-fast.exe", pe("old application")),
            ("bin/ffmpeg.exe", pe("old decoder")),
            ("config/session.json", b"synthetic private login".to_vec()),
            ("config/preferences.json", b"synthetic preferences".to_vec()),
            ("cache/audio.bin", b"synthetic audio cache".to_vec()),
            (
                "vk-fast-storage.ini",
                b"[storage]\r\nversion=1\r\nconfig=appdata\r\ncache=portable\r\n".to_vec(),
            ),
            ("Uninstall.exe", b"old installer uninstaller".to_vec()),
        ] {
            fs::write(install_dir.join(path), bytes).unwrap();
        }
        fs::write(stage.path.join("next-vk-fast.exe"), pe("new application")).unwrap();
        fs::write(stage.path.join("next-ffmpeg.exe"), pe("new decoder")).unwrap();
        let job = Job {
            schema: 1,
            nonce: stage_nonce(&stage.path).unwrap(),
            version: "0.1.2".into(),
            english: false,
            previous_exe: hash_file(&install_dir.join("vk-fast.exe"), MAX_EXECUTABLE).unwrap(),
            previous_decoder: hash_file(&install_dir.join("bin/ffmpeg.exe"), MAX_DECODER).unwrap(),
            executable: hash_file(&stage.path.join("next-vk-fast.exe"), MAX_EXECUTABLE).unwrap(),
            decoder: hash_file(&stage.path.join("next-ffmpeg.exe"), MAX_DECODER).unwrap(),
            install_dir,
        };
        write_job(&stage.path, &job).unwrap();
        Self {
            stage,
            job,
            install_root,
        }
    }

    fn data(&self) -> Vec<Vec<u8>> {
        [
            "config/session.json",
            "config/preferences.json",
            "cache/audio.bin",
            "vk-fast-storage.ini",
            "Uninstall.exe",
        ]
        .iter()
        .map(|path| fs::read(self.job.install_dir.join(path)).unwrap())
        .collect()
    }
    fn assert_old(&self) {
        assert_eq!(
            hash_file(&self.job.install_dir.join("vk-fast.exe"), MAX_EXECUTABLE).unwrap(),
            self.job.previous_exe
        );
        assert_eq!(
            hash_file(&self.job.install_dir.join("bin/ffmpeg.exe"), MAX_DECODER).unwrap(),
            self.job.previous_decoder
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.install_root);
    }
}

#[test]
fn successful_update_replaces_both_files_preserves_all_data_and_calls_restart_once() {
    let fixture = Fixture::new();
    let before = fixture.data();
    let mut calls = 0;
    apply(&fixture.stage.path, &fixture.job, |job| {
        calls += 1;
        assert_eq!(
            hash_file(&job.install_dir.join("vk-fast.exe"), MAX_EXECUTABLE).unwrap(),
            job.executable
        );
        assert_eq!(
            hash_file(&job.install_dir.join("bin/ffmpeg.exe"), MAX_DECODER).unwrap(),
            job.decoder
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(fixture.data(), before);
    assert!(
        !fs::read_dir(&fixture.job.install_dir)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".previous"))
    );
}

#[test]
fn failed_new_process_start_restores_both_old_executables_and_keeps_data() {
    let fixture = Fixture::new();
    let before = fixture.data();
    assert_eq!(
        apply(&fixture.stage.path, &fixture.job, |_| Err(Error::Helper)),
        Err(Error::Helper)
    );
    fixture.assert_old();
    assert_eq!(fixture.data(), before);
}

#[test]
fn changed_installation_or_tampered_staging_never_replaces_the_current_version() {
    let fixture = Fixture::new();
    let before = fixture.data();
    fs::write(fixture.stage.path.join("next-vk-fast.exe"), pe("tampered")).unwrap();
    assert_eq!(
        apply(&fixture.stage.path, &fixture.job, |_| panic!(
            "must not start"
        )),
        Err(Error::Integrity)
    );
    fixture.assert_old();
    assert_eq!(fixture.data(), before);
    let fixture = Fixture::new();
    let changed = pe("independently updated");
    fs::write(fixture.job.install_dir.join("vk-fast.exe"), &changed).unwrap();
    assert_eq!(
        apply(&fixture.stage.path, &fixture.job, |_| panic!(
            "must not start"
        )),
        Err(Error::Changed)
    );
    assert_eq!(
        fs::read(fixture.job.install_dir.join("vk-fast.exe")).unwrap(),
        changed
    );
}

#[test]
fn failed_second_replacement_rolls_back_an_already_replaced_decoder() {
    let fixture = Fixture::new();
    let before = fixture.data();
    let mut executable = Replacement::stage(
        &fixture.stage.path.join("next-vk-fast.exe"),
        fixture.job.install_dir.join("vk-fast.exe"),
        &fixture.job.executable,
        &fixture.job.previous_exe,
        &fixture.job.nonce,
    )
    .unwrap();
    let mut audio = Replacement::stage(
        &fixture.stage.path.join("next-ffmpeg.exe"),
        fixture.job.install_dir.join("bin/ffmpeg.exe"),
        &fixture.job.decoder,
        &fixture.job.previous_decoder,
        &fixture.job.nonce,
    )
    .unwrap();
    audio.replace().unwrap();
    fs::remove_file(&executable.pending).unwrap();
    assert!(executable.replace().is_err());
    executable.rollback().unwrap();
    audio.rollback().unwrap();
    fixture.assert_old();
    assert_eq!(fixture.data(), before);
}

#[test]
fn helper_waits_for_the_process_lock_instead_of_an_arbitrary_delay() {
    let stage = StagedUpdate::new().unwrap();
    let first = File::create_new(stage.path.join("parent.lock")).unwrap();
    first.lock().unwrap();
    let second = OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.path.join("parent.lock"))
        .unwrap();
    assert_eq!(
        wait_for_parent(&second, Duration::from_millis(1)),
        Err(Error::Busy)
    );
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        drop(first);
    });
    wait_for_parent(&second, Duration::from_secs(2)).unwrap();
    release.join().unwrap();
}

#[test]
fn bounded_job_and_markers_reject_wrong_schema_nonce_and_unknown_commands() {
    let fixture = Fixture::new();
    let path = fixture.stage.path.join("job.json");
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (field, value) in [
        ("schema", serde_json::json!(2)),
        ("nonce", serde_json::json!("wrong")),
        ("command", serde_json::json!("not allowed")),
        ("executable", serde_json::json!("bad hash")),
    ] {
        let mut json = original.clone();
        json[field] = value;
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(read_job(&fixture.stage.path).is_err());
    }
    fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap();
    assert!(read_job(&fixture.stage.path).is_err());
    let marker = fixture.stage.path.join("ready");
    write_marker(&marker, &fixture.job.nonce).unwrap();
    assert!(marker_matches(&marker, &fixture.job.nonce));
    assert!(!marker_matches(&marker, "a different job"));
}

#[cfg(unix)]
#[test]
fn linked_program_files_or_staging_are_never_followed() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let target = fixture.job.install_dir.join("bin/ffmpeg.exe");
    let outside = fixture.stage.path.join("outside.exe");
    fs::rename(&target, &outside).unwrap();
    symlink(&outside, &target).unwrap();
    assert!(
        apply(&fixture.stage.path, &fixture.job, |_| panic!(
            "must not start"
        ))
        .is_err()
    );
    assert_eq!(
        hash_file(&outside, MAX_DECODER).unwrap(),
        fixture.job.previous_decoder
    );
    assert!(target.is_symlink());
}

// Run this probe on Windows (or in an isolated Wine prefix). No account, GUI,
// session, device or live network is involved. All executables are copies of
// this test binary under its unique temporary directory.
#[cfg(windows)]
#[test]
fn native_helper_process_waits_replaces_restarts_and_preserves_unicode_install_data() {
    use std::process::{Command, Stdio};
    let mut fixture = Fixture::new();
    let before = fixture.data();
    let current = std::env::current_exe().unwrap();
    let mut old = fs::read(&current).unwrap();
    old.extend_from_slice(b"old test version");
    let mut new = fs::read(&current).unwrap();
    new.extend_from_slice(b"new test version");
    fs::write(fixture.job.install_dir.join("vk-fast.exe"), &old).unwrap();
    fs::write(fixture.stage.path.join("next-vk-fast.exe"), &new).unwrap();
    fixture.job.previous_exe = hex_hash(&Sha256::digest(&old));
    fixture.job.executable = hex_hash(&Sha256::digest(&new));
    fixture.job.version = env!("CARGO_PKG_VERSION").into();
    write_job(&fixture.stage.path, &fixture.job).unwrap();
    fs::copy(&current, fixture.stage.path.join("runner.exe")).unwrap();
    let parent = File::create_new(fixture.stage.path.join("parent.lock")).unwrap();
    parent.lock().unwrap();
    let mut child = Command::new(fixture.stage.path.join("runner.exe"))
        .args([
            "--exact",
            "updater::install::tests::native_helper_child",
            "--ignored",
            "--nocapture",
        ])
        .env("VK_FAST_UPDATE_PROBE_STAGE", &fixture.stage.path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker_matches(&fixture.stage.path.join("ready"), &fixture.job.nonce) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "helper exited before readiness"
        );
        assert!(Instant::now() < deadline, "helper did not become ready");
        thread::sleep(Duration::from_millis(50));
    }
    fixture.assert_old();
    drop(parent);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "helper did not finish");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        hash_file(&fixture.job.install_dir.join("vk-fast.exe"), MAX_EXECUTABLE).unwrap(),
        fixture.job.executable
    );
    assert_eq!(
        hash_file(&fixture.job.install_dir.join("bin/ffmpeg.exe"), MAX_DECODER).unwrap(),
        fixture.job.decoder
    );
    assert_eq!(fixture.data(), before);
}

#[cfg(windows)]
#[test]
#[ignore = "Child process of the isolated native updater probe"]
fn native_helper_child() {
    use std::process::Command;
    let stage = PathBuf::from(std::env::var_os("VK_FAST_UPDATE_PROBE_STAGE").unwrap());
    run_helper_with(&stage, |stage, job| {
        let mut child = Command::new(job.install_dir.join("vk-fast.exe"))
            .args([
                "--exact",
                "updater::install::tests::native_restart_child",
                "--ignored",
                "--nocapture",
            ])
            .env("VK_FAST_UPDATE_PROBE_STAGE", stage)
            .spawn()
            .map_err(|_| Error::Helper)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker_matches(&stage.join("restarted"), &job.nonce) {
            if child.try_wait().map_err(|_| Error::Helper)?.is_some() {
                return Err(Error::Helper);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err(Error::Helper);
            }
            thread::sleep(Duration::from_millis(50));
        }
        if !child.wait().map_err(|_| Error::Helper)?.success() {
            return Err(Error::Helper);
        }
        Ok(())
    })
    .unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "Child process of the isolated native updater probe"]
fn native_restart_child() {
    let stage = PathBuf::from(std::env::var_os("VK_FAST_UPDATE_PROBE_STAGE").unwrap());
    complete_restart(&stage).unwrap();
    // Keep the Windows image alive until its parent has observed the ACK.
    thread::sleep(Duration::from_millis(200));
}
