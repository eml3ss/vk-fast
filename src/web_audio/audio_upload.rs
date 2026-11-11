//! VK's native audio upload API: request server, stream MP3, save once.
use super::*;
use crate::audio_upload::{self, Error, Source};
use crate::diagnostics::SendLogged;
use std::io::{self, Cursor};

impl WebAudioClient {
    pub fn upload_audio(
        &self,
        owner: i64,
        source: &Source,
        title: &str,
        artist: &str,
        canceled: &Arc<AtomicBool>,
        progress: impl Fn(u64, u64) + Send + 'static,
    ) -> Result<Track, Error> {
        source.unchanged()?;
        self.upload_audio_with(owner, title, artist, canceled,
            |generation,method,params| self.detail_request(owner,generation,method,params,canceled),
            |generation,url| {
                if !playlist_manage::valid_upload_url(url) { return Err(Error::Web(WebAudioError::Format)); }
                self.commit_home_streams(owner,generation,HashMap::new(),canceled)?;
                let mut random=[0;16]; getrandom::fill(&mut random).map_err(|_| Error::Read)?;
                let boundary=format!("vk-fast-audio-{}",random.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
                let header=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\n").into_bytes();
                let footer=format!("\r\n--{boundary}--\r\n").into_bytes();
                let length=header.len() as u64 + source.size + footer.len() as u64;
                let reader=ProgressReader { file:source.reader()?.take(source.size),
                    canceled: canceled.clone(), done:0,total:source.size,progress, last:0 };
                let upload_body=Cursor::new(header).chain(reader).chain(Cursor::new(footer));
                let response=self.http.post(url).timeout(Duration::from_secs(600))
                    .header(CONTENT_TYPE,format!("multipart/form-data; boundary={boundary}"))
                    .body(reqwest::blocking::Body::sized(upload_body,length)).send_logged("vk-http","audio upload").map_err(network)?;
                let text=body(response)?;
                self.commit_home_streams(owner,generation,HashMap::new(),canceled)?;
                source.unchanged()?;
                serde_json::from_str(&text).map_err(|_| Error::Web(WebAudioError::Format))
            })
    }

    fn upload_audio_with(
        &self,
        owner: i64,
        title: &str,
        artist: &str,
        canceled: &AtomicBool,
        mut request: impl FnMut(
            u64,
            &'static str,
            Vec<(&'static str, String)>,
        ) -> Result<Value, WebAudioError>,
        upload: impl FnOnce(u64, &str) -> Result<Value, Error>,
    ) -> Result<Track, Error> {
        if !audio_upload::valid_text(title) || !audio_upload::valid_text(artist) {
            return Err(Error::Metadata);
        }
        let generation = self.home_owner(owner, canceled)?;
        let server = request(generation, "audio.getUploadServer", vec![])?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let url = server
            .get("upload_url")
            .and_then(Value::as_str)
            .filter(|url| playlist_manage::valid_upload_url(url))
            .ok_or(WebAudioError::Format)?;
        let uploaded = upload(generation, url)?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        if uploaded.get("error").is_some()
            || uploaded.get("error_code").is_some()
            || uploaded
                .get("code")
                .is_some_and(|code| code.as_i64() != Some(0))
        {
            return Err(WebAudioError::Rejected.into());
        }
        let mut params = Vec::new();
        for key in ["server", "audio", "hash"] {
            let value = uploaded
                .get(key)
                .and_then(|value| match value {
                    Value::String(s) if !s.is_empty() && s.len() <= 64 * 1024 => Some(s.clone()),
                    Value::Number(n) if key == "server" && n.as_u64().is_some_and(|id| id > 0) => {
                        Some(n.to_string())
                    }
                    _ => None,
                })
                .ok_or(WebAudioError::Format)?;
            params.push((key, value));
        }
        params.extend([
            ("title", title.trim().into()),
            ("artist", artist.trim().into()),
        ]);
        // A failed save response is never automatically retried: the request
        // may already have committed on VK and repeating it could add a copy.
        let saved = request(generation, "audio.save", params).map_err(|error| match error {
            WebAudioError::Network
            | WebAudioError::Timeout
            | WebAudioError::Format
            | WebAudioError::Http(_) => Error::Uncertain,
            other => Error::Web(other),
        })?;
        self.commit_home_streams(owner, generation, HashMap::new(), canceled)?;
        let mut saved = if let Some(array) = saved.as_array() {
            if array.len() != 1 {
                return Err(Error::Uncertain);
            }
            array[0].clone()
        } else {
            saved
        };
        if saved.get("owner_id").and_then(number) != Some(owner)
            || saved.get("id").and_then(number).is_none_or(|id| id <= 0)
        {
            return Err(Error::Uncertain);
        }
        let object = saved.as_object_mut().ok_or(Error::Uncertain)?;
        object
            .entry("title")
            .or_insert_with(|| Value::String(title.trim().into()));
        object
            .entry("artist")
            .or_insert_with(|| Value::String(artist.trim().into()));
        object.entry("duration").or_insert(Value::from(0));
        let mut track = parse_track(&saved).map_err(|_| Error::Uncertain)?;
        track.library_liked = Some(true);
        Ok(track)
    }
}

struct ProgressReader<F> {
    file: std::io::Take<std::fs::File>,
    canceled: Arc<AtomicBool>,
    done: u64,
    total: u64,
    progress: F,
    last: u64,
}
impl<F: Fn(u64, u64)> Read for ProgressReader<F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(io::Error::other("Upload canceled"));
        }
        let read = self.file.read(output)?;
        self.done += read as u64;
        let percent = self.done.saturating_mul(100) / self.total.max(1);
        if percent != self.last {
            self.last = percent;
            (self.progress)(self.done, self.total);
        }
        Ok(read)
    }
}

#[cfg(test)]
#[path = "audio_upload_tests.rs"]
mod tests;
