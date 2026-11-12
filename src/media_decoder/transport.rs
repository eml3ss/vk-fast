//! MPEG-TS audio extraction. No video, external demuxer or process is involved.
use super::net::invalid;
use std::io;

#[derive(Default)]
pub(super) struct Transport {
    pmt: Option<u16>,
    audio: Option<u16>,
    pub first_pts: Option<u64>,
    pes: Vec<u8>,
    psi: Vec<u8>,
    pub stream_type: Option<u8>,
    continuity: Option<u8>,
    last_payload: Vec<u8>,
}
impl Transport {
    pub fn push(&mut self, data: &[u8], end: bool) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        let start = (0..data.len().min(192))
            .find(|i| data[*i] == 0x47 && (*i + 188 >= data.len() || data[*i + 188] == 0x47))
            .ok_or_else(|| invalid())?;
        if !(data.len() - start).is_multiple_of(188) {
            return Err(invalid());
        }
        for packet in data[start..].as_chunks::<188>().0 {
            if packet[0] != 0x47 || packet[1] & 0x80 != 0 || packet[3] & 0xc0 != 0 {
                return Err(invalid());
            }
            let pid = (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
            let control = (packet[3] >> 4) & 3;
            if control == 0 {
                return Err(invalid());
            }
            if control == 2 {
                continue;
            }
            let offset = if control == 3 {
                5 + usize::from(packet[4])
            } else {
                4
            };
            if offset > 188 {
                return Err(invalid());
            }
            let payload = &packet[offset..];
            if payload.is_empty() {
                continue;
            }
            let start = packet[1] & 0x40 != 0;
            if pid == 0 || Some(pid) == self.pmt {
                if start {
                    let pointer = usize::from(payload[0]) + 1;
                    if pointer > payload.len() {
                        return Err(invalid());
                    }
                    self.psi.clear();
                    self.psi.extend_from_slice(&payload[pointer..]);
                } else {
                    self.psi.extend_from_slice(payload);
                }
                self.section(pid)?;
            } else if Some(pid) == self.audio {
                let sequence = packet[3] & 15;
                if self.continuity == Some(sequence) && self.last_payload == payload {
                    continue;
                }
                if self
                    .continuity
                    .is_some_and(|last| (last + 1) & 15 != sequence)
                    && !start
                {
                    return Err(invalid());
                }
                self.continuity = Some(sequence);
                self.last_payload.clear();
                self.last_payload.extend_from_slice(payload);
                if start {
                    self.flush(&mut output, false)?;
                    self.pes.clear();
                }
                if start || !self.pes.is_empty() {
                    self.pes.extend_from_slice(payload);
                }
                if self.pes.len() > 1024 * 1024 {
                    return Err(invalid());
                }
                if self.pes.len() >= 6 {
                    let len = u16::from_be_bytes([self.pes[4], self.pes[5]]) as usize;
                    if len > 0 && self.pes.len() >= len + 6 {
                        self.flush(&mut output, true)?;
                        self.pes.clear();
                    }
                }
            }
        }
        if end {
            self.flush(&mut output, true)?;
            self.pes.clear();
        }
        Ok(output)
    }
    fn section(&mut self, pid: u16) -> io::Result<()> {
        if self.psi.len() < 3 {
            return Ok(());
        }
        let len = ((usize::from(self.psi[1] & 15) << 8) | usize::from(self.psi[2])) + 3;
        if len > 4096 {
            return Err(invalid());
        }
        if self.psi.len() < len {
            return Ok(());
        }
        let p = &self.psi[..len];
        if pid == 0 && p[0] == 0 && len >= 12 {
            for entry in p[8..len - 4].as_chunks::<4>().0 {
                if entry[0] != 0 || entry[1] != 0 {
                    self.pmt = Some((u16::from(entry[2] & 31) << 8) | u16::from(entry[3]));
                    break;
                }
            }
        } else if Some(pid) == self.pmt && p[0] == 2 && len >= 16 {
            let mut index = 12 + ((usize::from(p[10] & 15) << 8) | usize::from(p[11]));
            while index + 5 <= len - 4 {
                let kind = p[index];
                let audio = (u16::from(p[index + 1] & 31) << 8) | u16::from(p[index + 2]);
                if matches!(kind, 0x03 | 0x04 | 0x0f) && self.audio.is_none() {
                    self.audio = Some(audio);
                    self.stream_type = Some(kind);
                }
                index += 5 + ((usize::from(p[index + 3] & 15) << 8) | usize::from(p[index + 4]));
            }
        }
        self.psi.clear();
        Ok(())
    }
    fn flush(&mut self, out: &mut Vec<u8>, complete: bool) -> io::Result<()> {
        if self.pes.is_empty() {
            return Ok(());
        }
        let p = &self.pes;
        if p.len() < 9 || p[..3] != [0, 0, 1] {
            return Err(invalid());
        }
        let header = 9 + usize::from(p[8]);
        if header > p.len() {
            return Err(invalid());
        }
        let len = u16::from_be_bytes([p[4], p[5]]) as usize;
        if complete && len > 0 && p.len() < len + 6 {
            return Err(invalid());
        }
        if self.first_pts.is_none() && p[7] & 0x80 != 0 && p[8] >= 5 {
            let t = &p[9..14];
            self.first_pts = Some(
                (u64::from(t[0] & 14) << 29)
                    | (u64::from(t[1]) << 22)
                    | (u64::from(t[2] & 254) << 14)
                    | (u64::from(t[3]) << 7)
                    | u64::from(t[4] >> 1),
            );
        }
        let end = if len == 0 {
            p.len()
        } else {
            (len + 6).min(p.len())
        };
        if header > end {
            return Err(invalid());
        }
        out.extend_from_slice(&p[header..end]);
        Ok(())
    }
}
