//! Cookies belonging to VK Fast's own website sign-in session. This module
//! never opens browser profiles, browser databases, or system keyrings.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const MAX_VALUE_BYTES: usize = 16_384;

// No Debug: a website session cookie is an authentication credential.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionCookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub secure: bool,
    pub expires_unix: Option<i64>,
}

impl Drop for SessionCookie {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl SessionCookie {
    pub fn is_valid(&self) -> bool {
        matches!(
            self.domain.as_str(),
            "vk.ru"
                | ".vk.ru"
                | "www.vk.ru"
                | ".www.vk.ru"
                | "vk.com"
                | ".vk.com"
                | "www.vk.com"
                | ".www.vk.com"
                | "login.vk.ru"
                | "login.vk.com"
        ) && self.path.starts_with('/')
            && self.path.len() <= 4096
            && !self.path.chars().any(char::is_control)
            && !self.name.is_empty()
            && self.name.len() <= 1024
            && self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={} \t".contains(&byte))
            && self.value.len() <= MAX_VALUE_BYTES
            && self.value.bytes().all(
                |byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SessionCookie {
        SessionCookie {
            domain: "vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }
    }

    #[test]
    fn accepts_only_vk_website_domains_and_safe_cookie_octets() {
        let mut cookie = fixture();
        assert!(cookie.is_valid());
        for domain in [".vk.ru", "vk.com", ".www.vk.com"] {
            cookie.domain = domain.into();
            assert!(cookie.is_valid());
        }
        for domain in [
            "example.com",
            "vk.ru.example.com",
            ".id.vk.ru",
            "evilvk.com",
        ] {
            cookie.domain = domain.into();
            assert!(!cookie.is_valid());
        }
        cookie.domain = "vk.ru".into();
        for value in [
            "x\r\nHeader: value",
            "x;other=y",
            "x\"y",
            "x\\y",
            "кириллица",
        ] {
            cookie.value = value.into();
            assert!(!cookie.is_valid());
        }
    }

    #[test]
    fn rejects_invalid_names_paths_and_oversized_values() {
        let mut cookie = fixture();
        for name in ["", "name=value", "name;other", "name value"] {
            cookie.name = name.into();
            assert!(!cookie.is_valid());
        }
        cookie.name = "remixsid".into();
        cookie.path = "not-a-path".into();
        assert!(!cookie.is_valid());
        cookie.path = "/bad\npath".into();
        assert!(!cookie.is_valid());
        cookie.path = "/".into();
        cookie.value = "x".repeat(MAX_VALUE_BYTES + 1);
        assert!(!cookie.is_valid());
    }
}
