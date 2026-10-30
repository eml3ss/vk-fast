//! Small explicit RU/EN translations. Language selection never resets the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Russian,
    English,
}

impl Language {
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
