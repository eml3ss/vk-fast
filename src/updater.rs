//! Windows updates use the public release API, never the VK HTTP client or session.
//! The portable payload serves both installed and portable copies; user data is
//! deliberately outside the executable replacement transaction.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use reqwest::{Url, blocking::Client};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod install;
#[cfg(test)]
pub(crate) mod tests;

pub const RELEASES_URL: &str = "https://github.com/eml3ss/vk-fast/releases";
const API_URL: &str = "https://api.github.com/repos/eml3ss/vk-fast/releases/latest";
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_EXECUTABLE: u64 = 128 * 1024 * 1024;
const STAGE_PREFIX: &str = "VK-Fast-update-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Network,
    RateLimited,
    InvalidRelease,
    Integrity,
    InvalidPackage,
    Disk,
    Canceled,
    Helper,
    Busy,
    Changed,
    Rollback,
}

impl From<io::Error> for Error {
    fn from(_: io::Error) -> Self {
        Self::Disk
    }
}

impl Error {
    pub fn message(self, language: crate::i18n::Language) -> &'static str {
        match self {
            Self::Network => language.text("Не удалось связаться с GitHub. Попробуйте позже.", "Could not connect to GitHub. Try again later."),
            Self::RateLimited => language.text("GitHub временно ограничил запросы. Попробуйте позже.", "GitHub temporarily limited requests. Try again later."),
            Self::InvalidRelease => language.text("В выпуске пока нет подходящего обновления Windows.", "This release does not have a compatible Windows update yet."),
            Self::Integrity => language.text("Проверка обновления не пройдена. Загрузите его заново.", "Update verification failed. Download it again."),
            Self::InvalidPackage => language.text("Архив обновления повреждён или имеет неподходящий формат.", "The update archive is damaged or incompatible."),
            Self::Disk => language.text("Не удалось записать обновление. Проверьте свободное место и доступ к папке программы.", "Could not write the update. Check free space and access to the application folder."),
            Self::Canceled => language.text("Загрузка отменена.", "Download canceled."),
            Self::Helper => language.text("Не удалось запустить обновление. Текущая версия сохранена.", "Could not start the update. The current version was retained."),
            Self::Busy => language.text("Файлы программы заняты. Полностью закройте другие копии VK Fast и попробуйте снова.", "Application files are busy. Fully close other copies of VK Fast and try again."),
            Self::Changed => language.text("Файлы программы изменились. Проверьте обновления ещё раз.", "Application files changed. Check for updates again."),
            Self::Rollback => language.text("Не удалось завершить восстановление. Резервные файлы сохранены рядом с программой; переустановите VK Fast в ту же папку.", "Recovery could not finish. Backup files remain next to the application; reinstall VK Fast into the same folder."),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub notes: String,
    asset_url: Url,
    asset_size: u64,
    digest: [u8; 32],
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
    state: String,
}

fn parse_release(bytes: &[u8], current: &str) -> Result<Option<Release>, Error> {
    let release: GithubRelease =
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidRelease)?;
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| Error::InvalidRelease)?;
    let current = Version::parse(current).map_err(|_| Error::InvalidRelease)?;
    if release.draft
        || release.prerelease
        || !version.pre.is_empty()
        || !version.cmp_precedence(&current).is_gt()
    {
        return Ok(None);
    }
    // Only our exact runtime archive is executable update input. Source code,
    // installers, other architectures and arbitrary asset URLs are not accepted.
    let name = format!("VK-Fast-{version}-windows-x86_64.zip");
    let mut assets = release
        .assets
        .into_iter()
        .filter(|asset| asset.name == name);
    let asset = assets.next().ok_or(Error::InvalidRelease)?;
    if assets.next().is_some()
        || asset.state != "uploaded"
        || !(1..=MAX_DOWNLOAD).contains(&asset.size)
    {
        return Err(Error::InvalidRelease);
    }
    let expected = format!("{RELEASES_URL}/download/{}/{name}", release.tag_name);
    if asset.browser_download_url != expected {
        return Err(Error::InvalidRelease);
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .and_then(parse_hash)
        .ok_or(Error::Integrity)?;
    Ok(Some(Release {
        version,
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .take(4096)
            .collect(),
        asset_url: Url::parse(&expected).map_err(|_| Error::InvalidRelease)?,
        asset_size: asset.size,
        digest,
    }))
}

fn allowed_download_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

fn client() -> Result<Client, Error> {
    Client::builder()
        .user_agent(concat!("VK-Fast/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !allowed_download_url(attempt.url()) {
                attempt.error("Update redirect rejected")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| Error::Network)
}

pub fn check() -> Result<Option<Release>, Error> {
    let response = client()?
        .get(API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .timeout(Duration::from_secs(20))
        .send()
        .map_err(|_| Error::Network)?;
    match response.status().as_u16() {
        404 => return Ok(None),
        403 | 429 => return Err(Error::RateLimited),
        200 => {}
        _ => return Err(Error::Network),
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(Error::InvalidRelease);
    }
    parse_release(&bytes, env!("CARGO_PKG_VERSION"))
}

pub struct StagedUpdate {
    pub path: PathBuf,
    handed_off: bool,
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        if !self.handed_off {
            let path = self.path.clone();
            let _ = std::thread::Builder::new()
                .name("update-discard".into())
                .spawn(move || {
                    let _ = fs::remove_dir_all(path);
                });
        }
    }
}

impl StagedUpdate {
    fn new() -> Result<Self, Error> {
        let base = fs::canonicalize(std::env::temp_dir())?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| Error::Disk)?;
        let path = base.join(format!("{STAGE_PREFIX}{}", hex_hash(&random)));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            path,
            handed_off: false,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Job {
    schema: u32,
    nonce: String,
    install_dir: PathBuf,
    version: String,
    previous_exe: String,
    executable: String,
    english: bool,
}

pub fn download(
    release: &Release,
    canceled: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<StagedUpdate, Error> {
    let stage = StagedUpdate::new()?;
    let response = client()?
        .get(release.asset_url.clone())
        .send()
        .map_err(|_| Error::Network)?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size != release.asset_size)
    {
        return Err(Error::Network);
    }
    let archive = stage.path.join("download.zip");
    let mut file = File::create_new(&archive)?;
    receive_archive(
        response,
        &mut file,
        release.asset_size,
        release.digest,
        canceled,
        &mut progress,
    )?;
    file.sync_all()?;
    drop(file);
    if canceled.load(Ordering::Acquire) {
        return Err(Error::Canceled);
    }
    let executable = extract_package(&archive, &stage.path, &release.version)?;
    fs::remove_file(archive)?;
    let current = fs::canonicalize(std::env::current_exe()?)?;
    if current.file_name().and_then(|name| name.to_str()) != Some("vk-fast.exe") {
        return Err(Error::Helper);
    }
    let install_dir = current.parent().ok_or(Error::Disk)?.to_path_buf();
    plain(&install_dir, true)?;
    let job = Job {
        schema: 2,
        nonce: stage_nonce(&stage.path)?,
        install_dir,
        version: release.version.to_string(),
        previous_exe: hash_file(&current, MAX_EXECUTABLE)?,
        executable,
        english: false,
    };
    fs::copy(current, stage.path.join("runner.exe"))?;
    write_job(&stage.path, &job)?;
    Ok(stage)
}

fn receive_archive(
    mut response: impl Read,
    file: &mut impl Write,
    expected_size: u64,
    expected_digest: [u8; 32],
    canceled: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), Error> {
    if !(1..=MAX_DOWNLOAD).contains(&expected_size) {
        return Err(Error::Integrity);
    }
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        if canceled.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        let read = response.read(&mut buffer).map_err(|_| Error::Network)?;
        if read == 0 {
            break;
        }
        received += read as u64;
        if received > expected_size {
            return Err(Error::Integrity);
        }
        file.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        progress(received, expected_size);
    }
    if received != expected_size || <[u8; 32]>::from(hasher.finalize()) != expected_digest {
        return Err(Error::Integrity);
    }
    Ok(())
}

fn extract_package(archive: &Path, stage: &Path, version: &Version) -> Result<String, Error> {
    let mut archive =
        zip::ZipArchive::new(File::open(archive)?).map_err(|_| Error::InvalidPackage)?;
    let entries = archive.len();
    if !(1..=4).contains(&entries) {
        return Err(Error::InvalidPackage);
    }
    let prefix = format!("VK-Fast-{version}-windows-x86_64/");
    let mut seen = HashSet::new();
    let mut exe = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| Error::InvalidPackage)?;
        let name = entry
            .name()
            .map_err(|_| Error::InvalidPackage)?
            .into_owned();
        if !seen.insert(name.clone())
            || entry.enclosed_name().is_none()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(Error::InvalidPackage);
        }
        let relative = if name == "vk-fast.exe" {
            if entries != 1 {
                return Err(Error::InvalidPackage);
            }
            name.as_str()
        } else {
            name.strip_prefix(&prefix).ok_or(Error::InvalidPackage)?
        };
        if matches!(relative, "" | "config/" | "cache/") {
            if !entry.is_dir() || entry.size() != 0 {
                return Err(Error::InvalidPackage);
            }
            continue;
        }
        let (destination, limit) = match relative {
            "vk-fast.exe" if exe.is_none() => ("next-vk-fast.exe", MAX_EXECUTABLE),
            _ => return Err(Error::InvalidPackage),
        };
        let expected = entry.size();
        if entry.is_dir() || !(1..=limit).contains(&expected) {
            return Err(Error::InvalidPackage);
        }
        let output = stage.join(destination);
        let mut file = File::create_new(&output)?;
        let received = io::copy(&mut (&mut entry).take(limit + 1), &mut file)
            .map_err(|_| Error::InvalidPackage)?;
        file.sync_all()?;
        drop(file);
        if received != expected {
            return Err(Error::InvalidPackage);
        }
        validate_pe(&output)?;
        let digest = hash_file(&output, limit)?;
        exe = Some(digest);
    }
    exe.ok_or(Error::InvalidPackage)
}

fn validate_pe(path: &Path) -> Result<(), Error> {
    let mut file = File::open(path)?;
    let mut header = [0; 64];
    file.read_exact(&mut header)
        .map_err(|_| Error::InvalidPackage)?;
    if &header[..2] != b"MZ" {
        return Err(Error::InvalidPackage);
    }
    let offset = u32::from_le_bytes(
        header[60..64]
            .try_into()
            .map_err(|_| Error::InvalidPackage)?,
    );
    if !(64..=1024 * 1024).contains(&offset) {
        return Err(Error::InvalidPackage);
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut pe = [0; 26];
    file.read_exact(&mut pe)
        .map_err(|_| Error::InvalidPackage)?;
    if &pe[..4] != b"PE\0\0" || pe[4..6] != [0x64, 0x86] || pe[24..26] != [0x0b, 0x02] {
        return Err(Error::InvalidPackage);
    }
    Ok(())
}

fn hash_file(path: &Path, limit: u64) -> Result<String, Error> {
    plain(path, false)?;
    let mut file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(Error::Integrity);
    }
    let mut hasher = Sha256::new();
    let copied = io::copy(&mut (&mut file).take(limit + 1), &mut hasher)?;
    if copied > limit {
        return Err(Error::Integrity);
    }
    Ok(hex_hash(&hasher.finalize()))
}

fn hex_hash(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn parse_hash(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut output = [0; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

fn plain(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Error::Disk);
        }
    }
    if metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(Error::Disk);
    }
    Ok(())
}

fn stage_nonce(stage: &Path) -> Result<String, Error> {
    let nonce = stage
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(STAGE_PREFIX))
        .ok_or(Error::Helper)?;
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Helper);
    }
    Ok(nonce.to_owned())
}

fn read_job(stage: &Path) -> Result<Job, Error> {
    plain(stage, true)?;
    plain(&stage.join("job.json"), false)?;
    let mut bytes = Vec::new();
    File::open(stage.join("job.json"))?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(Error::Helper);
    }
    let job: Job = serde_json::from_slice(&bytes).map_err(|_| Error::Helper)?;
    let version = Version::parse(&job.version).map_err(|_| Error::Helper)?;
    if job.schema != 2
        || job.nonce != stage_nonce(stage)?
        || !version.pre.is_empty()
        || !job.install_dir.is_absolute()
        || job.install_dir.starts_with(stage)
        || [&job.previous_exe, &job.executable]
            .iter()
            .any(|hash| parse_hash(hash).is_none())
    {
        return Err(Error::Helper);
    }
    plain(&job.install_dir, true)?;
    Ok(job)
}

fn write_job(stage: &Path, job: &Job) -> Result<(), Error> {
    let mut file = File::create(stage.join("job.json"))?;
    file.write_all(&serde_json::to_vec(job).map_err(|_| Error::Helper)?)?;
    file.sync_all()?;
    Ok(())
}
