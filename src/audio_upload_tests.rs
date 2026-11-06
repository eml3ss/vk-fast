use super::*;

pub(crate) struct Temporary(pub std::path::PathBuf);
impl Temporary {
    pub fn new() -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!(
            "vk-fast-upload-test-{:x}",
            u128::from_ne_bytes(random)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn source() -> (Temporary, Arc<Source>) {
    let temp = Temporary::new();
    let path = temp.0.join("Исполнитель - Мой трек.mp3");
    std::fs::write(
        &path,
        [vec![0xff, 0xfb, 0x90, 0x64], vec![0; 2048]].concat(),
    )
    .unwrap();
    let source = Arc::new(Source::open(&path).unwrap());
    (temp, source)
}

#[test]
fn selecting_a_local_file_only_prepares_metadata_and_a_bounded_reader() {
    let (_temp, source) = source();
    assert_eq!(source.artist, "Исполнитель");
    assert_eq!(source.title, "Мой трек");
    let mut read = Vec::new();
    source.reader().unwrap().read_to_end(&mut read).unwrap();
    assert_eq!(read.len() as u64, source.size);
    assert_eq!(&read[..4], &[0xff, 0xfb, 0x90, 0x64]);
}
#[test]
fn rejects_other_formats_empty_files_and_oversized_mp3_without_reading_them() {
    let temp = Temporary::new();
    for (name, bytes) in [
        ("track.mp3", b"MZ definitely not audio".as_slice()),
        ("track.wav", b"RIFF".as_slice()),
        ("empty.mp3", b"".as_slice()),
    ] {
        let path = temp.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(Source::open(&path), Err(Error::Format)));
    }
    let path = temp.0.join("large.mp3");
    File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
    assert!(matches!(Source::open(&path), Err(Error::TooLarge)));
}
#[test]
fn skips_id3_tags_and_detects_a_changed_selected_file_before_upload() {
    let temp = Temporary::new();
    let path = temp.0.join("track.mp3");
    let mut bytes = b"ID3\x04\0\0\0\0\x01\0".to_vec();
    bytes.extend_from_slice(&[0; 128]);
    bytes.extend_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
    std::fs::write(&path, bytes).unwrap();
    let source = Source::open(&path).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(source.size + 1)
        .unwrap();
    assert!(matches!(source.reader(), Err(Error::Changed)));
}
#[test]
fn metadata_is_required_bounded_and_accepts_original_languages() {
    for text in ["Мой трек", "日本語の曲", "Artist & friends"] {
        assert!(valid_text(text));
    }
    for text in ["", "   ", "track\nname", &"a".repeat(MAX_TEXT + 1)] {
        assert!(!valid_text(text));
    }
}
