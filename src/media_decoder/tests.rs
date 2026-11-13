use super::*;
use net::Net;
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

fn decode(input: Box<dyn MediaSource>, origin: f64, offset: f64) -> Vec<f32> {
    let mut decoder =
        Pipeline::new(input, origin, offset, Arc::new(AtomicBool::new(false))).unwrap();
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).unwrap();
    assert!(bytes.len().is_multiple_of(8));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}
fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}
#[test]
fn mp3_native_decode_resamples_and_seeks_on_the_same_stereo_timeline() {
    let all = decode(Box::new(File::open(fixture("tone.mp3")).unwrap()), 0.0, 0.0);
    assert!(
        (all.len() / 2).abs_diff(12 * 48000) < 64,
        "{}",
        all.len() / 2
    );
    assert!(all.iter().all(|v| v.is_finite()));
    assert!(all.iter().any(|v| v.abs() > 0.01));
    let seek = decode(Box::new(File::open(fixture("tone.mp3")).unwrap()), 0.0, 8.0);
    let reference = &all[8 * 48000 * 2..];
    assert!(
        (seek.len() / 2).abs_diff(reference.len() / 2) < 64,
        "seek {} expected {}",
        seek.len(),
        reference.len()
    );
    let error = seek
        .iter()
        .zip(reference)
        .skip(4800)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        / seek.len() as f32;
    assert!(error < 0.0001, "seek MSE {error}");
}
#[test]
fn mono_aac_is_duplicated_to_stereo_without_a_subprocess() {
    let pcm = decode(Box::new(File::open(fixture("tone.aac")).unwrap()), 0.0, 0.0);
    assert!((48000..52000).contains(&(pcm.len() / 2)));
    assert!(pcm.as_chunks::<2>().0.iter().all(|c| c[0] == c[1]));
}

struct Server {
    url: reqwest::Url,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(files: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url =
            reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let worker = thread::spawn(move || {
            while !cancel.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = reply(&mut stream, &files);
                    }
                    Err(_) => thread::sleep(Duration::from_millis(1)),
                }
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
    fn net(&self) -> Net {
        let mut net = Net::new(self.stop.clone()).unwrap();
        net.allow_local = true;
        net
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
fn reply(stream: &mut TcpStream, files: &HashMap<String, Vec<u8>>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut request = Vec::new();
    let mut b = [0; 1];
    while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
        if stream.read(&mut b)? == 0 {
            return Ok(());
        }
        request.push(b[0]);
    }
    let request = String::from_utf8_lossy(&request);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let Some(data) = files.get(path.split('?').next().unwrap_or(path)) else {
        stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    };
    assert!(!request.to_ascii_lowercase().contains("cookie:"));
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
    let range = request
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("range: bytes=")
                .map(str::to_owned)
        })
        .and_then(|r| {
            r.split_once('-')
                .map(|(a, b)| (a.parse::<usize>().ok(), b.parse::<usize>().ok()))
        });
    let range = if path.contains("no-range") {
        None
    } else {
        range
    };
    let (start, end, ranged) = match range {
        Some((Some(a), Some(b))) if a < data.len() => (a, b.min(data.len() - 1), true),
        _ => (0, data.len().saturating_sub(1), false),
    };
    let body = if data.is_empty() {
        &data[..]
    } else {
        &data[start..=end]
    };
    let header = if ranged {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            data.len(),
            body.len()
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
    };
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)
}
fn hls_files(folder: &str) -> HashMap<String, Vec<u8>> {
    std::fs::read_dir(fixture(folder))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                format!("/{}", entry.file_name().to_string_lossy()),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
fn hls_decode(server: &Server, offset: f64) -> Vec<f32> {
    let net = server.net();
    let reply = net
        .get(
            "playlist",
            &server.url.join("stream.m3u8").unwrap(),
            None,
            1024 * 1024,
        )
        .unwrap();
    let stream = hls::Hls::new(net, reply.url, reply.data, offset).unwrap();
    let origin = stream.origin;
    decode(Box::new(stream), origin, offset)
}
#[test]
fn transport_aac_hls_supports_full_track_and_tail_seek() {
    let server = Server::new(hls_files("ts"));
    let all = hls_decode(&server, 0.0);
    assert!(
        (12 * 48000..13 * 48000).contains(&(all.len() / 2)),
        "{}",
        all.len() / 2
    );
    let seek = hls_decode(&server, 9.0);
    let reference = &all[9 * 48000 * 2..];
    assert!(
        (seek.len() / 2).abs_diff(reference.len() / 2) < 128,
        "seek {} reference {}",
        seek.len() / 2,
        reference.len() / 2
    );
    let error = seek
        .iter()
        .zip(reference)
        .skip(4800)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        / seek.len() as f32;
    assert!(error < 0.0001, "seek MSE {error}");
}
#[test]
fn fragmented_mp4_hls_uses_the_native_demuxer() {
    let server = Server::new(hls_files("fmp4"));
    let pcm = hls_decode(&server, 0.0);
    assert!(
        (12 * 48000..13 * 48000).contains(&(pcm.len() / 2)),
        "{}",
        pcm.len() / 2
    );
    let seek = hls_decode(&server, 8.0);
    let reference = &pcm[8 * 48000 * 2..];
    assert!(
        (seek.len() / 2).abs_diff(reference.len() / 2) < 64,
        "fragment seek {} expected {}",
        seek.len() / 2,
        reference.len() / 2
    );
    let mse = seek
        .iter()
        .zip(reference)
        .skip(4800)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        / seek.len() as f32;
    assert!(mse < 0.0001, "fragment seek MSE {mse}");
}
#[test]
fn protected_hls_segments_are_decrypted_with_the_declared_sequence_iv() {
    use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    let mut files = hls_files("ts");
    let key = [37u8; 16];
    for (index, name) in [
        "segment00.ts",
        "segment01.ts",
        "segment02.ts",
        "segment03.ts",
        "segment04.ts",
        "segment05.ts",
        "segment06.ts",
    ]
    .into_iter()
    .enumerate()
    {
        let path = format!("/{name}");
        if index >= 4 {
            continue;
        }
        let clear = files.get(&path).unwrap();
        let cipher = cbc::Encryptor::<aes::Aes128>::new_from_slices(
            &key,
            &(41u128 + index as u128).to_be_bytes(),
        )
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(clear);
        files.insert(path, cipher);
    }
    files.insert("/key".into(), key.to_vec());
    let playlist = String::from_utf8(files["/stream.m3u8"].clone())
        .unwrap()
        .replace(
            "#EXT-X-MEDIA-SEQUENCE:0",
            "#EXT-X-MEDIA-SEQUENCE:41\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"",
        )
        .replace("segment04.ts", "#EXT-X-KEY:METHOD=NONE\nsegment04.ts");
    files.insert("/stream.m3u8".into(), playlist.into_bytes());
    let server = Server::new(files);
    let pcm = hls_decode(&server, 0.0);
    assert!(pcm.iter().any(|v| v.abs() > 0.01));
}
#[test]
fn pending_network_header_is_canceled_without_waiting_for_the_request_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = stop.clone();
    let worker = thread::spawn(move || {
        let (_socket, _) = listener.accept().unwrap();
        while !cancel.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    let mut net = Net::new(stop.clone()).unwrap();
    net.allow_local = true;
    let done = stop.clone();
    let trigger = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        done.store(true, Ordering::Release);
    });
    let start = Instant::now();
    assert!(
        net.get(
            "test",
            &reqwest::Url::parse(&format!("http://{addr}/")).unwrap(),
            None,
            1024
        )
        .is_err()
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    trigger.join().unwrap();
    worker.join().unwrap();
}

fn wave(rate: u32, channels: u16, seconds: u32) -> Vec<u8> {
    let len = rate * channels as u32 * seconds * 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * channels as u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&(channels * 4).to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&len.to_le_bytes());
    for frame in 0..rate * seconds {
        let value = (frame as f32 * 850.0 * std::f32::consts::TAU / rate as f32).sin() * 0.1;
        for channel in 0..channels {
            bytes.extend_from_slice(&(if channel % 2 == 0 { value } else { -value }).to_le_bytes());
        }
    }
    bytes
}
#[test]
fn direct_http_range_and_no_range_sources_decode_seek_and_keep_credentials_private() {
    let wave = wave(44100, 2, 2);
    let reference = decode(Box::new(Cursor::new(wave.clone())), 0.0, 0.5);
    let server = Server::new(HashMap::from([
        ("/tone.wav".into(), wave.clone()),
        ("/no-range.wav".into(), wave),
    ]));
    for path in ["tone.wav", "no-range.wav"] {
        let (source, origin) =
            source::from_net(server.net(), server.url.join(path).unwrap(), 0.5, None).unwrap();
        let actual = decode(source, origin, 0.5);
        assert_eq!(actual.len(), reference.len());
        assert!(
            actual
                .iter()
                .zip(&reference)
                .all(|(a, b)| (a - b).abs() < 0.00001)
        );
    }
}
#[test]
fn public_decoder_refuses_local_files_userinfo_and_non_https_urls() {
    for url in [
        "file:///etc/passwd",
        "http://127.0.0.1/test",
        "https://name:password@example.test/a",
        "https://example.test:8000/a",
    ] {
        assert!(open(url, 0.0).is_err());
    }
    assert!(open("https://example.test/a", f64::NAN).is_err());
}
#[test]
fn a_missing_hls_segment_is_logged_with_where_and_why_but_no_address() {
    let mut files = hls_files("ts");
    files.remove("/segment04.ts").unwrap();
    let server = Server::new(files);
    let (pcm, log) = crate::diagnostics::capture_for_test(|| {
        let net = server.net();
        let reply = net
            .get(
                "playlist",
                &server.url.join("stream.m3u8").unwrap(),
                None,
                1 << 20,
            )
            .unwrap();
        let stream = hls::Hls::new(net, reply.url, reply.data, 0.0).unwrap();
        let mut decoder =
            Pipeline::new(Box::new(stream), 0.0, 0.0, Arc::new(AtomicBool::new(false))).unwrap();
        let mut bytes = Vec::new();
        decoder.read_to_end(&mut bytes).map(|_| bytes.len())
    });
    let error = pcm.unwrap_err();
    assert!(error.to_string().contains("segment status 404"), "{error}");
    for expected in [
        "source kind=hls segments=7",
        "format ",
        "codec=aac rate_khz=",
        "media_http segment status=200 kib=",
        "media_http segment status=404",
        "hls segment_failed index=4 of=7",
        "demux_failed at_s=",
    ] {
        assert!(log.contains(expected), "missing {expected:?} in:\n{log}");
    }
    assert!(
        !log.contains("127.0.0.1") && !log.contains("segment04"),
        "{log}"
    );
}

fn media_store(
    directory: &std::path::Path,
) -> (
    crate::audio_cache::AudioCache,
    crate::audio_cache::MediaStore,
) {
    let cache =
        crate::audio_cache::AudioCache::test_at(directory.join("audio"), 64 * 1024 * 1024).unwrap();
    let key = crate::audio_cache::CacheKey {
        account_id: 42,
        owner_id: -7,
        track_id: 9,
        duration: 36,
    };
    let store = crate::audio_cache::MediaStore::new(cache.clone(), key);
    (cache, store)
}
/// Three copies of the tone: more than one 256 KiB chunk. Its header still
/// says 12 seconds, which is what the decoder plays.
fn long_tone() -> Vec<u8> {
    std::fs::read(fixture("tone.mp3")).unwrap().repeat(3)
}

#[test]
fn a_played_mp3_is_kept_whole_and_plays_again_from_disk_alone() {
    let file = long_tone();
    assert!(file.len() as u64 > crate::audio_cache::MEDIA_CHUNK);
    let server = Server::new(HashMap::from([("/track.mp3".into(), file.clone())]));
    let directory = tempfile::tempdir().unwrap();
    let (_cache, store) = media_store(directory.path());
    let url = server.url.join("track.mp3").unwrap();
    assert!(source::stored(store.clone(), 0.0).is_none());
    let (input, origin) =
        source::from_net(server.net(), url.clone(), 0.0, Some(store.clone())).unwrap();
    let online = decode(input, origin, 0.0);
    // Every chunk the decoder read is stored: the whole file.
    assert_eq!(store.complete(), Some(file.len() as u64));
    drop(server);
    // No server now: the stored file decodes to the very same samples, and
    // a seek inside it lands where it does online.
    let offline = decode(source::stored(store.clone(), 0.0).unwrap(), 0.0, 0.0);
    assert_eq!(offline, online);
    let tail = decode(source::stored(store.clone(), 8.0).unwrap(), 0.0, 8.0);
    let reference = &online[8 * 48000 * 2..];
    assert!((tail.len() / 2).abs_diff(reference.len() / 2) < 64);
    // A chunk lost since (a cleared cache) is an error, not silence.
    let path = directory.path().join("audio");
    for entry in std::fs::read_dir(&path).unwrap() {
        let folder = entry.unwrap().path();
        if folder.is_dir() {
            let _ = std::fs::remove_file(folder.join("00000001.chunk"));
        }
    }
    assert!(store.complete().is_none());
}

#[test]
fn a_background_download_stores_the_file_and_skips_playlists() {
    let file = long_tone();
    let playlist = b"#EXTM3U\n#EXT-X-ENDLIST\n".to_vec();
    let server = Server::new(HashMap::from([
        ("/track.mp3".into(), file.clone()),
        ("/index.m3u8".into(), playlist),
    ]));
    let directory = tempfile::tempdir().unwrap();
    let (cache, store) = media_store(directory.path());
    let url = server.url.join("track.mp3").unwrap();
    source::download(
        server.net(),
        url.as_str(),
        &store.at_current_revision().unwrap(),
    )
    .unwrap();
    assert_eq!(store.complete(), Some(file.len() as u64));
    let bytes = decode(source::stored(store.clone(), 0.0).unwrap(), 0.0, 0.0);
    assert!((bytes.len() / 2).abs_diff(12 * 48000) < 4800);
    // A cleared cache stops a download that started before it.
    cache.clear().unwrap();
    let stale = store.at_current_revision().unwrap();
    cache.clear().unwrap();
    assert!(source::download(server.net(), url.as_str(), &stale).is_err());
    assert!(store.complete().is_none());
    // A playlist is not a file to keep.
    let other = crate::audio_cache::MediaStore::new(
        cache.clone(),
        crate::audio_cache::CacheKey {
            account_id: 42,
            owner_id: -7,
            track_id: 10,
            duration: 36,
        },
    );
    let playlist_url = server.url.join("index.m3u8").unwrap();
    source::download(server.net(), playlist_url.as_str(), &other).unwrap();
    assert!(other.total().is_none());
}
