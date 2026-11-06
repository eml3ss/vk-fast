//! Small explicit RU/EN translations. Language selection never resets the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Russian,
    English,
}

impl Language {
    pub fn system() -> Self {
        sys_locale::get_locale()
            .as_deref()
            .map(Self::from_locale)
            .unwrap_or(Self::English)
    }

    fn from_locale(locale: &str) -> Self {
        let language = locale
            .trim()
            .split(['-', '_', '.', '@', ':'])
            .next()
            .unwrap_or_default();
        if language.eq_ignore_ascii_case("ru") {
            Self::Russian
        } else {
            Self::English
        }
    }

    pub fn text(self, russian: &'static str, english: &'static str) -> &'static str {
        match self {
            Self::Russian => russian,
            Self::English => english,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ru" => Some(Self::Russian),
            "en" => Some(Self::English),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_locale_supports_russian_variants_and_defaults_other_languages_to_english() {
        for locale in [
            "ru",
            "ru-RU",
            "ru_RU.UTF-8",
            "RU-by",
            "ru-Cyrl-RU",
            " ru_RU@variant ",
        ] {
            assert_eq!(Language::from_locale(locale), Language::Russian, "{locale}");
        }
        for locale in [
            "en",
            "en-US",
            "en_GB.UTF-8",
            "uk-UA",
            "de-DE",
            "ja-JP",
            "C",
            "C.UTF-8",
            "POSIX",
            "",
            "russian",
        ] {
            assert_eq!(Language::from_locale(locale), Language::English, "{locale}");
        }
    }
}
