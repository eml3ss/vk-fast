//! Fresh VK website QR sessions, separate from VK Fast's own VK ID OAuth.
//! Protocol: public qrAuth-web entry.b9595b4b.js and
//! static.vk.ru/vkid/1.1.1427/185.js, checked 2026-10-29.

use crate::{
    i18n::Language,
    model::{Profile, profile_avatar_from_object},
    web_session::SessionCookie,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Url,
    blocking::{Client, Response},
    cookie::{CookieStore, Jar},
    header::{ORIGIN, REFERER},
    redirect::Policy,
};
use serde_json::Value;
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

// Actual VK website identity; never replaces VK Fast's own OAuth app 54863250.
const WEBSITE_APP: &str = "7913379";
const API: &str = "https://api.vk.ru/method/";
const RETURN_URI: &str = "https://vk.ru/feed";
const MAX_BODY: u64 = 8 * 1024 * 1024;
const MAX_LIFETIME: u64 = 300;
const USER_AGENT: &str = crate::client_platform::USER_AGENT;

#[derive(Clone, Default)]
pub struct Control {
    canceled: Arc<AtomicBool>,
    code: Arc<Mutex<Option<Zeroizing<String>>>>,
}
impl Control {
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        if let Ok(mut code) = self.code.lock() {
            *code = None;
        }
    }
    /// A security code displayed by VK on the phone, never an account password.
    pub fn submit_code(&self, code: String) {
        let code = Zeroizing::new(code);
        if let Some(code) = normalize_code(&code)
            && let Ok(mut pending) = self.code.lock()
            && !self.canceled.load(Ordering::Acquire)
        {
            *pending = Some(code);
        }
    }
    fn check(&self) -> Result<(), Error> {
        if self.canceled.load(Ordering::Acquire) {
            Err(Error::Canceled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Creating,
    Waiting,
    Scanned,
    Confirming,
    Finishing,
}
// No Debug: the URL can authorize the newly created website session.
pub enum Event {
    Ready { url: String, expires_in: u64 },
    Status(Stage),
    CodeResult(Result<(), Error>),
}
pub struct SignedIn {
    pub profile: Profile,
    pub cookies: Vec<SessionCookie>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Canceled,
    Expired,
    Declined,
    Network,
    Timeout,
    Http(u16),
    Api(i64),
    InvalidResponse,
    PhoneVerification,
    Finish,
    IncorrectCode,
}
impl Error {
    pub fn localized(&self, language: Language) -> String {
        let text = match self {
            Self::Canceled => language.text("Вход по QR отменён.", "QR sign-in canceled."),
            Self::Expired => language.text("QR-код истёк. Создайте новый код.", "The QR code expired. Generate a new code."),
            Self::Declined => language.text("Вход отклонён на телефоне.", "Sign-in was declined on the phone."),
            Self::Network => language.text("Не удалось связаться с VK для входа по QR.", "Could not connect to VK for QR sign-in."),
            Self::Timeout => language.text("VK не ответил вовремя. Повторите вход по QR.", "VK timed out. Try QR sign-in again."),
            Self::Http(code) => return format!("{} {code}.", language.text("VK вернул HTTP", "VK returned HTTP")),
            Self::Api(code) => return format!("{} (VK {code}).", language.text("VK отклонил запрос входа по QR", "VK rejected the QR sign-in request")),
            Self::InvalidResponse => language.text("Формат входа по QR у VK изменился или не распознан.", "VK's QR sign-in format changed or was not recognized."),
            Self::PhoneVerification => language.text("VK требует дополнительную проверку аккаунта. Завершите её в официальном приложении VK и повторите вход.", "VK requires additional account verification. Complete it in the official VK app and try again."),
            Self::Finish => language.text("Вход подтверждён, но VK не выдал вебсессию для музыки. Повторите вход по QR.", "Sign-in was approved, but VK did not issue a website session for music. Try QR sign-in again."),
            Self::IncorrectCode => language.text("Код не подошёл. Проверьте код на телефоне и попробуйте ещё раз.", "The code was incorrect. Check the code on your phone and try again."),
        };
        text.into()
    }
}

fn normalize_code(raw: &str) -> Option<Zeroizing<String>> {
    if raw.len() > 64 {
        return None;
    }
    let code = Zeroizing::new(
        raw.chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>(),
    );
    (code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())).then_some(code)
}

// Website contract: validation 0 accepts the code, 1 permits a retry,
// 2 expires the session. These are distinct from checkAuthCode's statuses.
fn validation_result(response: &Value) -> Result<(), Error> {
    match response.get("status").and_then(Value::as_u64) {
        Some(0) => Ok(()),
        Some(1) => Err(Error::IncorrectCode),
        Some(2) => Err(Error::Expired),
        _ => Err(Error::InvalidResponse),
    }
}

fn validation_feedback(
    events: &SyncSender<Event>,
    response: Result<Value, Error>,
) -> Result<(), Error> {
    let result = response.and_then(|response| validation_result(&response));
    if matches!(result, Err(Error::Expired | Error::Canceled)) {
        return result;
    }
    // A bad code or interrupted validation request leaves the same QR alive.
    // Polling still decides whether mobile approval actually completed.
    match events.try_send(Event::CodeResult(result)) {
        Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err(Error::Canceled),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
fn lifetime(expires: u64, current: u64) -> Result<u64, Error> {
    let seconds = if expires > current {
        expires - current
    } else if expires <= MAX_LIFETIME {
        expires
    } else {
        0
    };
    if seconds == 0 {
        Err(Error::Expired)
    } else {
        Ok(seconds.min(MAX_LIFETIME))
    }
}
fn allowed_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            matches!(
                host,
                "vk.ru"
                    | "id.vk.ru"
                    | "api.vk.ru"
                    | "login.vk.ru"
                    | "vk.com"
                    | "id.vk.com"
                    | "api.vk.com"
                    | "login.vk.com"
            )
        })
}
fn valid_qr_url(value: &str) -> bool {
    value.len() <= 16_384
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port_or_known_default() == Some(443)
                && url.host_str().is_some_and(|host| {
                    matches!(
                        host,
                        "id.vk.ru" | "qr.vk.ru" | "vk.ru" | "id.vk.com" | "qr.vk.com" | "vk.com"
                    )
                })
        })
}
fn network(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Network
    }
}
fn read(response: Response) -> Result<Zeroizing<String>, Error> {
    if !response.status().is_success() {
        return Err(Error::Http(response.status().as_u16()));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    if bytes.len() as u64 > MAX_BODY {
        return Err(Error::InvalidResponse);
    }
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| Error::InvalidResponse)
}
fn decode_api(text: &str) -> Result<Value, Error> {
    let envelope: Value = serde_json::from_str(text).map_err(|_| Error::InvalidResponse)?;
    if let Some(code) = envelope
        .get("error")
        .and_then(|error| error.get("error_code"))
        .and_then(Value::as_i64)
    {
        return Err(Error::Api(code));
    }
    envelope
        .get("response")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or(Error::InvalidResponse)
}
fn call(
    client: &Client,
    method: &str,
    version: &str,
    parameters: &[(&str, &str)],
) -> Result<Value, Error> {
    let response = client
        .post(format!("{API}{method}"))
        .query(&[("v", version), ("client_id", WEBSITE_APP)])
        .header(ORIGIN, "https://id.vk.ru")
        .header(REFERER, "https://id.vk.ru/")
        .form(parameters)
        .send()
        .map_err(network)?;
    decode_api(&read(response)?)
}
fn string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 16_384)
        .ok_or(Error::InvalidResponse)
}
fn parse_init(html: &str) -> Result<Value, Error> {
    let rest = html
        .split_once("window.init")
        .ok_or(Error::InvalidResponse)?
        .1;
    let rest = rest
        .trim_start()
        .strip_prefix('=')
        .ok_or(Error::InvalidResponse)?
        .trim_start();
    let value = serde_json::Deserializer::from_str(rest)
        .into_iter::<Value>()
        .next()
        .ok_or(Error::InvalidResponse)?
        .map_err(|_| Error::InvalidResponse)?;
    let app = value
        .pointer("/auth/host_app_id")
        .and_then(|app| app.as_u64().or_else(|| app.as_str()?.parse().ok()));
    if app != WEBSITE_APP.parse().ok() {
        return Err(Error::InvalidResponse);
    }
    Ok(value)
}
fn bootstrap(client: &Client) -> Result<Value, Error> {
    let response = client
        .get("https://id.vk.ru/auth")
        .query(&[
            ("app_id", WEBSITE_APP),
            ("redirect_uri", RETURN_URI),
            ("response_type", "silent_token"),
        ])
        .header(REFERER, "https://vk.ru/")
        .send()
        .map_err(network)?;
    parse_init(&read(response)?)
}
fn send_ready(
    events: &SyncSender<Event>,
    mut event: Event,
    control: &Control,
    deadline: Instant,
) -> Result<(), Error> {
    loop {
        control.check()?;
        match events.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(Error::Canceled),
            Err(TrySendError::Full(pending)) => event = pending,
        }
        if Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn wait(control: &Control, deadline: Instant) -> Result<(), Error> {
    let next = (Instant::now() + Duration::from_secs(3)).min(deadline);
    while Instant::now() < next {
        control.check()?;
        thread::sleep(Duration::from_millis(100));
    }
    if Instant::now() >= deadline {
        Err(Error::Expired)
    } else {
        control.check()
    }
}

/// Creates a fresh anonymous QR session. Only the user can scan/approve it.
pub fn start(control: Control, events: SyncSender<Event>) -> Result<SignedIn, Error> {
    control.check()?;
    let _ = events.try_send(Event::Status(Stage::Creating));
    let jar = Arc::new(Jar::default());
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .cookie_provider(jar.clone())
        .user_agent(USER_AGENT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !allowed_url(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| Error::Network)?;
    let init = bootstrap(&client)?;
    control.check()?;
    let anonymous = call(
        &client,
        "auth.getAnonymToken",
        "5.276",
        &[("client_id", WEBSITE_APP)],
    )?;
    let anonymous = Zeroizing::new(string(&anonymous, "/token")?.to_owned());
    let data = call(
        &client,
        "auth.getQrAuthData",
        "5.276",
        &[
            ("app_id", WEBSITE_APP),
            ("origin", "https://vk.ru"),
            ("lang", "0"),
        ],
    )?;
    let verification = Zeroizing::new(string(&data, "/code_auth_verification_hash")?.to_owned());
    control.check()?;
    let qr = call(
        &client,
        "auth.getAuthCode",
        "5.126",
        &[
            ("device_name", crate::client_platform::DEVICE_NAME),
            ("auth_code_flow", "0"),
            ("verification_hash", &verification),
            ("force_regenerate", "1"),
            ("anonymous_token", &anonymous),
        ],
    )?;
    let url = string(&qr, "/auth_url")?.to_owned();
    if !valid_qr_url(&url) {
        return Err(Error::InvalidResponse);
    }
    let hash = Zeroizing::new(string(&qr, "/auth_hash")?.to_owned());
    let expires = qr
        .get("expires_in")
        .and_then(Value::as_u64)
        .ok_or(Error::InvalidResponse)?;
    let expires_in = lifetime(expires, now())?;
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    send_ready(
        &events,
        Event::Ready { url, expires_in },
        &control,
        deadline,
    )?;
    let mut last_stage = None;
    loop {
        control.check()?;
        if Instant::now() >= deadline {
            return Err(Error::Expired);
        }
        let code = control.code.lock().ok().and_then(|mut code| code.take());
        if let Some(code) = code {
            let result = call(
                &client,
                "auth.validateAuthCode",
                "5.126",
                &[
                    ("access_token", &anonymous),
                    ("auth_hash", &hash),
                    ("validation_code", &code),
                ],
            );
            control.check()?;
            validation_feedback(&events, result)?;
        }
        let response = call(
            &client,
            "auth.checkAuthCode",
            "5.126",
            &[
                ("auth_hash", &hash),
                ("web_auth", "1"),
                ("anonymous_token", &anonymous),
            ],
        )?;
        control.check()?;
        let stage = match response.get("status").and_then(Value::as_i64) {
            Some(0) => Stage::Waiting,
            Some(1) => Stage::Scanned,
            Some(5) => Stage::Confirming,
            Some(2) => {
                let _ = events.try_send(Event::Status(Stage::Finishing));
                return finish(&client, &jar, &init, &response, &control);
            }
            Some(3) => return Err(Error::Declined),
            Some(4) => return Err(Error::Expired),
            _ => return Err(Error::InvalidResponse),
        };
        if last_stage != Some(stage) {
            let _ = events.try_send(Event::Status(stage));
            last_stage = Some(stage);
        }
        wait(&control, deadline)?;
    }
}

fn finish_parameters(init: &Value, token: &str) -> Result<Vec<(&'static str, String)>, Error> {
    let response_type = string(init, "/data/oauth/response_type")?;
    let mut parameters = vec![
        ("token", token.to_owned()),
        ("uuid", string(init, "/data/uuid")?.to_owned()),
        ("app_id", WEBSITE_APP.into()),
        ("version", "1".into()),
        ("sid", String::new()),
        ("oauth_version", String::new()),
        (
            "oauth_force_hash",
            if init
                .pointer("/data/oauth/force_hash")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "1"
            } else {
                "0"
            }
            .into(),
        ),
        ("is_registration", "0".into()),
        ("oauth_response_type", response_type.to_owned()),
        (
            "is_oauth_migrated_flow",
            if init
                .pointer("/data/app/authorize_using_oauth_process")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "1"
            } else {
                "0"
            }
            .into(),
        ),
        ("to", STANDARD.encode(RETURN_URI)),
    ];
    if let Some(hash) = init
        .pointer("/data/oauth/hashes/vkid_oauth_hash")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 8192)
    {
        parameters.push(("vkid_oauth_hash", hash.to_owned()));
    }
    Ok(parameters)
}
fn finish(
    client: &Client,
    jar: &Jar,
    init: &Value,
    approved: &Value,
    control: &Control,
) -> Result<SignedIn, Error> {
    if approved.get("need_password").and_then(Value::as_bool) == Some(true) {
        return Err(Error::PhoneVerification);
    }
    let token = Zeroizing::new(
        string(approved, "/super_app_token")
            .map_err(|_| Error::Finish)?
            .to_owned(),
    );
    let mut parameters = finish_parameters(init, &token)?;
    let response = client
        .post("https://login.vk.ru/?act=connect_code_auth")
        .header(ORIGIN, "https://id.vk.ru")
        .header(REFERER, "https://id.vk.ru/")
        .form(&parameters)
        .send()
        .map_err(network);
    for (key, value) in &mut parameters {
        if *key == "token" {
            value.zeroize();
        }
    }
    let result: Value =
        serde_json::from_str(&read(response?)?).map_err(|_| Error::InvalidResponse)?;
    if result.get("type").and_then(Value::as_str) != Some("okay") {
        return Err(Error::Finish);
    }
    let data = result
        .get("data")
        .filter(|data| data.is_object())
        .ok_or(Error::Finish)?;
    if data.get("response_type").and_then(Value::as_str) != Some("auth_token") {
        return Err(Error::PhoneVerification);
    }
    let auth = data
        .get("auth_info")
        .filter(|auth| auth.is_object())
        .ok_or(Error::Finish)?;
    let user = auth
        .get("user")
        .or_else(|| data.get("user"))
        .ok_or(Error::Finish)?;
    let id = user
        .get("id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or(Error::Finish)?;
    let name = |key: &str| {
        user.get(key)
            .and_then(Value::as_str)
            .filter(|name| name.len() <= 1024)
            .unwrap_or_default()
            .to_owned()
    };
    let profile = Profile {
        id,
        first_name: name("first_name"),
        last_name: name("last_name"),
        avatar_url: profile_avatar_from_object(user),
    };
    control.check()?;
    // The actual website redirects its auth_info JSON payload to its registered
    // return URI. Only real Set-Cookie headers in this isolated jar are retained.
    let payload = Zeroizing::new(serde_json::to_string(auth).map_err(|_| Error::Finish)?);
    let _ = read(
        client
            .get(RETURN_URI)
            .query(&[("payload", payload.as_str())])
            .header(REFERER, "https://id.vk.ru/")
            .send()
            .map_err(network)?,
    )?;
    control.check()?;
    let cookies = website_cookies(jar)?;
    if !cookies.iter().any(|cookie| {
        matches!(cookie.name.as_str(), "remixsid" | "remixsid6") && !cookie.value.is_empty()
    }) {
        return Err(Error::Finish);
    }
    Ok(SignedIn { profile, cookies })
}
fn website_cookies(jar: &Jar) -> Result<Vec<SessionCookie>, Error> {
    let mut cookies = Vec::new();
    for host in ["vk.ru", "vk.com"] {
        let url = Url::parse(&format!("https://{host}/")).map_err(|_| Error::Finish)?;
        let Some(header) = jar.cookies(&url) else {
            continue;
        };
        let values = Zeroizing::new(header.to_str().map_err(|_| Error::Finish)?.to_owned());
        for item in values.split(';') {
            let Some((name, value)) = item.trim().split_once('=') else {
                return Err(Error::Finish);
            };
            let cookie = SessionCookie {
                domain: host.into(),
                path: "/".into(),
                name: name.into(),
                value: value.into(),
                secure: true,
                expires_unix: None,
            };
            if !cookie.is_valid() || cookies.len() >= 256 {
                return Err(Error::Finish);
            }
            cookies.push(cookie);
        }
    }
    Ok(cookies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn confirmation_codes_preserve_zeroes_and_accept_phone_spacing() {
        let control = Control::default();
        control.submit_code(" 012\u{202f}345 ".into());
        assert_eq!(
            control.code.lock().unwrap().take().unwrap().as_str(),
            "012345"
        );
        assert!(control.code.lock().unwrap().is_none());
        for invalid in ["01234", "0123456", "abc012345", "０１２３４５"] {
            control.submit_code(invalid.into());
            assert!(control.code.lock().unwrap().is_none());
        }
        control.submit_code("012345".into());
        control.cancel();
        assert!(control.code.lock().unwrap().is_none());
        control.submit_code("012345".into());
        assert!(control.code.lock().unwrap().is_none());
    }

    #[test]
    fn incorrect_code_allows_a_second_attempt_without_expiring_the_session() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(4);
        assert_eq!(
            validation_feedback(&sender, Ok(json!({"status":1}))),
            Ok(())
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(Event::CodeResult(Err(Error::IncorrectCode)))
        ));
        assert_eq!(
            validation_feedback(&sender, Ok(json!({"status":0}))),
            Ok(())
        );
        assert!(matches!(receiver.try_recv(), Ok(Event::CodeResult(Ok(())))));
        assert_eq!(validation_feedback(&sender, Err(Error::Timeout)), Ok(()));
        assert!(matches!(
            receiver.try_recv(),
            Ok(Event::CodeResult(Err(Error::Timeout)))
        ));
        assert_eq!(
            validation_feedback(&sender, Ok(json!({"status":2}))),
            Err(Error::Expired)
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn interprets_absolute_server_expiry_and_bounds_lifetime() {
        assert_eq!(lifetime(1_700_000_180, 1_700_000_000).unwrap(), 180);
        assert_eq!(lifetime(180, 1_700_000_000).unwrap(), 180);
        assert_eq!(lifetime(1_700_010_000, 1_700_000_000).unwrap(), 300);
        assert_eq!(lifetime(1_699_999_000, 1_700_000_000), Err(Error::Expired));
    }
    #[test]
    fn bootstrap_requires_website_identity_and_errors_do_not_echo_tokens() {
        let init =
            parse_init(r#"<script>window.init = {"auth":{"host_app_id":7913379}};</script>"#)
                .unwrap();
        assert_eq!(init["auth"]["host_app_id"], 7913379);
        assert!(parse_init(r#"window.init={"auth":{"host_app_id":54863250}};"#).is_err());
        assert_eq!(
            decode_api(r#"{"error":{"error_code":15,"error_msg":"synthetic-secret"}}"#)
                .unwrap_err(),
            Error::Api(15)
        );
        assert!(
            !Error::Api(15)
                .localized(Language::English)
                .contains("synthetic-secret")
        );
    }
    #[test]
    fn qr_urls_and_cookie_exports_stay_on_the_expected_website() {
        assert!(valid_qr_url("https://qr.vk.ru/ca?q=synthetic-code"));
        assert!(!valid_qr_url("https://qr.vk.ru.example.com/ca"));
        assert!(!valid_qr_url("http://qr.vk.ru/ca"));
        assert!(!valid_qr_url("https://user:secret@qr.vk.ru/ca"));
        let jar = Jar::default();
        jar.add_cookie_str(
            "remixsid=synthetic; Path=/; Secure",
            &Url::parse("https://vk.ru/").unwrap(),
        );
        jar.add_cookie_str(
            "id_only=excluded; Path=/; Secure",
            &Url::parse("https://id.vk.ru/").unwrap(),
        );
        jar.add_cookie_str(
            "foreign=excluded; Path=/; Secure",
            &Url::parse("https://example.com/").unwrap(),
        );
        let cookies = website_cookies(&jar).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].domain, "vk.ru");
        assert_eq!(cookies[0].name, "remixsid");
    }
    #[test]
    fn finish_uses_fresh_website_metadata_without_foreign_secrets() {
        let init = json!({"data":{"uuid":"synthetic-uuid","oauth":{"response_type":"silent_token","force_hash":false,"hashes":{"vkid_oauth_hash":"synthetic-hash"}},"app":{"authorize_using_oauth_process":false}}});
        let params = finish_parameters(&init, "synthetic-token").unwrap();
        assert!(
            params
                .iter()
                .any(|(key, value)| *key == "app_id" && value == WEBSITE_APP)
        );
        assert!(
            params
                .iter()
                .any(|(key, value)| *key == "to" && value == &STANDARD.encode(RETURN_URI))
        );
        assert!(!params.iter().any(|(key, _)| *key == "client_secret"));
        let control = Control::default();
        control.cancel();
        assert_eq!(control.check(), Err(Error::Canceled));
    }
}
