//! LRCLIB (lrclib.net), an open lyrics database with timed lines. Only the
//! title, artist, album and length reach it: no VK data or credentials.
use super::{
    Error, Line, Lyrics, MAX_RESPONSE, MAX_TEXT, Provider, Query, parse_lrc, same_artist,
    same_title, validate_lines,
};
use crate::diagnostics::SendLogged;
use serde_json::Value;
use std::{io::Read, sync::Arc};
use zeroize::Zeroizing;

const API: &str = "https://lrclib.net/api/";

impl Provider {
    fn lrclib_request(
        &self,
        method: &'static str,
        params: &[(&str, String)],
        canceled: &impl Fn() -> bool,
    ) -> Result<Value, Error> {
        if canceled() {
            return Err(Error::Canceled);
        }
        if cfg!(test) {
            return Err(Error::NotFound);
        }
        let response = self
            .http
            .get(format!("{API}{method}"))
            .query(params)
            .header("Accept", "application/json")
            .send_logged("lyrics", &format!("lrclib {method}"))
            .map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        if status == 404 {
            return Err(Error::NotFound);
        }
        if status != 200 {
            let retry = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse().ok())
                .unwrap_or(300_u64)
                .clamp(30, 86400);
            return Err(super::status_error(status, retry));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        response
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Network)?;
        if canceled() {
            return Err(Error::Canceled);
        }
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err(Error::Format);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Format)
    }

    /// The exact record for the title, artists, album and length; without
    /// one, a search by title and main artist, checked like Musixmatch's.
    /// Timed lines win: an exact record with plain text only is kept while
    /// the search looks for a timed one.
    pub(super) fn lrclib(
        &self,
        query: &Query,
        canceled: &impl Fn() -> bool,
    ) -> Result<Arc<Lyrics>, Error> {
        let mut first = None;
        let mut plain = None;
        for artist in [&query.artists, &query.artist] {
            let mut params = vec![
                ("track_name", query.title.clone()),
                ("artist_name", artist.clone()),
            ];
            if !query.album.trim().is_empty() {
                params.push(("album_name", query.album.clone()));
            }
            if query.duration > 0 {
                params.push(("duration", query.duration.to_string()));
            }
            match self.lrclib_request("get", &params, canceled) {
                Ok(record) => match parse(&record, query) {
                    Ok(lyrics) if lyrics.synced || lyrics.instrumental => {
                        return Ok(Arc::new(lyrics));
                    }
                    Ok(lyrics) => {
                        plain.get_or_insert(lyrics);
                    }
                    Err(error) => {
                        first.get_or_insert(error);
                    }
                },
                Err(Error::NotFound) => {}
                Err(error) => return Err(error),
            }
            if query.artists == query.artist {
                break;
            }
        }
        let found = match self.lrclib_request(
            "search",
            &[
                ("track_name", query.title.clone()),
                ("artist_name", query.artist.clone()),
            ],
            canceled,
        ) {
            Ok(found) => found,
            Err(error) => return plain.map(Arc::new).ok_or(error),
        };
        match best_searched(&found, query) {
            Some(lyrics) if lyrics.synced || plain.is_none() => Ok(Arc::new(lyrics)),
            _ => plain.map(Arc::new).ok_or(first.unwrap_or(Error::NotFound)),
        }
    }
}

/// The search's best record: the same title and main artist, a length
/// within 5 % (at least 8 s), with lyrics; timed ones first, then the one
/// closest in length (the search's own order puts other uploads first,
/// such as lines with a romanisation glued on).
fn best_searched(found: &Value, query: &Query) -> Option<Lyrics> {
    let records = found.as_array()?;
    let wanted = f64::from(query.duration);
    let mut matches: Vec<(Lyrics, f64)> = records
        .iter()
        .take(50)
        .filter(|record| {
            record
                .get("trackName")
                .and_then(Value::as_str)
                .is_some_and(|title| same_title(title, &query.title))
                && record
                    .get("artistName")
                    .and_then(Value::as_str)
                    .is_some_and(|artist| same_artist(artist, &query.artist))
        })
        .filter_map(|record| {
            let length = record.get("duration").and_then(Value::as_f64);
            let off = match length {
                Some(length) if length.is_finite() && query.duration > 0 => (length - wanted).abs(),
                _ => f64::MAX,
            };
            parse(record, query).ok().map(|lyrics| (lyrics, off))
        })
        .filter(|(lyrics, _)| !lyrics.instrumental)
        .collect();
    // Stable: equally close records keep the search's order.
    matches
        .sort_by(|(a, a_off), (b, b_off)| (!a.synced).cmp(&!b.synced).then(a_off.total_cmp(b_off)));
    matches.into_iter().next().map(|(lyrics, _)| lyrics)
}

/// A record's lyrics: its timed lines where they fit the track's length,
/// its plain text otherwise; `instrumental` with no words.
fn parse(record: &Value, query: &Query) -> Result<Lyrics, Error> {
    let length = record
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|length| length.is_finite() && *length > 0.0);
    let fits = length.is_none_or(|length| {
        query.duration == 0 || {
            let wanted = f64::from(query.duration);
            (length - wanted).abs() <= (wanted * 0.05).max(8.0)
        }
    });
    let text = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    if text("syncedLyrics").is_some_and(|text| text.len() > MAX_TEXT)
        || text("plainLyrics").is_some_and(|text| text.len() > MAX_TEXT)
    {
        return Err(Error::Format);
    }
    let instrumental = record.get("instrumental").and_then(Value::as_bool) == Some(true);
    let mut lyrics = Lyrics {
        lines: Vec::new(),
        synced: false,
        instrumental,
        _copyright: String::new(),
        source_url: None,
        tracking_url: None,
    };
    if let Some(synced) = text("syncedLyrics").filter(|_| fits && !query.speed_variant)
        && let Ok(lines) = parse_lrc(synced)
        && lines.iter().any(|line| !line.text.trim().is_empty())
    {
        lyrics.lines = lines;
        lyrics.synced = true;
    } else if let Some(plain) = text("plainLyrics") {
        lyrics.lines = plain
            .lines()
            .map(|line| Line {
                at: None,
                text: line.trim_end().to_owned(),
            })
            .collect();
    } else if !instrumental {
        return Err(Error::NotFound);
    }
    validate_lines(&lyrics.lines)?;
    Ok(lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn query(duration: u32) -> Query {
        Query {
            key: (1, 1),
            title: "Synthetic Song".into(),
            artist: "Synthetic Artist".into(),
            artists: "Synthetic Artist".into(),
            album: "Synthetic Album".into(),
            duration,
            speed_variant: false,
        }
    }

    #[test]
    fn a_record_gives_timed_lines_when_they_fit_and_plain_text_otherwise() {
        let record = json!({
            "trackName": "Synthetic Song", "artistName": "Synthetic Artist",
            "duration": 200.0, "instrumental": false,
            "plainLyrics": "First line\nSecond line",
            "syncedLyrics": "[00:01.50] First line\n[00:04.00] Second line"
        });
        let timed = parse(&record, &query(201)).unwrap();
        assert!(timed.synced);
        assert_eq!(timed.lines[1].at, Some(4.0));
        // Another version's timing (a minute longer) is not trusted.
        let plain = parse(&record, &query(260)).unwrap();
        assert!(!plain.synced);
        assert_eq!(plain.lines.len(), 2);
        assert!(plain.lines.iter().all(|line| line.at.is_none()));
        let instrumental = parse(&json!({"instrumental": true}), &query(200)).unwrap();
        assert!(instrumental.instrumental && instrumental.lines.is_empty());
        assert_eq!(
            parse(&json!({"plainLyrics": " "}), &query(200)).unwrap_err(),
            Error::NotFound
        );
    }

    #[test]
    fn a_search_takes_the_same_song_by_the_same_artist_timed_first() {
        let found = json!([
            {"trackName": "Other Song", "artistName": "Synthetic Artist", "duration": 200.0,
             "syncedLyrics": "[00:01.00] wrong song"},
            {"trackName": "Synthetic Song", "artistName": "Somebody Else", "duration": 200.0,
             "syncedLyrics": "[00:01.00] wrong artist"},
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 200.0,
             "plainLyrics": "plain words"},
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 199.0,
             "syncedLyrics": "[00:01.00] timed words"}
        ]);
        let best = best_searched(&found, &query(200)).unwrap();
        assert!(best.synced);
        assert_eq!(best.lines[0].text, "timed words");
        assert!(best_searched(&json!([]), &query(200)).is_none());
    }

    #[test]
    fn among_timed_records_the_one_closest_in_length_wins() {
        // The search lists another upload first (lines with a
        // romanisation glued on, a few seconds shorter), and one whose
        // unsupported offset leaves only plain text.
        let found = json!([
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 199.0,
             "plainLyrics": "plain from an offset record",
             "syncedLyrics": "[offset:-64036]\n[00:01.00] offset words"},
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 197.0,
             "syncedLyrics": "[00:01.00] glued words"},
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 200.0,
             "syncedLyrics": "[00:01.00] closest words"},
            {"trackName": "Synthetic Song", "artistName": "Synthetic Artist", "duration": 201.0,
             "syncedLyrics": "[00:01.00] near words"}
        ]);
        let best = best_searched(&found, &query(200)).unwrap();
        assert!(best.synced);
        assert_eq!(best.lines[0].text, "closest words");
    }
}
