//! Bounded HTTPS reads, canceled even while DNS, TLS or a stalled body waits.
use reqwest::{
    Client, Url,
    header::{CONTENT_RANGE, RANGE},
};
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct Net {
    runtime: Option<tokio::runtime::Runtime>,
    client: Client,
    canceled: Arc<AtomicBool>,
    #[cfg(test)]
    pub allow_local: bool,
}
pub(super) struct Reply {
    pub data: Vec<u8>,
    pub total: Option<u64>,
    pub ranged: bool,
    pub url: Url,
}
pub(super) enum Initial {
    Bytes(Reply),
    File(tempfile::NamedTempFile),
}
/// Malformed or refused media. The message names the code line that refused
/// it (never data from the stream), so a log shows exactly which check failed.
#[track_caller]
pub(super) fn invalid() -> io::Error {
    let location = std::panic::Location::caller();
    let file = location.file().rsplit(['/', '\\']).next().unwrap_or("");
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid media data at {file}:{}", location.line()),
    )
}
/// A media request that got no answer. Only the kind of failure is logged:
/// reqwest's own text would contain the private media address.
fn request_failed(what: &str, error: &reqwest::Error, started: Instant) -> io::Error {
    let cause = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() || error.is_decode() {
        "body"
    } else if error.is_redirect() {
        "redirect"
    } else if error.is_request() {
        "request"
    } else {
        "other"
    };
    crate::diagnostics::event(
        "decoder",
        &format!(
            "media_http {what} failed cause={cause} ms={}",
            started.elapsed().as_millis()
        ),
    );
    io::Error::other(format!("media request failed: {what} {cause}"))
}
fn refused(what: &str, status: u16, started: Instant) -> io::Error {
    crate::diagnostics::event(
        "decoder",
        &format!(
            "media_http {what} status={status} ms={}",
            started.elapsed().as_millis()
        ),
    );
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("media request refused: {what} status {status}"),
    )
}
// The log hides numbers of five digits and more as identifiers, so sizes
// are written in KiB and MiB.
fn log_reply(what: &str, ranged: bool, bytes: usize, total: Option<u64>, started: Instant) {
    crate::diagnostics::event(
        "decoder",
        &format!(
            "media_http {what} status={} kib={:.1} total_mib={} ms={}",
            if ranged { 206 } else { 200 },
            bytes as f64 / 1024.0,
            total.map_or_else(
                || "unknown".into(),
                |total| format!("{:.2}", total as f64 / 1048576.0)
            ),
            started.elapsed().as_millis()
        ),
    );
}
pub(super) fn canceled() -> io::Error {
    io::Error::other("Media operation canceled")
}
pub(super) fn valid_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|p| p == 443)
        && url.fragment().is_none()
        && url.as_str().len() <= 16384
}
impl Net {
    pub fn new(canceled: Arc<AtomicBool>) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = Client::builder()
            .default_headers(reqwest::header::HeaderMap::from_iter([(
                reqwest::header::ACCEPT_ENCODING,
                reqwest::header::HeaderValue::from_static("identity"),
            )]))
            .user_agent(crate::client_platform::USER_AGENT)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(45))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 5 || !valid_url(attempt.url()) {
                    attempt.error("Media redirect rejected")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            runtime: Some(runtime),
            client,
            canceled,
            #[cfg(test)]
            allow_local: false,
        })
    }
    pub fn check_url(&self, url: &Url) -> io::Result<()> {
        #[cfg(test)]
        if self.allow_local
            && url.scheme() == "http"
            && url
                .host_str()
                .is_some_and(|h| matches!(h, "127.0.0.1" | "localhost"))
        {
            return Ok(());
        }
        if valid_url(url) {
            Ok(())
        } else {
            Err(invalid())
        }
    }
    /// `what` names the request in the log (playlist, segment, key, range…).
    pub fn get(
        &self,
        what: &'static str,
        url: &Url,
        range: Option<(u64, u64)>,
        limit: usize,
    ) -> io::Result<Reply> {
        self.check_url(url)?;
        let started = Instant::now();
        let future = async {
            let mut request = self.client.get(url.clone());
            if let Some((start, end)) = range {
                request = request.header(RANGE, format!("bytes={start}-{end}"));
            }
            let mut response = request
                .send()
                .await
                .map_err(|error| request_failed(what, &error, started))?;
            if !response.status().is_success() {
                return Err(refused(what, response.status().as_u16(), started));
            }
            self.check_url(response.url())?;
            let ranged = response.status().as_u16() == 206;
            let total = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.parse().ok())
                .or_else(|| (!ranged).then(|| response.content_length()).flatten());
            if ranged
                && range.is_some_and(|(start, _)| {
                    !response
                        .headers()
                        .get(CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.starts_with(&format!("bytes {start}-")))
                })
            {
                return Err(invalid());
            }
            let mut data = Vec::new();
            while let Some(bytes) = response
                .chunk()
                .await
                .map_err(|error| request_failed(what, &error, started))?
            {
                if data.len().saturating_add(bytes.len()) > limit {
                    crate::diagnostics::event(
                        "decoder",
                        &format!("media_http {what} too_large limit_kib={}", limit / 1024),
                    );
                    return Err(invalid());
                }
                data.extend_from_slice(&bytes);
            }
            log_reply(what, ranged, data.len(), total, started);
            Ok(Reply {
                data,
                total,
                ranged,
                url: response.url().clone(),
            })
        };
        self.run(future)
    }
    pub fn initial(&self, url: &Url, chunk: u64) -> io::Result<Initial> {
        self.check_url(url)?;
        let started = Instant::now();
        let what = "initial";
        self.run(async {
            let mut response = self
                .client
                .get(url.clone())
                .header(RANGE, format!("bytes=0-{}", chunk - 1))
                .send()
                .await
                .map_err(|error| request_failed(what, &error, started))?;
            if !response.status().is_success() {
                return Err(refused(what, response.status().as_u16(), started));
            }
            self.check_url(response.url())?;
            let ranged = response.status().as_u16() == 206;
            let total = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.parse().ok());
            let mut data = Vec::new();
            let mut file = None;
            let mut received = 0u64;
            while let Some(bytes) = response
                .chunk()
                .await
                .map_err(|error| request_failed(what, &error, started))?
            {
                received += bytes.len() as u64;
                if received > 512 * 1024 * 1024 {
                    return Err(invalid());
                }
                if let Some(file) = &mut file {
                    Write::write_all(file, &bytes)?;
                    continue;
                }
                data.extend_from_slice(&bytes);
                if ranged && data.len() > chunk as usize {
                    return Err(invalid());
                }
                let playlist =
                    data.starts_with(b"#EXTM3U") || data.starts_with(b"\xef\xbb\xbf#EXTM3U");
                if playlist && data.len() > 1024 * 1024 {
                    return Err(invalid());
                }
                if !ranged && !playlist && data.len() > chunk as usize {
                    let mut spool = tempfile::NamedTempFile::new()?;
                    spool.write_all(&data)?;
                    data.clear();
                    file = Some(spool);
                }
            }
            log_reply(what, ranged, received as usize, total, started);
            if let Some(file) = file {
                return Ok(Initial::File(file));
            }
            Ok(Initial::Bytes(Reply {
                data,
                total,
                ranged,
                url: response.url().clone(),
            }))
        })
    }
    fn run<T>(&self, future: impl std::future::Future<Output = io::Result<T>>) -> io::Result<T> {
        self.runtime.as_ref().ok_or_else(|| invalid())?.block_on(async {
            tokio::select! {
                biased;
                _=async {loop {if self.canceled.load(Ordering::Acquire) {break;}tokio::time::sleep(Duration::from_millis(10)).await;}}=>Err(canceled()),
                result=future=>result,
            }
        })
    }
}
impl Drop for Net {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}
