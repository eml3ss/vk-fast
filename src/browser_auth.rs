//! VK ID PKCE via a static HTTPS return page and the vkfast application URI.
//! Browser opening is delegated to the OS. No browser profiles or CDP.
use crate::{
    auth_return,
    i18n::Language,
    model::{Profile, profile_avatar_from_object},
    session_store::{self, SavedSession},
    vk::{VkClient, VkError},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Url, blocking::Client, redirect::Policy};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

const AUTHORIZE: &str = "https://id.vk.ru/authorize";
const TOKEN: &str = "https://id.vk.ru/oauth2/auth";
const USER_INFO: &str = "https://id.vk.ru/oauth2/user_info";
const WAIT: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub struct Control {
    cancelled: Arc<AtomicBool>,
    stage: Arc<AtomicU8>,
}
impl Control {
    pub fn restoring() -> Self {
        let control = Self::default();
        control.stage.store(4, Ordering::Relaxed);
        control
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    fn check(&self) -> Result<(), LoginError> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(LoginError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn status(&self, lang: Language) -> &'static str {
        match self.stage.load(Ordering::Relaxed) {
            2 => lang.text(
                "Подтвердите доступ в браузере…",
                "Approve access in your browser…",
            ),
            3 => lang.text("Завершаем вход через VK ID…", "Completing VK ID sign-in…"),
            4 => lang.text(
                "Восстанавливаем сохранённый вход…",
                "Restoring your saved sign-in…",
            ),
            _ => lang.text(
                "Открываем браузер по умолчанию…",
                "Opening your default browser…",
            ),
        }
    }
}

#[derive(Debug)]
pub enum LoginError {
    ConfigMissing,
    InvalidConfig,
    Random,
    ReturnBusy,
    RedirectNotRegistered,
    Listener,
    BrowserOpen,
    Cancelled,
    Timeout,
    Denied,
    Protocol,
    Network,
    Http(u16),
    AccountMismatch,
    SessionStorage,
    SessionExpired,
    Verify(VkError),
}
impl LoginError {
    pub fn localized(&self, lang: Language) -> String {
        let t = |ru, en| lang.text(ru, en);
        match self {
            Self::ConfigMissing => t("Вход через VK ID ещё не настроен для этой сборки. Разработчику нужны APP_ID и зарегистрированный адрес возврата.", "VK ID sign-in is not configured for this build. The developer must register an app ID and redirect URI.").into(),
            Self::InvalidConfig => t("Неверная конфигурация VK ID: нужен APP_ID и зарегистрированный HTTPS-адрес страницы возврата.", "Invalid VK ID configuration: register an app ID and an HTTPS return page.").into(),
            Self::Random => t("Не удалось создать защищённые параметры входа.", "Could not generate secure sign-in parameters.").into(),
            Self::ReturnBusy => t("Вход через браузер уже запущен в другом окне VK Fast.", "Browser sign-in is already running in another VK Fast window.").into(),
            Self::RedirectNotRegistered => t("Добавьте https://eml3ss.github.io/vk-fast-auth/callback/ в доверенные Redirect URL приложения VK Fast в кабинете VK ID.", "Add https://eml3ss.github.io/vk-fast-auth/callback/ to the trusted Redirect URLs for VK Fast in the VK ID dashboard.").into(),
            Self::Listener => t("Не удалось открыть канал возврата в приложение. Проверьте обработчик ссылок vkfast.", "Could not open the application return channel. Check the vkfast link handler.").into(),
            Self::BrowserOpen => t("Не удалось открыть системный браузер по умолчанию.", "Could not open the default system browser.").into(),
            Self::Cancelled => t("Вход отменён.", "Sign-in cancelled.").into(),
            Self::Timeout => t("Время ожидания подтверждения истекло (5 минут). Повторите вход.", "Sign-in approval timed out after 5 minutes. Try again.").into(),
            Self::Denied => t("Доступ не был предоставлен в VK ID.", "Access was not granted in VK ID.").into(),
            Self::Protocol => t("VK ID вернул некорректный ответ авторизации. Повторите вход.", "VK ID returned an invalid authorization response. Try again.").into(),
            Self::Network => t("Не удалось связаться с VK ID. Проверьте интернет и повторите вход.", "Could not reach VK ID. Check your connection and try again.").into(),
            Self::Http(code) => format!("VK ID: HTTP {code}"),
            Self::AccountMismatch => t("Полученный профиль не соответствует ответу VK ID.", "The returned profile does not match the VK ID token response.").into(),
            Self::SessionStorage => t("Не удалось прочитать или сохранить вход на этом компьютере.", "Could not read or save sign-in on this computer.").into(),
            Self::SessionExpired => t("Сохранённый вход истёк или был отозван. Войдите через VK ещё раз.", "Saved sign-in expired or was revoked. Sign in with VK again.").into(),
            Self::Verify(error) => error.localized(lang),
        }
    }
}

pub struct SignedIn {
    pub profile: Profile,
    pub client: VkClient,
    pub cache_warning: bool,
}

#[derive(Clone)]
struct Config {
    client_id: String,
    redirect: Url,
}
impl Config {
    fn registered() -> Result<Self, LoginError> {
        let app = std::env::var("VK_FAST_VK_CLIENT_ID")
            .ok()
            .or_else(|| option_env!("VK_FAST_VK_CLIENT_ID").map(str::to_owned));
        let redirect = std::env::var("VK_FAST_VK_REDIRECT_URI")
            .ok()
            .or_else(|| option_env!("VK_FAST_VK_REDIRECT_URI").map(str::to_owned));
        Self::new(
            &app.ok_or(LoginError::ConfigMissing)?,
            &redirect.ok_or(LoginError::ConfigMissing)?,
        )
    }
    fn new(client_id: &str, redirect: &str) -> Result<Self, LoginError> {
        let redirect = Url::parse(redirect).map_err(|_| LoginError::InvalidConfig)?;
        let legacy_loopback = redirect.scheme() == "http"
            && matches!(
                redirect.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            );
        let https = redirect.scheme() == "https"
            && redirect.host_str().is_some_and(|host| {
                host.contains('.') && host.parse::<std::net::IpAddr>().is_err()
            });
        if client_id.parse::<u64>().ok().is_none_or(|id| id == 0)
            || !client_id.bytes().all(|byte| byte.is_ascii_digit())
            || !(https || legacy_loopback)
            || redirect
                .port_or_known_default()
                .is_none_or(|port| port == 0)
            || !redirect.username().is_empty()
            || redirect.password().is_some()
            || redirect.query().is_some()
            || redirect.fragment().is_some()
        {
            return Err(LoginError::InvalidConfig);
        }
        Ok(Self {
            client_id: client_id.into(),
            redirect,
        })
    }
    fn supports_native_return(&self) -> bool {
        self.redirect.scheme() == "https"
    }
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn nonce() -> Result<Zeroizing<String>, LoginError> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|_| LoginError::Random)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}
fn authorize_url(config: &Config, state: &str, verifier: &str) -> Url {
    let mut url = Url::parse(AUTHORIZE).unwrap();
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", config.client_id.as_str()),
        ("app_id", config.client_id.as_str()),
        ("redirect_uri", config.redirect.as_str()),
        ("state", state),
        ("code_challenge", challenge(verifier).as_str()),
        ("code_challenge_method", "s256"),
    ]);
    url
}

// Deliberately no Debug implementation for authorization codes or tokens.
struct Approval {
    code: Zeroizing<String>,
    device_id: Zeroizing<String>,
}
enum Callback {
    Ignore,
    Denied,
    Approved(Approval),
}
fn parse_return(value: &str, expected_state: &str) -> Callback {
    if value.len() > 16_384 {
        return Callback::Ignore;
    }
    let Ok(url) = Url::parse(value) else {
        return Callback::Ignore;
    };
    if url.scheme() != "vkfast"
        || url.host_str() != Some("oauth")
        || url.path() != "/callback"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return Callback::Ignore;
    }
    let mut params = HashMap::new();
    for (key, value) in url.query_pairs() {
        if params
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Callback::Ignore;
        }
    }
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return Callback::Ignore;
    }
    if params.contains_key("access_token") || params.contains_key("refresh_token") {
        return Callback::Ignore;
    }
    if params.contains_key("error") {
        return Callback::Denied;
    }
    let (Some(code), Some(device_id)) = (params.remove("code"), params.remove("device_id")) else {
        return Callback::Ignore;
    };
    if [&code, &device_id].iter().any(|value| {
        value.is_empty()
            || value.len() > 8192
            || !value.is_ascii()
            || value.chars().any(char::is_control)
    }) {
        return Callback::Ignore;
    }
    Callback::Approved(Approval {
        code: Zeroizing::new(code),
        device_id: Zeroizing::new(device_id),
    })
}

fn wait_return(
    listener: &auth_return::ReturnListener,
    state: &str,
    control: &Control,
    opening: Option<&mpsc::Receiver<bool>>,
) -> Result<Approval, LoginError> {
    let until = Instant::now() + WAIT;
    loop {
        control.check()?;
        if opening.is_some_and(|result| matches!(result.try_recv(), Ok(false))) {
            return Err(LoginError::BrowserOpen);
        }
        if Instant::now() >= until {
            return Err(LoginError::Timeout);
        }
        match listener.try_receive() {
            Ok(Some(value)) => match parse_return(&value, state) {
                Callback::Ignore => (),
                Callback::Denied => return Err(LoginError::Denied),
                Callback::Approved(approval) => return Ok(approval),
            },
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => (),
            Err(_) => return Err(LoginError::Listener),
        }
    }
}

fn redirect_rejected(html: &str) -> bool {
    let Some((_, tail)) = html.split_once("window.init") else {
        return false;
    };
    let Some(json) = tail.trim_start().strip_prefix('=') else {
        return false;
    };
    let Some(Ok(value)) = serde_json::Deserializer::from_str(json.trim_start())
        .into_iter::<Value>()
        .next()
    else {
        return false;
    };
    let error = &value["error"];
    error["code"].as_str() == Some("invalid_request")
        && error["message"]
            .as_str()
            .is_some_and(|message| message.contains("redirect_uri"))
}

fn check_registration(url: &Url) -> Result<(), LoginError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(crate::client_platform::USER_AGENT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() < 5
                && attempt.url().scheme() == "https"
                && matches!(attempt.url().host_str(), Some("id.vk.ru" | "id.vk.com"))
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| LoginError::Network)?;
    let response = client
        .get(url.clone())
        .send()
        .map_err(|_| LoginError::Network)?;
    if !response.status().is_success() {
        return Err(LoginError::Http(response.status().as_u16()));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    response
        .take(2_097_153)
        .read_to_end(&mut bytes)
        .map_err(|_| LoginError::Network)?;
    if bytes.len() > 2_097_152 {
        return Err(LoginError::Protocol);
    }
    if redirect_rejected(std::str::from_utf8(&bytes).map_err(|_| LoginError::Protocol)?) {
        return Err(LoginError::RedirectNotRegistered);
    }
    Ok(())
}

fn json_response(response: reqwest::blocking::Response) -> Result<Value, LoginError> {
    if !response.status().is_success() {
        return Err(LoginError::Http(response.status().as_u16()));
    }
    let mut body = Zeroizing::new(Vec::new());
    response
        .take(65537)
        .read_to_end(&mut body)
        .map_err(|_| LoginError::Network)?;
    if body.len() > 65536 {
        return Err(LoginError::Protocol);
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| LoginError::Protocol)?;
    if let Some(error) = value.get("error") {
        if matches!(
            error.as_str(),
            Some("invalid_token" | "invalid_grant" | "access_denied")
        ) || error["error_code"].as_i64() == Some(5)
        {
            return Err(LoginError::SessionExpired);
        }
        return Err(LoginError::Protocol);
    }
    Ok(value)
}
fn user_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|id| *id > 0)
}
fn token_result(value: &Value, state: &str) -> Result<(Zeroizing<String>, i64), LoginError> {
    if value["state"].as_str() != Some(state) {
        return Err(LoginError::Protocol);
    }
    let token = value["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or(LoginError::Protocol)?;
    let user = user_id(&value["user_id"]).ok_or(LoginError::Protocol)?;
    Ok((Zeroizing::new(token.to_owned()), user))
}
fn vk_profile(value: &Value, expected_id: i64) -> Result<Profile, LoginError> {
    let user = &value["user"];
    let id = user_id(&user["user_id"]).ok_or(LoginError::Protocol)?;
    if id != expected_id {
        return Err(LoginError::AccountMismatch);
    }
    Ok(Profile {
        id,
        first_name: user["first_name"].as_str().unwrap_or("VK").to_owned(),
        last_name: user["last_name"].as_str().unwrap_or_default().to_owned(),
        avatar_url: profile_avatar_from_object(user),
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}
fn expiry(value: &Value) -> u64 {
    value["expires_in"]
        .as_u64()
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| now().checked_add(seconds))
        .unwrap_or(0)
}
fn http_client() -> Result<Client, LoginError> {
    Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| LoginError::Network)
}
fn fetch_profile(
    http: &Client,
    config: &Config,
    token: &str,
    expected_id: i64,
) -> Result<Profile, LoginError> {
    let response = http
        .post(USER_INFO)
        .query(&[("client_id", config.client_id.as_str())])
        .form(&[("access_token", token)])
        .send()
        .map_err(|_| LoginError::Network)?;
    vk_profile(&json_response(response)?, expected_id)
}
fn expired(error: &LoginError) -> bool {
    matches!(error, LoginError::SessionExpired | LoginError::Http(401))
}
fn refresh_saved(
    http: &Client,
    config: &Config,
    saved: &mut SavedSession,
    control: &Control,
) -> Result<bool, LoginError> {
    if saved.refresh_token.is_empty() {
        return Err(LoginError::SessionExpired);
    }
    let state = nonce()?;
    let response = http
        .post(TOKEN)
        .query(&[
            ("grant_type", "refresh_token"),
            ("redirect_uri", config.redirect.as_str()),
            ("client_id", config.client_id.as_str()),
            ("device_id", saved.device_id.as_str()),
            ("state", state.as_str()),
        ])
        .form(&[("refresh_token", saved.refresh_token.as_str())])
        .send()
        .map_err(|_| LoginError::Network)?;
    let value = json_response(response)?;
    let (mut token, id) = token_result(&value, &state)?;
    if id != saved.profile.id {
        return Err(LoginError::AccountMismatch);
    }
    control.check()?;
    saved.access_token.zeroize();
    saved.access_token = std::mem::take(&mut *token);
    if let Some(refresh) = value["refresh_token"]
        .as_str()
        .filter(|token| !token.is_empty())
    {
        saved.refresh_token.zeroize();
        saved.refresh_token = refresh.to_owned();
    }
    saved.expires_at = expiry(&value);
    // Persist rotating refresh credentials before a later profile request can fail.
    Ok(session_store::save(saved).is_err())
}

pub fn restore(control: Control) -> Result<Option<SignedIn>, LoginError> {
    control.check()?;
    let Some(mut saved) = session_store::load().map_err(|_| LoginError::SessionStorage)? else {
        return Ok(None);
    };
    let registered = Config::registered()?;
    if saved.client_id != registered.client_id {
        return Ok(None);
    }
    // Existing tokens keep the redirect they were issued for. Refresh never
    // opens a listener, including for legacy localhost sessions.
    let config = Config::new(&saved.client_id, &saved.redirect_uri)?;
    let http = http_client()?;
    let mut cache_warning = false;
    let mut refreshed = false;
    if saved.expires_at != 0
        && saved.expires_at <= now().saturating_add(60)
        && !saved.refresh_token.is_empty()
    {
        match refresh_saved(&http, &config, &mut saved, &control) {
            Ok(warning) => {
                cache_warning = warning;
                refreshed = true;
            }
            Err(error) if expired(&error) => {
                let _ = session_store::clear();
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    }
    control.check()?;
    let profile = match fetch_profile(&http, &config, &saved.access_token, saved.profile.id) {
        Ok(profile) => profile,
        Err(error) if expired(&error) => {
            if refreshed || saved.refresh_token.is_empty() {
                let _ = session_store::clear();
                return Ok(None);
            }
            match refresh_saved(&http, &config, &mut saved, &control) {
                Ok(warning) => cache_warning |= warning,
                Err(error) if expired(&error) => {
                    let _ = session_store::clear();
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
            fetch_profile(&http, &config, &saved.access_token, saved.profile.id)?
        }
        Err(error) => return Err(error),
    };
    control.check()?;
    saved.profile = profile.clone();
    cache_warning |= session_store::save(&saved).is_err();
    let client = VkClient::new(saved.access_token.clone())
        .map_err(LoginError::Verify)?
        .using_app_id(config.client_id);
    Ok(Some(SignedIn {
        profile,
        client,
        cache_warning,
    }))
}

fn return_listener() -> Result<auth_return::ReturnListener, LoginError> {
    auth_return::bind().map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::AlreadyExists | io::ErrorKind::AddrInUse => {
            LoginError::ReturnBusy
        }
        _ => LoginError::Listener,
    })
}

/// Checks the private application return channel, without TCP or browser access.
pub fn check_setup() -> Result<String, LoginError> {
    let config = Config::registered()?;
    if !config.supports_native_return() {
        return Err(LoginError::InvalidConfig);
    }
    let _listener = return_listener()?;
    Ok(format!(
        "VK ID app {} / {}: vkfast application return channel OK (no TCP listener)",
        config.client_id, config.redirect
    ))
}

pub fn check_registration_setup() -> Result<String, LoginError> {
    let config = Config::registered()?;
    if !config.supports_native_return() {
        return Err(LoginError::InvalidConfig);
    }
    let verifier = nonce()?;
    let state = nonce()?;
    check_registration(&authorize_url(&config, &state, &verifier))?;
    Ok("VK принял HTTPS-адрес возврата этой сборки.".into())
}

pub fn login(control: Control) -> Result<SignedIn, LoginError> {
    control.check()?;
    let config = Config::registered()?;
    if !config.supports_native_return() {
        return Err(LoginError::InvalidConfig);
    }
    let verifier = nonce()?;
    let state = nonce()?;
    let authorization = authorize_url(&config, &state, &verifier);
    check_registration(&authorization)?;
    control.check()?;
    let listener = return_listener()?;
    // open::that delegates to the operating system's default browser handler.
    // Run it separately: a desktop launcher can wait until its browser closes,
    // while OAuth callback reception must already be active.
    let (opened, opening) = mpsc::channel();
    let opening_control = control.clone();
    thread::Builder::new()
        .name("system-browser".into())
        .spawn(move || {
            if opening_control.check().is_err() {
                return;
            }
            let _ = opened.send(open::that(authorization.as_str()).is_ok());
        })
        .map_err(|_| LoginError::BrowserOpen)?;
    control.stage.store(2, Ordering::Relaxed);
    let approval = wait_return(&listener, &state, &control, Some(&opening))?;
    drop(listener);
    control.check()?;
    control.stage.store(3, Ordering::Relaxed);
    let http = http_client()?;
    let response = http
        .post(TOKEN)
        .query(&[
            ("grant_type", "authorization_code"),
            ("redirect_uri", config.redirect.as_str()),
            ("client_id", config.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
            ("state", state.as_str()),
            ("device_id", approval.device_id.as_str()),
        ])
        .form(&[("code", approval.code.as_str())])
        .send()
        .map_err(|_| LoginError::Network)?;
    let value = json_response(response)?;
    let (mut token, expected_id) = token_result(&value, &state)?;
    control.check()?;
    let profile = fetch_profile(&http, &config, &token, expected_id)?;
    control.check()?;
    let saved = SavedSession {
        format_version: 1,
        client_id: config.client_id.clone(),
        redirect_uri: config.redirect.to_string(),
        access_token: token.as_str().to_owned(),
        refresh_token: value["refresh_token"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        device_id: approval.device_id.as_str().to_owned(),
        expires_at: expiry(&value),
        profile: profile.clone(),
    };
    let cache_warning = session_store::save(&saved).is_err();
    let client = VkClient::new(std::mem::take(&mut *token))
        .map_err(LoginError::Verify)?
        .using_app_id(config.client_id.clone());
    Ok(SignedIn {
        profile,
        client,
        cache_warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config::new("12345", "https://example.com/callback/").unwrap()
    }
    #[test]
    fn pkce_matches_rfc7636_vector() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
    #[test]
    fn authorization_uses_https_return_page_with_code_and_pkce() {
        let url = authorize_url(&config(), "test-state", "test-verifier");
        let params: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(url.host_str(), Some("id.vk.ru"));
        assert_eq!(params["redirect_uri"], "https://example.com/callback/");
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["client_id"], "12345");
        assert_eq!(params["code_challenge"], challenge("test-verifier"));
        assert!(!params.contains_key("code_verifier"));
    }
    #[test]
    fn native_return_requires_state_and_rejects_duplicate_or_secret_fields() {
        for url in [
            "vkfast://oauth/callback?state=wrong&code=x&device_id=d",
            "vkfast://other/callback?state=right&code=x&device_id=d",
            "vkfast://oauth/other?state=right&code=x&device_id=d",
            "https://example.com/callback?state=right&code=x&device_id=d",
            "vkfast://oauth/callback?state=right&state=wrong&code=x&device_id=d",
            "vkfast://oauth/callback?state=right&code=x",
            "vkfast://oauth/callback?state=right&code=x&device_id=d&access_token=secret",
            "vkfast://oauth/callback?state=right&code=x&device_id=d#fragment",
        ] {
            assert!(matches!(parse_return(url, "right"), Callback::Ignore));
        }
        assert!(matches!(
            parse_return(
                "vkfast://oauth/callback?state=right&error=access_denied",
                "right"
            ),
            Callback::Denied
        ));
        let Callback::Approved(approval) = parse_return(
            "vkfast://oauth/callback?state=right&code=synthetic-code&device_id=synthetic-device",
            "right",
        ) else {
            panic!("valid callback rejected");
        };
        assert_eq!(approval.code.as_str(), "synthetic-code");
        assert_eq!(approval.device_id.as_str(), "synthetic-device");
    }
    #[test]
    fn registration_errors_are_not_mistaken_for_a_working_redirect() {
        assert!(redirect_rejected(
            r#"<script>window.init={"error":{"code":"invalid_request","message":"redirect_uri is invalid"}};</script>"#
        ));
        assert!(!redirect_rejected(
            r#"window.init={"auth":{"host_app_id":12345},"data":{}};"#
        ));
    }
    #[test]
    fn old_saved_redirect_remains_valid_for_refresh_but_cannot_start_a_listener() {
        let old = Config::new("12345", "http://localhost/callback").unwrap();
        assert!(!old.supports_native_return());
        assert!(config().supports_native_return());
        for redirect in [
            "https://user:pass@example.com/callback",
            "http://evil.invalid/callback",
            "https://example.com/callback#bad",
            "https://example.com/callback?bad=1",
        ] {
            assert!(Config::new("12345", redirect).is_err());
        }
    }
    #[test]
    fn verifies_token_state_and_vk_id_profile() {
        let value = serde_json::json!({"access_token":"test-token","state":"right","user_id":42});
        assert!(token_result(&value, "wrong").is_err());
        let (token, user) = token_result(&value, "right").unwrap();
        assert_eq!(token.as_str(), "test-token");
        assert_eq!(user, 42);
        let info = serde_json::json!({"user":{"user_id":"42","first_name":"Test","avatar":"https://images.example.test/self.jpg"}});
        assert_eq!(vk_profile(&info, 42).unwrap().id, 42);
        assert_eq!(
            vk_profile(&info, 42).unwrap().avatar_url.as_deref(),
            Some("https://images.example.test/self.jpg")
        );
        assert!(matches!(
            vk_profile(&info, 43),
            Err(LoginError::AccountMismatch)
        ));
    }
}
