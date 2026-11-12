use super::{
    hls::Hls,
    net::{Initial, Net, Reply, invalid},
};
use reqwest::Url;
use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::{Arc, atomic::AtomicBool},
};
use symphonia::core::io::MediaSource;
const CHUNK: u64 = 256 * 1024;

pub(super) fn open(
    url: &str,
    offset: f64,
    canceled: Arc<AtomicBool>,
) -> io::Result<(Box<dyn MediaSource>, f64)> {
    let url = Url::parse(url).map_err(|_| invalid())?;
    let net = Net::new(canceled)?;
    from_net(net, url, offset)
}
pub(super) fn from_net(net: Net, url: Url, offset: f64) -> io::Result<(Box<dyn MediaSource>, f64)> {
    let first = net.initial(&url, CHUNK)?;
    match first {
        Initial::File(mut file) => {
            let bytes = file.seek(SeekFrom::End(0))?;
            file.seek(SeekFrom::Start(0))?;
            log_source("whole_file", bytes, offset);
            Ok((Box::new(Spool(file)), 0.0))
        }
        Initial::Bytes(mut reply)
            if reply.data.starts_with(b"#EXTM3U")
                || reply.data.starts_with(b"\xef\xbb\xbf#EXTM3U") =>
        {
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
                .filter(|n| *n > 0 && *n <= 512 * 1024 * 1024)
                .ok_or_else(|| invalid())?;
            log_source("ranges", total, offset);
            Ok((
                Box::new(Http {
                    net,
                    url: reply.url,
                    total,
                    position: 0,
                    start: 0,
                    buffer: reply.data,
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
struct Http {
    net: Net,
    url: Url,
    total: u64,
    position: u64,
    start: u64,
    buffer: Vec<u8>,
}
impl Read for Http {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.total {
            return Ok(0);
        }
        if self.position < self.start || self.position >= self.start + self.buffer.len() as u64 {
            let end = (self.position + CHUNK - 1).min(self.total - 1);
            let Reply {
                data,
                total,
                ranged,
                ..
            } = self.net.get(
                "range",
                &self.url,
                Some((self.position, end)),
                CHUNK as usize,
            )?;
            if !ranged
                || total != Some(self.total)
                || data.is_empty()
                || data.len() as u64 > end - self.position + 1
            {
                return Err(invalid());
            }
            self.start = self.position;
            self.buffer = data;
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
