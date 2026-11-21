//! VK's pages, API and sign-in reached as the Chrome the client says it is.
//!
//! The client names itself Chrome (`client_platform::USER_AGENT`). With the
//! usual Rust TLS stack it still greeted VK unlike any Chrome: another TLS
//! handshake, HTTP/1.1 where Chrome speaks HTTP/2, other headers. VK's
//! anti-fraud can tell, and such a mismatch is a bot's mark. Here `wreq`
//! (BoringSSL, as Chrome itself) sends the same TLS handshake, HTTP/2
//! settings and header order as that Chrome version on this platform, over
//! one pooled connection, as a browser tab does.
//!
//! The rest of the client is synchronous: requests run on a small shared
//! runtime and the calling worker thread waits. The API mirrors the part of
//! `reqwest::blocking` the VK code uses.
use std::{
    io::{self, Read},
    sync::{Arc, OnceLock},
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{
    Method, StatusCode, Url,
    cookie::{CookieStore, Jar},
    header::{
        ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName,
        HeaderValue, LOCATION, ORIGIN, REFERER, SET_COOKIE, USER_AGENT,
    },
};
use zeroize::Zeroizing;

/// The Chrome version both the handshake and the user agent name.
const PROFILE: wreq_util::Profile = wreq_util::Profile::Chrome149;
/// `PROFILE`'s version, which `client_platform::USER_AGENT` must name too.
#[cfg(test)]
const CHROME_VERSION: &str = "149";
/// No answer from VK is anywhere near this; more is cut off.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// Redirects followed at most, as `reqwest`'s custom policies here did.
const MAX_REDIRECTS: usize = 5;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("vk-http")
            .enable_all()
            .build()
            .expect("the HTTP runtime starts")
    })
}

/// Chrome's `sec-ch-ua` for `PROFILE`.
const SEC_CH_UA: &str = r#""Google Chrome";v="149", "Chromium";v="149", "Not)A;Brand";v="24""#;

/// The languages a browser in this system's language asks for, as Chrome
/// sends them: Russian systems first ask for Russian.
fn accept_language() -> &'static str {
    static LANGUAGE: OnceLock<&'static str> = OnceLock::new();
    LANGUAGE.get_or_init(|| {
        if sys_locale::get_locale().is_some_and(|locale| locale.to_lowercase().starts_with("ru")) {
            "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7"
        } else {
            "en-US,en;q=0.9"
        }
    })
}

/// Where a page's script sends a request, as `sec-fetch-site` says.
#[derive(Clone, Copy)]
pub enum FetchSite {
    /// To the page's own host (vk.ru's `al_audio.php` from vk.ru).
    SameOrigin,
    /// To another host of the same site (`web.api.vk.ru`, `login.vk.ru`).
    SameSite,
    /// To another site (an upload server).
    CrossSite,
}

fn platform() -> wreq_util::Platform {
    if cfg!(target_os = "windows") {
        wreq_util::Platform::Windows
    } else {
        wreq_util::Platform::Linux
    }
}

/// Why a request failed: only its kind, never an address.
#[derive(Debug)]
pub struct Error {
    timeout: bool,
    connect: bool,
}

impl Error {
    fn other() -> Self {
        Self {
            timeout: false,
            connect: false,
        }
    }
    fn from_wreq(error: &wreq::Error) -> Self {
        Self {
            timeout: error.is_timeout(),
            connect: error.is_connect(),
        }
    }
    pub fn is_timeout(&self) -> bool {
        self.timeout
    }
    pub fn is_connect(&self) -> bool {
        self.connect
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(if self.timeout {
            "timed out"
        } else if self.connect {
            "could not connect"
        } else {
            "request failed"
        })
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Which redirects a client follows itself.
#[derive(Clone, Copy)]
pub enum Redirects {
    /// None: the caller reads each 3xx answer (VK's own pages do).
    None,
    /// Up to five, to addresses `allowed` accepts; another is returned as is.
    Follow(fn(&Url) -> bool),
}

pub struct ClientBuilder {
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    redirects: Redirects,
    jar: Option<Arc<Jar>>,
}

impl ClientBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }
    pub fn redirects(mut self, redirects: Redirects) -> Self {
        self.redirects = redirects;
        self
    }
    /// Cookies kept in `jar`: sent with each request, every hop of a
    /// redirect included, and stored from each answer.
    pub fn cookie_jar(mut self, jar: Arc<Jar>) -> Self {
        self.jar = Some(jar);
        self
    }
    pub fn build(self) -> Result<Client> {
        let _inside = runtime().enter();
        let emulation = wreq_util::Emulation::builder()
            .profile(PROFILE)
            .platform(platform())
            .build();
        let mut builder = wreq::Client::builder()
            .emulation(emulation)
            .redirect(wreq::redirect::Policy::none());
        if let Some(timeout) = self.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(timeout) = self.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        let inner = builder.build().map_err(|error| Error::from_wreq(&error))?;
        Ok(Client {
            inner,
            redirects: self.redirects,
            jar: self.jar,
        })
    }
}

#[derive(Clone)]
pub struct Client {
    inner: wreq::Client,
    redirects: Redirects,
    jar: Option<Arc<Jar>>,
}

/// An address given as text or already parsed.
pub trait IntoTarget {
    fn into_target(self) -> Result<Url>;
}

impl IntoTarget for Url {
    fn into_target(self) -> Result<Url> {
        Ok(self)
    }
}

impl IntoTarget for &Url {
    fn into_target(self) -> Result<Url> {
        Ok(self.clone())
    }
}

impl IntoTarget for &str {
    fn into_target(self) -> Result<Url> {
        Url::parse(self).map_err(|_| Error::other())
    }
}

impl IntoTarget for String {
    fn into_target(self) -> Result<Url> {
        self.as_str().into_target()
    }
}

impl IntoTarget for &String {
    fn into_target(self) -> Result<Url> {
        self.as_str().into_target()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            timeout: None,
            connect_timeout: None,
            redirects: Redirects::None,
            jar: None,
        }
    }
    pub fn get(&self, url: impl IntoTarget) -> RequestBuilder {
        self.request(Method::GET, url)
    }
    pub fn post(&self, url: impl IntoTarget) -> RequestBuilder {
        self.request(Method::POST, url)
    }
    fn request(&self, method: Method, url: impl IntoTarget) -> RequestBuilder {
        RequestBuilder {
            client: self.clone(),
            method,
            url: url.into_target(),
            headers: HeaderMap::new(),
            body: None,
            timeout: None,
            invalid: false,
            fetch: None,
        }
    }
}

pub struct RequestBuilder {
    client: Client,
    method: Method,
    url: Result<Url>,
    headers: HeaderMap,
    body: Option<Zeroizing<Vec<u8>>>,
    timeout: Option<Duration>,
    /// A header or parameter that could not be encoded: the request fails.
    invalid: bool,
    /// Sent as a page's script would (`fetch`), not as a page load.
    fetch: Option<FetchSite>,
}

impl RequestBuilder {
    pub fn header<K, V>(mut self, name: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        HeaderValue: TryFrom<V>,
    {
        match (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            (Ok(name), Ok(value)) => {
                self.headers.append(name, value);
            }
            _ => self.invalid = true,
        }
        self
    }

    /// Adds `parameters` to the address's query.
    pub fn query<T: serde::Serialize + ?Sized>(mut self, parameters: &T) -> Self {
        match (&mut self.url, serde_urlencoded::to_string(parameters)) {
            (Ok(url), Ok(encoded)) if !encoded.is_empty() => {
                let query = match url.query() {
                    Some(existing) if !existing.is_empty() => format!("{existing}&{encoded}"),
                    _ => encoded,
                };
                url.set_query(Some(&query));
            }
            (_, Ok(_)) => {}
            (_, Err(_)) => self.invalid = true,
        }
        self
    }

    /// An `application/x-www-form-urlencoded` body.
    pub fn form<T: serde::Serialize + ?Sized>(mut self, form: &T) -> Self {
        match serde_urlencoded::to_string(form) {
            Ok(body) => {
                self.headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                );
                self.body = Some(Zeroizing::new(body.into_bytes()));
            }
            Err(_) => self.invalid = true,
        }
        self
    }

    /// Sends it as a script on a VK page does (`fetch`/XHR): Chrome's
    /// headers for that, in its order, instead of a page load's.
    pub fn fetch(mut self, site: FetchSite) -> Self {
        self.fetch = Some(site);
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(Zeroizing::new(body.into()));
        self
    }

    /// Sends it and logs the request's kind, status and time (never the
    /// address), as `diagnostics::SendLogged` does for other clients.
    pub fn send_logged(self, component: &str, what: &str) -> Result<Response> {
        let started = std::time::Instant::now();
        let result = self.send();
        if crate::diagnostics::enabled() {
            let outcome = match &result {
                Ok(response) => format!("status={}", response.status().as_u16()),
                Err(error) if error.is_timeout() => "timed out".into(),
                Err(error) if error.is_connect() => "no connection".into(),
                Err(_) => "failed".into(),
            };
            crate::diagnostics::event(
                component,
                &format!("{what} {outcome} ms={}", started.elapsed().as_millis()),
            );
        }
        result
    }

    pub fn send(self) -> Result<Response> {
        if self.invalid {
            return Err(Error::other());
        }
        let Self {
            client,
            mut method,
            url,
            mut headers,
            mut body,
            timeout,
            fetch,
            ..
        } = self;
        let mut url = url?;
        runtime().block_on(async move {
            let mut hops = 0;
            loop {
                let mut request = client.inner.request(method.clone(), url.as_str());
                let mut sent = headers.clone();
                if let Some(cookies) = client.jar.as_ref().and_then(|jar| jar.cookies(&url)) {
                    sent.insert(COOKIE, cookies);
                }
                match fetch {
                    Some(site) => {
                        let (headers, order) = fetch_headers(&sent, site);
                        request = request
                            .default_headers(false)
                            .headers(headers)
                            .orig_headers(order);
                    }
                    None => {
                        sent.insert(ACCEPT_LANGUAGE, HeaderValue::from_static(accept_language()));
                        for (name, value) in &sent {
                            request = request.header(name.clone(), value.clone());
                        }
                    }
                }
                if let Some(body) = &body {
                    request = request.body(body.to_vec());
                }
                if let Some(timeout) = timeout {
                    request = request.timeout(timeout);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|error| Error::from_wreq(&error))?;
                let status = response.status();
                let answer_headers = response.headers().clone();
                if let Some(jar) = &client.jar {
                    let mut set = answer_headers.get_all(SET_COOKIE).iter();
                    jar.set_cookies(&mut set, &url);
                }
                if let Redirects::Follow(allowed) = client.redirects
                    && status.is_redirection()
                    && hops < MAX_REDIRECTS
                    && let Some(next) = answer_headers
                        .get(LOCATION)
                        .and_then(|location| location.to_str().ok())
                        .and_then(|location| url.join(location).ok())
                    && allowed(&next)
                {
                    hops += 1;
                    // As a browser: these become a GET without the body.
                    if matches!(
                        status,
                        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
                    ) {
                        method = Method::GET;
                        body = None;
                        headers.remove(CONTENT_TYPE);
                    }
                    url = next;
                    continue;
                }
                let mut bytes = Zeroizing::new(Vec::new());
                let mut stream = response.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|error| Error::from_wreq(&error))?;
                    let room = (MAX_BODY + 1).saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
                    if bytes.len() > MAX_BODY {
                        break;
                    }
                }
                return Ok(Response {
                    status,
                    headers: answer_headers,
                    url,
                    body: bytes,
                    read: 0,
                });
            }
        })
    }
}

/// Connects to a well-known HTTPS site through this client and returns the
/// status: proof that its TLS and root certificates work on this system.
pub fn check_tls() -> Result<u16> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    client
        .get("https://github.com/")
        .send()
        .map(|response| response.status().as_u16())
}

/// A script's request headers as Chrome sends them, in its order; the
/// caller's own (origin, referer, content type, cookies) take their places.
fn fetch_headers(own: &HeaderMap, site: FetchSite) -> (HeaderMap, wreq::header::OrigHeaderMap) {
    let platform = if cfg!(target_os = "windows") {
        "\"Windows\""
    } else {
        "\"Linux\""
    };
    let site = match site {
        FetchSite::SameOrigin => "same-origin",
        FetchSite::SameSite => "same-site",
        FetchSite::CrossSite => "cross-site",
    };
    let fixed = |value: &'static str| Some(HeaderValue::from_static(value));
    let own_value = |name: &HeaderName| own.get(name).cloned();
    let ordered: [(HeaderName, Option<HeaderValue>); 15] = [
        (
            HeaderName::from_static("sec-ch-ua-platform"),
            fixed(platform),
        ),
        (USER_AGENT, fixed(crate::client_platform::USER_AGENT)),
        (HeaderName::from_static("sec-ch-ua"), fixed(SEC_CH_UA)),
        (CONTENT_TYPE, own_value(&CONTENT_TYPE)),
        (HeaderName::from_static("sec-ch-ua-mobile"), fixed("?0")),
        (ACCEPT, fixed("*/*")),
        (ORIGIN, own_value(&ORIGIN)),
        (HeaderName::from_static("sec-fetch-site"), fixed(site)),
        (HeaderName::from_static("sec-fetch-mode"), fixed("cors")),
        (HeaderName::from_static("sec-fetch-dest"), fixed("empty")),
        (REFERER, own_value(&REFERER)),
        (ACCEPT_ENCODING, fixed("gzip, deflate, br, zstd")),
        (ACCEPT_LANGUAGE, fixed(accept_language())),
        (COOKIE, own_value(&COOKIE)),
        (HeaderName::from_static("priority"), fixed("u=1, i")),
    ];
    let mut headers = HeaderMap::new();
    let mut order = wreq::header::OrigHeaderMap::new();
    // Chrome puts a body's length first; it is added with the body.
    order.insert(reqwest::header::CONTENT_LENGTH);
    for (name, value) in ordered {
        if let Some(value) = value {
            order.insert(name.clone());
            headers.insert(name, value);
        }
    }
    // Anything else the caller set comes after, as it would.
    for (name, value) in own {
        if !headers.contains_key(name) {
            order.insert(name.clone());
            headers.append(name.clone(), value.clone());
        }
    }
    (headers, order)
}

/// A whole answer, read before the call returns. Its bytes are wiped when
/// it is dropped: VK's answers carry tokens.
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    url: Url,
    body: Zeroizing<Vec<u8>>,
    read: usize,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    /// The address that answered, after any redirects followed.
    pub fn url(&self) -> &Url {
        &self.url
    }
}

impl Read for Response {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let rest = &self.body[self.read..];
        let count = rest.len().min(buffer.len());
        buffer[..count].copy_from_slice(&rest[..count]);
        self.read += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_form_encode_like_a_browser_form() {
        let client = Client::builder().build().unwrap();
        let request = client
            .get("https://vk.ru/feed?x=1")
            .query(&[("a", "b c"), ("d", "é")])
            .form(&[("act", "reload_audio"), ("ids", "1_2,3_4")]);
        assert_eq!(
            request.url.as_ref().unwrap().query(),
            Some("x=1&a=b+c&d=%C3%A9")
        );
        assert_eq!(
            request.body.as_deref().map(Vec::as_slice),
            Some(&b"act=reload_audio&ids=1_2%2C3_4"[..])
        );
        assert_eq!(
            request.headers.get(CONTENT_TYPE).unwrap(),
            "application/x-www-form-urlencoded"
        );
        assert!(client.get("not an address").send().is_err());
        assert!(
            client
                .get("https://vk.ru/")
                .header("bad header", "x")
                .send()
                .is_err()
        );
    }

    #[test]
    fn the_handshake_and_the_user_agent_name_one_chrome_version() {
        assert!(matches!(PROFILE, wreq_util::Profile::Chrome149));
        let version = format!("Chrome/{CHROME_VERSION}.");
        assert!(
            crate::client_platform::USER_AGENT.contains(&version),
            "{} vs {version}",
            crate::client_platform::USER_AGENT
        );
    }

    #[test]
    fn a_scripts_request_carries_chromes_fetch_headers_in_its_order() {
        let mut own = HeaderMap::new();
        own.insert(REFERER, HeaderValue::from_static("https://vk.ru/"));
        own.insert(ORIGIN, HeaderValue::from_static("https://vk.ru"));
        own.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        own.insert(COOKIE, HeaderValue::from_static("a=b"));
        let (headers, order) = fetch_headers(&own, FetchSite::SameSite);
        let names: Vec<&str> = order.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "content-length",
                "sec-ch-ua-platform",
                "user-agent",
                "sec-ch-ua",
                "content-type",
                "sec-ch-ua-mobile",
                "accept",
                "origin",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
                "referer",
                "accept-encoding",
                "accept-language",
                "cookie",
                "priority"
            ]
        );
        assert_eq!(headers["sec-fetch-site"], "same-site");
        assert_eq!(headers["sec-fetch-mode"], "cors");
        assert_eq!(headers[COOKIE], "a=b");
        assert!(!headers.contains_key("upgrade-insecure-requests"));
        assert!(!headers.contains_key("sec-fetch-user"));
        // A GET without a body or origin leaves those out.
        let (headers, _) = fetch_headers(&HeaderMap::new(), FetchSite::SameOrigin);
        assert!(!headers.contains_key(CONTENT_TYPE) && !headers.contains_key(ORIGIN));
        assert!(SEC_CH_UA.contains(CHROME_VERSION));
    }

    #[test]
    fn a_response_reads_once_to_its_end() {
        let mut response = Response {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            url: Url::parse("https://vk.ru/").unwrap(),
            body: Zeroizing::new(b"hello".to_vec()),
            read: 0,
        };
        let mut text = String::new();
        response.read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(response.read(&mut [0; 4]).unwrap(), 0);
    }

    /// The handshake VK sees is still Chrome's, after a `wreq`/`btls-sys`
    /// update (`docs/release.md`). Asks an outside echo service, so it runs
    /// only on request: `cargo test --locked fingerprint_is_chromes -- --ignored`.
    #[test]
    #[ignore = "asks tls.peet.ws over the network"]
    fn fingerprint_is_chromes() {
        let client = Client::builder().build().unwrap();
        let mut body = Vec::new();
        client
            .get("https://tls.peet.ws/api/all")
            .send()
            .unwrap()
            .read_to_end(&mut body)
            .unwrap();
        let echo: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(echo["http_version"], "h2");
        assert_eq!(echo["tls"]["ja4"], "t13d1516h2_8daaf6152771_d8a2da3f94cd");
        assert_eq!(
            echo["http2"]["akamai_fingerprint_hash"],
            "52d84b11737d980aef856699f885ca86"
        );
    }
}
