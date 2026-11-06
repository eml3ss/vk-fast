//! Local audio selection never uploads anything. The chosen, open file is sent
//! only after the user confirms title and artist in the upload form.
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use crate::{i18n::Language, web_audio::WebAudioError};

pub const MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const MAX_TEXT: usize = 256;

pub struct Source {
    pub name: String,
    pub title: String,
    pub artist: String,
    pub size: u64,
    file: File,
    metadata: Metadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Read,
    TooLarge,
    Format,
    Metadata,
    Changed,
    Web(WebAudioError),
    Uncertain,
}

impl From<WebAudioError> for Error {
    fn from(error: WebAudioError) -> Self {
        Self::Web(error)
    }
}

impl Error {
    pub fn message(&self, language: Language) -> String {
        match self {
            Self::Read => language.text("Не удалось прочитать аудиофайл.", "Could not read the audio file."),
            Self::TooLarge => language.text("Выберите MP3 размером до 200 МБ.", "Choose an MP3 file up to 200 MB."),
            Self::Format => language.text("Выберите аудиофайл в формате MP3.", "Choose an MP3 audio file."),
            Self::Metadata => language.text("Укажите название трека и исполнителя.", "Enter the track title and artist."),
            Self::Changed => language.text("Файл изменился. Выберите его заново.", "The file changed. Select it again."),
            Self::Uncertain => language.text("Не удалось подтвердить сохранение. Трек уже мог попасть в вашу музыку — обновите библиотеку перед повторной загрузкой.", "Could not confirm saving. The track may already be in your music. Refresh your library before uploading it again."),
            Self::Web(WebAudioError::Rejected) => language.text("VK отклонил загрузку. Проверьте, подходит ли этот MP3 для загрузки на сайте VK.", "VK rejected the upload. Check whether this MP3 can be uploaded on the VK website."),
            Self::Web(error) => return error.localized(language),
        }.into()
    }
}

pub fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= MAX_TEXT
        && !text.chars().any(char::is_control)
}

pub fn choose(language: Language) -> Result<Option<Arc<Source>>, Error> {
    let Some(path) = rfd::FileDialog::new()
        .set_title(language.text("Выбрать трек", "Choose a track"))
        .add_filter("MP3", &["mp3"])
        .pick_file()
    else {
        return Ok(None);
    };
    Source::open(&path).map(|source| Some(Arc::new(source)))
}

impl Source {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp3"))
        {
            return Err(Error::Format);
        }
        let mut file = File::open(path).map_err(|_| Error::Read)?;
        let metadata = file.metadata().map_err(|_| Error::Read)?;
        if !metadata.is_file() || metadata.len() < 4 {
            return Err(Error::Format);
        }
        if metadata.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let mut header = [0u8; 10];
        let count = file.read(&mut header).map_err(|_| Error::Read)?;
        let offset = if count == 10 && &header[..3] == b"ID3" {
            if !(2..=4).contains(&header[3]) || header[6..10].iter().any(|b| b & 0x80 != 0) {
                return Err(Error::Format);
            }
            let size = header[6..10]
                .iter()
                .fold(0u64, |size, value| (size << 7) | u64::from(*value));
            10 + size
                + if header[3] == 4 && header[5] & 0x10 != 0 {
                    10
                } else {
                    0
                }
        } else {
            0
        };
        if offset >= metadata.len() {
            return Err(Error::Format);
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| Error::Read)?;
        let mut sample = Vec::new();
        (&mut file)
            .take(64 * 1024)
            .read_to_end(&mut sample)
            .map_err(|_| Error::Read)?;
        if !sample.windows(4).any(mp3_header) {
            return Err(Error::Format);
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| Error::Read)?;
        let name = path
            .file_name()
            .ok_or(Error::Read)?
            .to_string_lossy()
            .into_owned();
        let stem = path.file_stem().ok_or(Error::Read)?.to_string_lossy();
        let (artist, title) = stem
            .split_once(" - ")
            .filter(|(artist, title)| valid_text(artist) && valid_text(title))
            .unwrap_or(("", &stem));
        Ok(Self {
            name,
            title: title
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_TEXT)
                .collect(),
            artist: artist.chars().take(MAX_TEXT).collect(),
            size: metadata.len(),
            file,
            metadata,
        })
    }

    pub fn reader(&self) -> Result<File, Error> {
        self.unchanged()?;
        let mut file = self.file.try_clone().map_err(|_| Error::Read)?;
        file.seek(SeekFrom::Start(0)).map_err(|_| Error::Read)?;
        Ok(file)
    }

    pub fn unchanged(&self) -> Result<(), Error> {
        let current = self.file.metadata().map_err(|_| Error::Read)?;
        if current.len() != self.size || current.modified().ok() != self.metadata.modified().ok() {
            return Err(Error::Changed);
        }
        Ok(())
    }
}

fn mp3_header(bytes: &[u8]) -> bool {
    bytes[0] == 0xff
        && bytes[1] & 0xe0 == 0xe0
        && bytes[1] & 0x18 != 0x08
        && bytes[1] & 0x06 == 0x02
        && !matches!(bytes[2] >> 4, 0 | 15)
        && bytes[2] & 0x0c != 0x0c
}

#[cfg(test)]
#[path = "audio_upload_tests.rs"]
pub(crate) mod tests;
