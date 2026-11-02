//! VK lyrics returned for the exact audio id, using the connected music session.
use super::{Error, Line, Lyrics, MAX_LINES, parse_plain, validate_lines};
use crate::web_audio::WebAudioError;
use serde_json::{Value, json};

pub(super) fn error(error: WebAudioError) -> Error {
    match error {
        WebAudioError::Canceled => Error::Canceled,
        WebAudioError::NoSession
        | WebAudioError::Expired
        | WebAudioError::WrongAccount
        | WebAudioError::Unverified => Error::NoSession,
        WebAudioError::Network | WebAudioError::Timeout => Error::Network,
        WebAudioError::Http(429) => Error::RateLimited(300),
        WebAudioError::Http(403) | WebAudioError::Rejected => Error::Restricted,
        WebAudioError::Http(404) | WebAudioError::NoStream => Error::NotFound,
        _ => Error::Format,
    }
}

pub(super) fn parse(value: &Value) -> Result<Lyrics, Error> {
    let value = value.get("lyrics").unwrap_or(value);
    if let Some(timestamps) = value.get("timestamps").and_then(Value::as_array)
        && !timestamps.is_empty()
        && timestamps.len() <= MAX_LINES
    {
        let lines = timestamps
            .iter()
            .map(|row| {
                let milliseconds = row
                    .get("begin")
                    .and_then(Value::as_f64)
                    .filter(|t| t.is_finite() && (0.0..=86_400_000.0).contains(t))?;
                let text = row.get("line").and_then(Value::as_str)?.to_owned();
                Some(Line {
                    at: Some(milliseconds / 1000.0),
                    text,
                })
            })
            .collect::<Option<Vec<_>>>();
        if let Some(mut lines) = lines {
            validate_lines(&lines)?;
            if lines.iter().any(|line| !line.text.trim().is_empty()) {
                lines.sort_by(|a, b| {
                    a.at.unwrap_or_default()
                        .total_cmp(&b.at.unwrap_or_default())
                });
                return Ok(Lyrics {
                    lines,
                    synced: true,
                    instrumental: false,
                    _copyright: String::new(),
                    source_url: None,
                    tracking_url: None,
                });
            }
        }
    }
    let text = match value.get("text") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(lines)) if lines.len() <= MAX_LINES => lines
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .ok_or(Error::Format)?
            .join("\n"),
        _ => return Err(Error::NotFound),
    };
    parse_plain(&json!({"lyrics_body":text}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vk_timestamps_are_milliseconds_and_credits_are_not_part_of_lyrics() {
        let lyrics = parse(
            &json!({"credits":"Synthetic credits","lyrics":{"timestamps":[
                {"begin":2500,"end":4000,"line":"Вторая строка"},
                {"begin":1000,"end":2500,"line":"Первая строка"}
            ]}}),
        )
        .unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines[0].at, Some(1.0));
        assert_eq!(lyrics.lines[1].at, Some(2.5));
        assert_eq!(lyrics.active_at(2.0), Some(0));
        let plain = parse(&json!({"lyrics":{"text":["Тестовая строка","日本語のテスト"]},"credits":"Not a lyric"})).unwrap();
        assert!(!plain.synced);
        assert_eq!(plain.lines.len(), 2);
        assert!(parse(&json!({"lyrics":{}})).is_err());
    }
}
