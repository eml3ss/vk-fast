use super::*;
use serde_json::{Value, json};
use std::io::Cursor;
use zip::{ZipWriter, write::SimpleFileOptions};

pub(super) fn release_json(version: &str) -> Value {
    let name = format!("VK-Fast-{version}-windows-x86_64.zip");
    json!({
        "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
        "body": "A small release.",
        "assets": [{ "name":name, "size":1234, "digest":format!("sha256:{}", "ab".repeat(32)),
            "state":"uploaded", "browser_download_url":format!("{RELEASES_URL}/download/v{version}/{name}") }]
    })
}

pub(crate) fn release(version: &str) -> Release {
    parse_release(
        &serde_json::to_vec(&release_json(version)).unwrap(),
        "0.1.0",
    )
    .unwrap()
    .unwrap()
}

fn parse(value: &Value, current: &str) -> Result<Option<Release>, Error> {
    parse_release(&serde_json::to_vec(value).unwrap(), current)
}

pub(super) fn pe(label: &str) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&128u32.to_le_bytes());
    bytes[128..132].copy_from_slice(b"PE\0\0");
    bytes[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
    bytes[152..154].copy_from_slice(&0x020bu16.to_le_bytes());
    bytes[256..256 + label.len()].copy_from_slice(label.as_bytes());
    bytes
}

#[test]
fn checks_numeric_stable_versions_and_never_downgrades_or_uses_prereleases() {
    assert!(parse(&release_json("0.1.10"), "0.1.9").unwrap().is_some());
    for current in ["0.1.10", "0.2.0", "1.0.0", "0.1.10+local"] {
        assert!(parse(&release_json("0.1.10"), current).unwrap().is_none());
    }
    assert!(
        parse(&release_json("0.1.10+build"), "0.1.10")
            .unwrap()
            .is_none()
    );
    for flag in ["draft", "prerelease"] {
        let mut value = release_json("0.1.2");
        value[flag] = json!(true);
        assert!(parse(&value, "0.1.1").unwrap().is_none());
    }
    assert!(
        parse(&release_json("0.1.2-beta.1"), "0.1.1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn exact_runtime_asset_and_official_repository_are_required() {
    for suffix in ["-setup.exe", "-source-and-licenses.zip", "-aarch64.zip"] {
        let mut value = release_json("0.1.2");
        value["assets"][0]["name"] = json!(format!("VK-Fast-0.1.2{suffix}"));
        assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::InvalidRelease);
    }
    for url in [
        "https://example.com/update.zip",
        "https://github.com/attacker/vk-fast/releases/download/v0.1.2/VK-Fast-0.1.2-windows-x86_64.zip",
        "http://github.com/eml3ss/vk-fast/releases/download/v0.1.2/VK-Fast-0.1.2-windows-x86_64.zip",
    ] {
        let mut value = release_json("0.1.2");
        value["assets"][0]["browser_download_url"] = json!(url);
        assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::InvalidRelease);
    }
    let mut value = release_json("0.1.2");
    let duplicate = value["assets"][0].clone();
    value["assets"].as_array_mut().unwrap().push(duplicate);
    assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::InvalidRelease);
}

#[test]
fn missing_or_invalid_github_digest_and_unfinished_or_oversize_assets_are_rejected() {
    for digest in [
        Value::Null,
        json!("md5:abc"),
        json!("sha256:xyz"),
        json!(format!("sha256:{}", "zz".repeat(32))),
    ] {
        let mut value = release_json("0.1.2");
        value["assets"][0]["digest"] = digest;
        assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::Integrity);
    }
    for size in [0, MAX_DOWNLOAD + 1, u64::MAX] {
        let mut value = release_json("0.1.2");
        value["assets"][0]["size"] = json!(size);
        assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::InvalidRelease);
    }
    let mut value = release_json("0.1.2");
    value["assets"][0]["state"] = json!("new");
    assert_eq!(parse(&value, "0.1.1").unwrap_err(), Error::InvalidRelease);
}

#[test]
fn redirects_allow_only_https_github_asset_hosts_without_embedded_credentials() {
    for url in [
        "https://github.com/a",
        "https://release-assets.githubusercontent.com/a?token=signed",
        "https://objects.githubusercontent.com/a",
    ] {
        assert!(allowed_download_url(&Url::parse(url).unwrap()));
    }
    for url in [
        "http://github.com/a",
        "https://github.com.evil.test/a",
        "https://evil.githubusercontent.com/a",
        "file:///C:/vk-fast.exe",
        "https://user:password@github.com/a",
        "https://github.com:444/a",
    ] {
        assert!(!allowed_download_url(&Url::parse(url).unwrap()));
    }
}

#[test]
fn streaming_checks_complete_size_digest_and_cancellation_before_extraction() {
    let bytes = vec![b'x'; 128 * 1024];
    let digest = Sha256::digest(&bytes).into();
    let canceled = AtomicBool::new(false);
    let mut output = Vec::new();
    let mut last = (0, 0);
    receive_archive(
        Cursor::new(&bytes),
        &mut output,
        bytes.len() as u64,
        digest,
        &canceled,
        |done, total| last = (done, total),
    )
    .unwrap();
    assert_eq!(output, bytes);
    assert_eq!(last, (bytes.len() as u64, bytes.len() as u64));
    for expected in [bytes.len() as u64 - 1, bytes.len() as u64 + 1] {
        assert_eq!(
            receive_archive(
                Cursor::new(&bytes),
                &mut Vec::new(),
                expected,
                digest,
                &canceled,
                |_, _| {}
            ),
            Err(Error::Integrity)
        );
    }
    assert_eq!(
        receive_archive(
            Cursor::new(&bytes),
            &mut Vec::new(),
            bytes.len() as u64,
            [0; 32],
            &canceled,
            |_, _| {}
        ),
        Err(Error::Integrity)
    );
    let mut output = Vec::new();
    assert_eq!(
        receive_archive(
            Cursor::new(&bytes),
            &mut output,
            bytes.len() as u64,
            digest,
            &canceled,
            |_, _| canceled.store(true, Ordering::Release)
        ),
        Err(Error::Canceled)
    );
    assert!(output.len() < bytes.len());
}

fn archive(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, options).unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
    }
    zip.finish().unwrap();
}

#[test]
fn extraction_accepts_only_x64_program_files_and_ignores_empty_data_directories() {
    let stage = StagedUpdate::new().unwrap();
    let exe = pe("app");
    let path = stage.path.join("test.zip");
    archive(
        &path,
        &[
            ("VK-Fast-0.1.2-windows-x86_64/vk-fast.exe", &exe),
            ("VK-Fast-0.1.2-windows-x86_64/config/", b""),
            ("VK-Fast-0.1.2-windows-x86_64/cache/", b""),
        ],
    );
    let first = extract_package(&path, &stage.path, &Version::new(0, 1, 2)).unwrap();
    assert_eq!(first, hex_hash(&Sha256::digest(&exe)));
    assert!(!stage.path.join("config").exists() && !stage.path.join("cache").exists());
}

#[test]
fn archive_cannot_write_traversal_data_payloads_other_versions_or_non_pe_binaries() {
    let exe = pe("app");
    for bad in [
        "../escaped.exe",
        "VK-Fast-0.1.2-windows-x86_64/config/session.json",
        "VK-Fast-0.1.2-windows-x86_64/bin/plugin.dll",
        "VK-Fast-0.1.1-windows-x86_64/vk-fast.exe",
        "VK-Fast-0.1.2-windows-x86_64/bin/../vk-fast.exe",
        "C:/escaped.exe",
    ] {
        let stage = StagedUpdate::new().unwrap();
        let path = stage.path.join("test.zip");
        archive(&path, &[(bad, &exe)]);
        assert!(
            extract_package(&path, &stage.path, &Version::new(0, 1, 2)).is_err(),
            "{bad}"
        );
    }
    let stage = StagedUpdate::new().unwrap();
    let path = stage.path.join("test.zip");
    archive(
        &path,
        &[(
            "VK-Fast-0.1.2-windows-x86_64/vk-fast.exe",
            b"not a PE executable",
        )],
    );
    assert_eq!(
        extract_package(&path, &stage.path, &Version::new(0, 1, 2)),
        Err(Error::InvalidPackage)
    );
}

#[test]
fn archive_symlink_and_other_architectures_are_rejected() {
    let stage = StagedUpdate::new().unwrap();
    let path = stage.path.join("test.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    zip.add_symlink(
        "VK-Fast-0.1.2-windows-x86_64/vk-fast.exe",
        "../../outside.exe",
        SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    assert_eq!(
        extract_package(&path, &stage.path, &Version::new(0, 1, 2)),
        Err(Error::InvalidPackage)
    );
    for machine in [0x014cu16, 0xaa64u16] {
        let mut bytes = pe("wrong architecture");
        bytes[132..134].copy_from_slice(&machine.to_le_bytes());
        let file = stage.path.join(format!("{machine}.exe"));
        fs::write(&file, bytes).unwrap();
        assert_eq!(validate_pe(&file), Err(Error::InvalidPackage));
    }
}
