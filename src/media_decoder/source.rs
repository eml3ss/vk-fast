use super::{
    hls::Hls,
    net::{Initial, Net, Reply, invalid},
};
use crate::audio_cache::{MAX_MEDIA_BYTES, MEDIA_CHUNK as CHUNK, MediaStore};
use reqwest::Url;
use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::{Arc, atomic::AtomicBool},
};
use symphonia::core::io::MediaSource;

pub(super) fn open(
    url: &str,
    offset: f64,
    canceled: Arc<AtomicBool>,
    store: Option<MediaStore>,
) -> io::Result<(Box<dyn MediaSource>, f64)> {
    let url = Url::parse(url).map_err(|_| invalid())?;
    let net = Net::new(canceled)?;
    from_net(net, url, offset, store)
}
/// A file kept whole in the cache plays from disk alone.
pub(super) fn stored(store: MediaStore, offset: f64) -> Option<Box<dyn MediaSource>> {
    let total = store.complete()?;
    log_source("cache", total, offset);
    Some(Box::new(Stored {
        store,
        total,
        position: 0,
        start: 0,
        buffer: Vec::new(),
    }))
}
/// Fetch every chunk of a plain file the store lacks, in order, on the
/// calling thread. A playlist (HLS) is left alone: its parts are not files.
pub(super) fn download(net: Net, url: &str, store: &MediaStore) -> io::Result<()> {
    let url = Url::parse(url).map_err(|_| invalid())?;
    let total = match store.total() {
        Some(total) => total,
        None => match net.initial(&url, CHUNK)? {
            Initial::Bytes(reply) if reply.ranged && !is_playlist(&reply.data) => {
                let total = reply
                    .total
                    .filter(|n| (1..=MAX_MEDIA_BYTES).contains(n))
                    .ok_or_else(|| invalid())?;
                keep(Some(store), total, 0, &reply.data);
                total
            }
            _ => return Ok(()),
        },
    };
    // A file larger than the whole cache would only evict the rest of it.
    if !store.fits(total) {
        return Ok(());
    }
    for index in 0..total.div_ceil(CHUNK) {
        if !store.revision_current() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Audio cache policy changed",
            ));
        }
        let start = index * CHUNK;
        if store.has_byte(start) {
            continue;
        }
        let end = (start + CHUNK).min(total) - 1;
        let reply = net.get("download", &url, Some((start, end)), CHUNK as usize)?;
        if !reply.ranged || reply.total != Some(total) || reply.data.len() as u64 != end - start + 1
        {
            return Err(invalid());
        }
        if !store.write(total, index, &reply.data) {
            // No room, or the cache was cleared: playback goes on streaming.
            return Ok(());
        }
    }
    Ok(())
}
fn is_playlist(data: &[u8]) -> bool {
    data.starts_with(b"#EXTM3U") || data.starts_with(b"\xef\xbb\xbf#EXTM3U")
}
/// Store a whole chunk as it arrives; a short answer is not one.
fn keep(store: Option<&MediaStore>, total: u64, start: u64, data: &[u8]) {
    if let Some(store) = store
        && start.is_multiple_of(CHUNK)
        && data.len() as u64 == (total - start).min(CHUNK)
    {
        store.write(total, start / CHUNK, data);
    }
}
pub(super) fn from_net(
    net: Net,
    url: Url,
    offset: f64,
    store: Option<MediaStore>,
) -> io::Result<(Box<dyn MediaSource>, f64)> {
    let first = net.initial(&url, CHUNK)?;
    match first {
        Initial::File(mut file) => {
            let bytes = file.seek(SeekFrom::End(0))?;
            file.seek(SeekFrom::Start(0))?;
            log_source("whole_file", bytes, offset);
            Ok((Box::new(Spool(file)), 0.0))
        }
        Initial::Bytes(mut reply) if is_playlist(&reply.data) => {
            if reply
                .total
                .is_some_and(|total| total > reply.data.len() as u64)
            {
                reply = net.get("playlist", &reply.url, None, 1024 * 1024)?;
            }
            let stream = Hls::new(net, reply.url, reply.data, offset)?;
            let origin = stream.origin;
            Ok((Box::new(stream), origin))
        }
        Initial::Bytes(reply) if !reply.ranged => {
            log_source("small_file", reply.data.len() as u64, offset);
            Ok((Box::new(Cursor::new(reply.data)), 0.0))
        }
        Initial::Bytes(reply) => {
            let total = reply
                .total
                .filter(|n| (1..=MAX_MEDIA_BYTES).contains(n))
                .ok_or_else(|| invalid())?;
            log_source("ranges", total, offset);
            keep(store.as_ref(), total, 0, &reply.data);
            Ok((
                Box::new(Http {
                    net,
                    url: reply.url,
                    total,
                    position: 0,
                    start: 0,
                    buffer: reply.data,
                    store,
                }),
                0.0,
            ))
        }
    }
}
fn log_source(kind: &str, bytes: u64, offset: f64) {
    crate::diagnostics::event(
        "decoder",
        &format!(
            "source kind={kind} mib={:.2} offset_s={offset:.2}",
            bytes as f64 / 1048576.0
        ),
    );
}
struct Spool(tempfile::NamedTempFile);
impl Read for Spool {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.0.read(out)
    }
}
impl Seek for Spool {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.0.seek(from)
    }
}
impl MediaSource for Spool {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.0.as_file().metadata().ok().map(|m| m.len())
    }
}
/// A file read in chunk-aligned ranges: from the cache when a chunk is
/// there, otherwise from VK, keeping each whole chunk that arrives.
struct Http {
    net: Net,
    url: Url,
    total: u64,
    position: u64,
    start: u64,
    buffer: Vec<u8>,
    store: Option<MediaStore>,
}
impl Read for Http {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.total {
            return Ok(0);
        }
        if self.position < self.start || self.position >= self.start + self.buffer.len() as u64 {
            let index = self.position / CHUNK;
            let start = index * CHUNK;
            if let Some(data) = self
                .store
                .as_ref()
                .and_then(|store| store.read(self.total, index))
            {
                self.start = start;
                self.buffer = data;
            } else {
                let end = (start + CHUNK - 1).min(self.total - 1);
                let Reply {
                    data,
                    total,
                    ranged,
                    ..
                } = self
                    .net
                    .get("range", &self.url, Some((start, end)), CHUNK as usize)?;
                if !ranged
                    || total != Some(self.total)
                    || (data.len() as u64) <= self.position - start
                    || data.len() as u64 > end - start + 1
                {
                    return Err(invalid());
                }
                keep(self.store.as_ref(), self.total, start, &data);
                self.start = start;
                self.buffer = data;
            }
        }
        let at = (self.position - self.start) as usize;
        let count = out.len().min(self.buffer.len() - at);
        out[..count].copy_from_slice(&self.buffer[at..at + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for Http {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.total) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
        };
        if !(0..=i128::from(self.total)).contains(&next) {
            return Err(invalid());
        }
        self.position = next as u64;
        Ok(self.position)
    }
}
impl MediaSource for Http {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.total)
    }
}
/// A file kept whole in the cache. A chunk that went missing since (the
/// cache was cleared) ends the stream with an error, as a lost network would.
struct Stored {
    store: MediaStore,
    total: u64,
    position: u64,
    start: u64,
    buffer: Vec<u8>,
}
impl Read for Stored {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.total {
            return Ok(0);
        }
        if self.position < self.start || self.position >= self.start + self.buffer.len() as u64 {
            let index = self.position / CHUNK;
            self.buffer = self.store.read(self.total, index).ok_or_else(|| {
                crate::diagnostics::event("decoder", "cache_chunk_missing");
                invalid()
            })?;
            self.start = index * CHUNK;
        }
        let at = (self.position - self.start) as usize;
        let count = out.len().min(self.buffer.len() - at);
        out[..count].copy_from_slice(&self.buffer[at..at + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for Stored {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.total) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
        };
        if !(0..=i128::from(self.total)).contains(&next) {
            return Err(invalid());
        }
        self.position = next as u64;
        Ok(self.position)
    }
}
impl MediaSource for Stored {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.total)
    }
}
