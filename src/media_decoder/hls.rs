//! Audio HLS: HTTPS playlists, AES-128 transport protection, TS and fMP4.
use super::{
    net::{Net, invalid},
    transport::Transport,
};
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use m3u8_rs::{Key, KeyMethod, MediaSegment, Playlist};
use reqwest::Url;
use std::{
    collections::HashMap,
    io::{self, Cursor, Read, Seek, SeekFrom},
};
use symphonia::core::io::MediaSource;
use zeroize::Zeroizing;
const SEGMENT_LIMIT: usize = 16 * 1024 * 1024;

pub(super) struct Hls {
    net: Net,
    base: Url,
    segments: Vec<MediaSegment>,
    ranges: Vec<Option<(u64, u64)>>,
    sequence: u64,
    next: usize,
    buffer: Cursor<Vec<u8>>,
    transport: Option<Transport>,
    key: Option<(Url, Zeroizing<Vec<u8>>)>,
    position: u64,
    pub origin: f64,
    init: Option<String>,
    kind: Option<bool>,
}
impl Hls {
    pub fn new(net: Net, mut base: Url, mut bytes: Vec<u8>, offset: f64) -> io::Result<Self> {
        let mut selected = None;
        for _ in 0..6 {
            let body = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
            let text = std::str::from_utf8(body).map_err(|_| invalid())?;
            // A cleared key has no IV. Supply an ignored value for parser
            // validation; METHOD=NONE still means clear transport bytes.
            let text = text
                .lines()
                .map(|line| {
                    if line.starts_with("#EXT-X-KEY:METHOD=NONE") && !line.contains("IV=") {
                        format!("{line},IV=0x0")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            match m3u8_rs::parse_playlist_res(text.as_bytes()).map_err(|_| invalid())? {
                Playlist::MediaPlaylist(list) => {
                    selected = Some(list);
                    break;
                }
                Playlist::MasterPlaylist(master) => {
                    let variant = master
                        .variants
                        .iter()
                        .filter(|v| !v.is_i_frame)
                        .filter(|v| v.resolution.is_none())
                        .max_by_key(|v| v.bandwidth)
                        .or_else(|| {
                            master
                                .variants
                                .iter()
                                .filter(|v| !v.is_i_frame)
                                .min_by_key(|v| v.bandwidth)
                        })
                        .ok_or_else(|| invalid())?;
                    let audio = variant
                        .audio
                        .as_ref()
                        .and_then(|group| {
                            master
                                .alternatives
                                .iter()
                                .find(|a| a.group_id == *group && a.uri.is_some())
                        })
                        .and_then(|a| a.uri.as_deref())
                        .unwrap_or(&variant.uri);
                    base = base.join(audio).map_err(|_| invalid())?;
                    let reply = net.get("playlist", &base, None, 1024 * 1024)?;
                    base = reply.url;
                    bytes = reply.data;
                }
            }
        }
        let mut list = selected.ok_or_else(|| invalid())?;
        if list.segments.is_empty() || list.segments.len() > 100_000 || list.i_frames_only {
            return Err(invalid());
        }
        let mut last_key = None;
        let mut last_map = None;
        let mut ranges = Vec::new();
        let mut range_end = HashMap::new();
        let mut times = Vec::new();
        let mut time = 0.0;
        for segment in &mut list.segments {
            if !segment.duration.is_finite() || segment.duration <= 0.0 || segment.duration > 3600.0
            {
                return Err(invalid());
            }
            times.push(time);
            time += f64::from(segment.duration);
            if let Some(key) = &segment.key {
                last_key = Some(key.clone());
            } else {
                segment.key = last_key.clone();
            }
            if let Some(map) = &segment.map {
                last_map = Some(map.clone());
            } else {
                segment.map = last_map.clone();
            }
            let range = if let Some(range) = &segment.byte_range {
                if range.length == 0 || range.length > SEGMENT_LIMIT as u64 {
                    return Err(invalid());
                }
                let start = range
                    .offset
                    .unwrap_or_else(|| *range_end.get(&segment.uri).unwrap_or(&0));
                let end = start.checked_add(range.length).ok_or_else(|| invalid())?;
                range_end.insert(segment.uri.clone(), end);
                Some((start, end - 1))
            } else {
                None
            };
            ranges.push(range);
        }
        let next = times
            .partition_point(|t| *t <= offset.max(0.0))
            .saturating_sub(1)
            .saturating_sub(2);
        // Fragment readers start their stream clock at the first visible
        // fragment. Restore its position on the complete playlist timeline.
        let origin = times[next];
        let protected = list
            .segments
            .iter()
            .filter(|segment| {
                segment
                    .key
                    .as_ref()
                    .is_some_and(|key| key.method != KeyMethod::None)
            })
            .count();
        let breaks = list.segments.iter().filter(|s| s.discontinuity).count();
        crate::diagnostics::event(
            "decoder",
            &format!(
                "source kind=hls segments={} playlist_s={time:.2} start_segment={next} \
                 start_s={origin:.2} offset_s={offset:.2} protected={protected} \
                 discontinuities={breaks} init={}",
                list.segments.len(),
                list.segments.iter().any(|s| s.map.is_some()),
            ),
        );
        let mut stream = Self {
            net,
            base,
            segments: list.segments,
            ranges,
            sequence: list.media_sequence,
            next,
            buffer: Cursor::new(Vec::new()),
            transport: None,
            key: None,
            position: 0,
            origin,
            init: None,
            kind: None,
        };
        // Resolve the TS media clock once. Playlist durations can be rounded;
        // timestamps keep a tail seek on the same timeline as a full decode.
        if next > 0 && stream.segments[next].map.is_none() {
            let head = stream.segment(0)?;
            if is_transport(&head) {
                let mut first = Transport::default();
                let _ = first.push(&head, false)?;
                let tail = stream.segment(next)?;
                let mut selected = Transport::default();
                let decoded = selected.push(&tail, false)?;
                if let (Some(base), Some(start)) = (first.first_pts, selected.first_pts) {
                    let delta = start.wrapping_sub(base) & ((1 << 33) - 1);
                    if delta < 1 << 32 {
                        stream.origin = delta as f64 / 90_000.0;
                    }
                    crate::diagnostics::event(
                        "decoder",
                        &format!(
                            "hls ts_clock start_s={:.2} playlist_start_s={origin:.2}",
                            delta as f64 / 90_000.0
                        ),
                    );
                }
                stream.kind = Some(true);
                stream.transport = Some(selected);
                stream.buffer = Cursor::new(decoded);
                stream.next += 1;
            }
        }
        Ok(stream)
    }
    fn segment(&mut self, index: usize) -> io::Result<Vec<u8>> {
        let segment = self.segments[index].clone();
        let url = self.base.join(&segment.uri).map_err(|_| invalid())?;
        let reply = self
            .net
            .get("segment", &url, self.ranges[index], SEGMENT_LIMIT)?;
        if let Some((start, end)) = self.ranges[index]
            && (!reply.ranged || reply.data.len() as u64 != end - start + 1)
        {
            return Err(invalid());
        }
        self.decrypt(reply.data, segment.key.as_ref(), index)
    }
    fn decrypt(&mut self, data: Vec<u8>, key: Option<&Key>, index: usize) -> io::Result<Vec<u8>> {
        let Some(key) = key.filter(|key| key.method != KeyMethod::None) else {
            return Ok(data);
        };
        if key.method != KeyMethod::AES128
            || key.keyformat.as_deref().is_some_and(|s| s != "identity")
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unsupported media protection",
            ));
        }
        let url = self
            .base
            .join(key.uri.as_deref().ok_or_else(|| invalid())?)
            .map_err(|_| invalid())?;
        if self.key.as_ref().is_none_or(|(cached, _)| *cached != url) {
            let bytes = self.net.get("key", &url, None, 16)?.data;
            if bytes.len() != 16 {
                return Err(invalid());
            }
            self.key = Some((url, Zeroizing::new(bytes)));
        }
        let iv = if let Some(iv) = key.iv.as_deref() {
            parse_iv(iv)?
        } else {
            u128::from(
                self.sequence
                    .checked_add(index as u64)
                    .ok_or_else(|| invalid())?,
            )
            .to_be_bytes()
        };
        let bytes = &self.key.as_ref().ok_or_else(|| invalid())?.1;
        cbc::Decryptor::<aes::Aes128>::new_from_slices(bytes, &iv)
            .map_err(|_| invalid())?
            .decrypt_padded_vec_mut::<Pkcs7>(&data)
            .map_err(|_| invalid())
    }
    fn advance(&mut self) -> io::Result<bool> {
        loop {
            if self.next >= self.segments.len() {
                if self.next == self.segments.len() {
                    crate::diagnostics::event(
                        "decoder",
                        &format!("hls end segments={}", self.segments.len()),
                    );
                    self.next += 1;
                }
                return Ok(false);
            }
            let index = self.next;
            let segment = self.segments[index].clone();
            if let Some(map) = &segment.map
                && self.init.as_ref() != Some(&map.uri)
            {
                let url = self.base.join(&map.uri).map_err(|_| invalid())?;
                let range = map.byte_range.as_ref().map(|r| {
                    let start = r.offset.unwrap_or(0);
                    (start, start.saturating_add(r.length).saturating_sub(1))
                });
                let reply = self.net.get("init", &url, range, 8 * 1024 * 1024)?;
                let data = self.decrypt(reply.data, segment.key.as_ref(), index)?;
                self.init = Some(map.uri.clone());
                self.buffer = Cursor::new(data);
                return Ok(true);
            }
            let data = self.segment(index).inspect_err(|error| {
                crate::diagnostics::event(
                    "decoder",
                    &format!(
                        "hls segment_failed index={index} of={} kind={:?} reason={error}",
                        self.segments.len(),
                        error.kind()
                    ),
                );
            })?;
            self.next += 1;
            let ts = self.kind.unwrap_or_else(|| is_transport(&data));
            self.kind = Some(ts);
            let bytes = if ts {
                if segment.discontinuity {
                    crate::diagnostics::event(
                        "decoder",
                        &format!("hls discontinuity index={index}"),
                    );
                    self.transport = Some(Transport::default());
                }
                self.transport
                    .get_or_insert_with(Transport::default)
                    .push(&data, self.next == self.segments.len())
                    .inspect_err(|error| {
                        crate::diagnostics::event(
                            "decoder",
                            &format!("hls ts_failed index={index} reason={error}"),
                        );
                    })?
            } else {
                data
            };
            if !bytes.is_empty() {
                self.buffer = Cursor::new(bytes);
                return Ok(true);
            }
        }
    }
}
impl Read for Hls {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.buffer.read(out)?;
            if count > 0 {
                self.position += count as u64;
                return Ok(count);
            }
            if !self.advance()? {
                return Ok(0);
            }
        }
    }
}
impl Seek for Hls {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        match from {
            SeekFrom::Current(0) => Ok(self.position),
            SeekFrom::Start(0) if self.position == 0 => Ok(0),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Streaming media source",
            )),
        }
    }
}
impl MediaSource for Hls {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}
fn is_transport(bytes: &[u8]) -> bool {
    (0..bytes.len().min(192)).any(|i| {
        i + 376 < bytes.len()
            && (bytes.len() - i).is_multiple_of(188)
            && bytes[i] == 0x47
            && bytes[i + 188] == 0x47
            && bytes[i + 376] == 0x47
    })
}
fn parse_iv(value: &str) -> io::Result<[u8; 16]> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() || value.len() > 32 || !value.is_ascii() {
        return Err(invalid());
    }
    let number = u128::from_str_radix(value, 16).map_err(|_| invalid())?;
    Ok(number.to_be_bytes())
}
