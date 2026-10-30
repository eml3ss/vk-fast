//! VK API transport. Only the documented users.get endpoint is guaranteed by the
//! public schema. audio.get is an experimental compatibility adapter.

use std::{
    fmt,
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{catalog, i18n::Language, model::AudioPage};

const API_ROOT: &str = "https://api.vk.com/method";
const API_VERSION: &str = "5.199";
const CATALOG_ROOT: &str = "https://api.vk.ru/method";
const CATALOG_VERSION: &str = "5.282";
pub const PAGE_SIZE: usize = 100;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

// Deliberately no Debug implementation: this type owns a credential.
#[derive(Clone)]
pub struct VkClient {
    http: Client,
    token: Arc<Zeroizing<String>>,
    app_id: Option<String>,
    catalog: Arc<Mutex<Option<CatalogCursor>>>,
}

struct CatalogCursor {
    owner_id: i64,
    section_id: String,
    next_from: Option<String>,
    offset: usize,
    finished: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VkError {
    InvalidToken,
    ClientSetup,
    Timeout,
    Network,
    Http(u16),
    Api(i64),
    Method { method: &'static str, code: i64 },
    UnsupportedCatalog,
    InvalidResponse,
    ResponseTooLarge,
}

impl VkError {
    pub fn localized(&self, language: Language) -> String {
        let t = |ru, en| language.text(ru, en);
        match self {
            Self::InvalidToken => t(
                "Вставьте только access token, без URL, пробелов и префикса Bearer.",
                "Paste only the access token, without a URL, spaces, or a Bearer prefix.",
            )
            .into(),
            Self::ClientSetup => t(
                "Не удалось создать HTTP-клиент.",
                "Could not initialize the HTTP client.",
            )
            .into(),
            Self::Timeout => t(
                "VK не ответил за 20 секунд. Попробуйте ещё раз.",
                "VK did not respond within 20 seconds. Try again.",
            )
            .into(),
            Self::Network => t(
                "Не удалось связаться с VK. Проверьте подключение к интернету.",
                "Could not connect to VK. Check your internet connection.",
            )
            .into(),
            Self::InvalidResponse => t(
                "Неожиданный формат ответа VK. Возможно, API изменился.",
                "Unexpected response format from VK. The API may have changed.",
            )
            .into(),
            Self::ResponseTooLarge => t(
                "Ответ VK слишком большой. Запрос остановлен.",
                "VK's response is too large. The request was stopped.",
            )
            .into(),
            Self::Http(code) => format!(
                "{} {code}. {}",
                t("VK вернул HTTP", "VK returned HTTP"),
                t("Попробуйте позже.", "Try again later.")
            ),
            Self::UnsupportedCatalog => t(
                "VK вернул каталог, но раздел «Моя музыка» или его формат не распознан.",
                "VK returned a catalog, but the My music section or its format was not recognized.",
            )
            .into(),
            Self::Method { method, code } => {
                format!("{} [{}]", Self::Api(*code).localized(language), method)
            }
            Self::Api(code) => {
                let message = match code {
                    3 => t(
                        "VK не распознал вызванный метод API.",
                        "VK did not recognize the requested API method.",
                    ),
                    5 => t(
                        "Токен недействителен или истёк. Нужен пользовательский токен VK API.",
                        "The token is invalid or expired. Use a VK API user token.",
                    ),
                    6 | 9 | 29 => t(
                        "VK ограничил частоту запросов. Подождите перед повторной попыткой.",
                        "VK has limited your requests. Wait before trying again.",
                    ),
                    7 | 15 | 20 | 21 | 23 | 201 | 1051 => t(
                        "У этого токена или приложения нет доступа к методу. Доступ к профилю не гарантирует доступ к музыке.",
                        "This token or app cannot access this method. Profile access does not guarantee music access.",
                    ),
                    14 => t(
                        "VK требует капчу. Этот прототип пока не поддерживает её прохождение.",
                        "VK requires a CAPTCHA. This prototype does not support it yet.",
                    ),
                    17 => t(
                        "VK требует дополнительную проверку аккаунта. Выполните её в официальном клиенте.",
                        "VK requires additional account verification. Complete it in the official client.",
                    ),
                    100 => t(
                        "VK отклонил параметры запроса. Возможно, контракт аудиометода изменился.",
                        "VK rejected the request parameters. The audio API may have changed.",
                    ),
                    _ => t("VK отклонил запрос.", "VK rejected the request."),
                };
                format!("{message} (VK {code})")
            }
        }
    }
}

impl fmt::Display for VkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.localized(Language::Russian))
    }
}

impl std::error::Error for VkError {}

#[derive(Deserialize)]
struct ApiError {
    error_code: i64,
    // Do not deserialize error_msg or request_params: VK may echo credentials.
}

#[derive(Deserialize)]
struct Envelope<T> {
    response: Option<T>,
    error: Option<ApiError>,
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, VkError> {
    let envelope: Envelope<T> =
        serde_json::from_slice(bytes).map_err(|_| VkError::InvalidResponse)?;
    if let Some(error) = envelope.error {
        return Err(VkError::Api(error.error_code));
    }
    envelope.response.ok_or(VkError::InvalidResponse)
}

fn transport_error(error: reqwest::Error) -> VkError {
    // Never forward reqwest's raw error text/URL or a server response to the UI.
    if error.is_timeout() {
        VkError::Timeout
    } else {
        VkError::Network
    }
}

impl VkClient {
    pub fn new(token: String) -> Result<Self, VkError> {
        Self::with_user_agent(token, concat!("vk-fast/", env!("CARGO_PKG_VERSION")))
    }

    pub fn with_user_agent(token: String, user_agent: &str) -> Result<Self, VkError> {
        let token = Zeroizing::new(token);
        let value = token.trim();
        if value.is_empty()
            || value.len() > 8192
            || !value.is_ascii()
            || value.chars().any(char::is_whitespace)
            || value.starts_with("http://")
            || value.starts_with("https://")
            || value.starts_with("access_token=")
        {
            return Err(VkError::InvalidToken);
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(Policy::none())
            .user_agent(user_agent)
            .build()
            .map_err(|_| VkError::ClientSetup)?;
        Ok(Self {
            http,
            token: Arc::new(Zeroizing::new(value.to_owned())),
            app_id: None,
            catalog: Arc::new(Mutex::new(None)),
        })
    }

    pub fn using_app_id(mut self, app_id: String) -> Self {
        self.app_id = Some(app_id);
        self
    }

    fn request(&self, method: &str, extra: &[(&str, String)]) -> reqwest::blocking::RequestBuilder {
        self.request_at(API_ROOT, API_VERSION, method, extra)
    }

    fn request_at(
        &self,
        root: &str,
        version: &str,
        method: &str,
        extra: &[(&str, String)],
    ) -> reqwest::blocking::RequestBuilder {
        // Fixed HTTPS endpoints; credentials remain in the POST body.
        let mut params = vec![("access_token", self.token.as_str()), ("v", version)];
        params.extend(extra.iter().map(|(key, value)| (*key, value.as_str())));
        self.http.post(format!("{root}/{method}")).form(&params)
    }

    fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        extra: &[(&str, String)],
    ) -> Result<T, VkError> {
        self.read_response(self.request(method, extra))
    }

    fn read_response<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<T, VkError> {
        let response = request.send().map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(VkError::Http(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| VkError::Network)?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(VkError::ResponseTooLarge);
        }
        decode(&bytes)
    }

    fn catalog_call(
        &self,
        method: &'static str,
        extra: &[(&str, String)],
    ) -> Result<serde_json::Value, VkError> {
        let mut params = extra.to_vec();
        params.push(("lang", "ru".into()));
        if let Some(app_id) = &self.app_id {
            params.push(("client_id", app_id.clone()));
        }
        self.read_response(self.request_at(CATALOG_ROOT, CATALOG_VERSION, method, &params))
            .map_err(|error| match error {
                VkError::Api(code) => VkError::Method { method, code },
                other => other,
            })
    }

    pub fn audio_page(&self, owner_id: i64, offset: usize) -> Result<AudioPage, VkError> {
        let mut state = self.catalog.lock().map_err(|_| VkError::InvalidResponse)?;
        if offset == 0 {
            *state = None;
        }
        if state.is_none() {
            match self.call(
                "audio.get",
                &[
                    ("owner_id", owner_id.to_string()),
                    ("count", PAGE_SIZE.to_string()),
                    ("offset", offset.to_string()),
                ],
            ) {
                Ok(page) => return Ok(page),
                Err(VkError::Api(3 | 23)) if offset == 0 => {
                    let catalog = self
                        .catalog_call("catalog.getAudio", &[("owner_id", owner_id.to_string())])?;
                    let section_id = catalog::select_section(catalog, owner_id)
                        .map_err(|_| VkError::UnsupportedCatalog)?;
                    *state = Some(CatalogCursor {
                        owner_id,
                        section_id,
                        next_from: None,
                        offset: 0,
                        finished: false,
                    });
                }
                Err(VkError::Api(code)) => {
                    return Err(VkError::Method {
                        method: "audio.get",
                        code,
                    });
                }
                Err(error) => return Err(error),
            }
        }
        let cursor = state.as_mut().ok_or(VkError::InvalidResponse)?;
        if cursor.owner_id != owner_id || cursor.offset != offset {
            return Err(VkError::InvalidResponse);
        }
        if cursor.finished {
            return Ok(AudioPage {
                count: None,
                items: vec![],
                exhausted: Some(true),
            });
        }
        // Metadata-only pages can precede the actual audio block. Bound the scan.
        for _ in 0..3 {
            let previous = cursor.next_from.clone();
            let mut params = vec![("section_id", cursor.section_id.clone())];
            if let Some(next) = &previous {
                params.push(("start_from", next.clone()));
            }
            let response = self.catalog_call("catalog.getSection", &params)?;
            let page = catalog::parse_page(response).map_err(|_| VkError::UnsupportedCatalog)?;
            if page.next_from.is_some() && page.next_from == previous {
                return Err(VkError::InvalidResponse);
            }
            cursor.next_from = page.next_from;
            cursor.finished = cursor.next_from.is_none();
            if !page.items.is_empty() || cursor.finished {
                cursor.offset += page.items.len();
                return Ok(AudioPage {
                    count: None,
                    items: page.items,
                    exhausted: Some(cursor.finished),
                });
            }
        }
        Err(VkError::UnsupportedCatalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Profile;

    #[test]
    fn request_contains_auth_and_audio_parameters_in_body_only() {
        let client = VkClient::new("test-secret".to_owned()).unwrap();
        let request = client
            .request(
                "audio.get",
                &[
                    ("owner_id", "42".to_owned()),
                    ("count", "100".to_owned()),
                    ("offset", "200".to_owned()),
                ],
            )
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.vk.com/method/audio.get"
        );
        assert!(request.url().query().is_none());
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        let form_url = reqwest::Url::parse(&format!("https://example.invalid/?{body}")).unwrap();
        let form: std::collections::HashMap<_, _> = form_url.query_pairs().collect();
        for (key, value) in [
            ("access_token", "test-secret"),
            ("v", "5.199"),
            ("owner_id", "42"),
            ("count", "100"),
            ("offset", "200"),
        ] {
            assert_eq!(form.get(key).map(|v| v.as_ref()), Some(value));
        }
    }

    #[test]
    fn decodes_current_user() {
        let users: Vec<Profile> =
            decode(br#"{"response":[{"id":42,"first_name":"Test","last_name":"User"}]}"#).unwrap();
        assert_eq!(users[0].id, 42);
        assert_eq!(users[0].display_name(), "Test User");
    }

    #[test]
    fn errors_do_not_expose_server_echoed_credentials() {
        let error = decode::<Vec<Profile>>(
            br#"{"error":{"error_code":5,"error_msg":"secret-token","request_params":[{"key":"access_token","value":"secret-token"}]}}"#,
        )
        .unwrap_err();
        assert_eq!(error, VkError::Api(5));
        assert!(!error.to_string().contains("secret-token"));
        assert!(!format!("{error:?}").contains("secret-token"));
    }

    #[test]
    fn rejects_invalid_envelopes_without_echoing_them() {
        for input in [
            b"<html>secret</html>".as_slice(),
            b"{}",
            b"{\"response\":null}",
        ] {
            assert_eq!(
                decode::<Vec<Profile>>(input).unwrap_err(),
                VkError::InvalidResponse
            );
        }
    }

    #[test]
    fn audio_access_denial_is_distinct_from_invalid_auth() {
        let result = decode::<AudioPage>(br#"{"error":{"error_code":201}}"#);
        assert_eq!(result.unwrap_err(), VkError::Api(201));
    }
}
