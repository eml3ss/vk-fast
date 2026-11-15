//! NetEase Cloud Music (music.163.com), a large catalogue with timed lyrics,
//! strong for Asian music. Only the title and artist reach its search, and
//! a song ID its lyrics request: no VK data or credentials.
use super::{
    Error, Line, Lyrics, MAX_RESPONSE, MAX_TEXT, Provider, Query, parse_lrc, same_artist,
    same_title, validate_lines,
};
use crate::diagnostics::SendLogged;
use serde_json::Value;
use std::{io::Read, sync::Arc};
use zeroize::Zeroizing;

const API: &str = "https://music.163.com/api/";
/// Candidates whose lyrics are asked for, best first.
const TRIED: usize = 3;

impl Provider {
    fn netease_request(
        &self,
        path: &'static str,
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
            .get(format!("{API}{path}"))
            .query(params)
            .header("Accept", "application/json")
            .header("Referer", "https://music.163.com/")
            .send_logged("lyrics", &format!("netease {path}"))
            .map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(super::status_error(status, 300));
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
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
        match value.get("code").and_then(Value::as_i64) {
            Some(200) | None => Ok(value),
            Some(code) => Err(super::status_error(u16::try_from(code).unwrap_or(500), 300)),
        }
    }

    /// The songs NetEase finds for the title and main artist, checked by
    /// title, artist and length; the first one with lyrics, timed first.
    pub(super) fn netease(
        &self,
        query: &Query,
        canceled: &impl Fn() -> bool,
    ) -> Result<Arc<Lyrics>, Error> {
        let found = self.netease_request(
            "search/get",
            &[
                ("s", format!("{} {}", query.title, query.artist)),
                ("type", "1".into()),
                ("limit", "20".into()),
                ("offset", "0".into()),
            ],
            canceled,
        )?;
        let candidates = candidates(&found, query);
        let mut plain = None;
        let mut first = None;
        for id in candidates.into_iter().take(TRIED) {
            let record = self.netease_request(
                "song/lyric",
                &[
                    ("id", id.to_string()),
                    ("lv", "1".into()),
                    ("kv", "1".into()),
                    ("tv", "-1".into()),
                ],
                canceled,
            )?;
            match parse(&record) {
                Ok(lyrics) if lyrics.synced || lyrics.instrumental => return Ok(Arc::new(lyrics)),
                Ok(lyrics) => {
                    plain.get_or_insert(lyrics);
                }
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
        }
        plain.map(Arc::new).ok_or(first.unwrap_or(Error::NotFound))
    }
}

/// IDs of the found songs with the same title and main artist and a length
/// within 5 % (at least 8 s), in NetEase's order.
fn candidates(found: &Value, query: &Query) -> Vec<u64> {
    let Some(songs) = found.pointer("/result/songs").and_then(Value::as_array) else {
        return Vec::new();
    };
    songs
        .iter()
        .take(50)
        .filter(|song| {
            song.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| same_title(name, &query.title))
        })
        .filter(|song| {
            song.get("artists")
                .or_else(|| song.get("ar"))
                .and_then(Value::as_array)
                .is_some_and(|artists| {
                    artists.iter().any(|artist| {
                        artist
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|name| same_artist(name, &query.artist))
                    })
                })
        })
        .filter(|song| {
            let length = song
                .get("duration")
                .or_else(|| song.get("dt"))
                .and_then(Value::as_u64)
                .map(|ms| ms as f64 / 1000.0);
            match length {
                Some(length) if query.duration > 0 => {
                    let wanted = f64::from(query.duration);
                    (length - wanted).abs() <= (wanted * 0.05).max(8.0)
                }
                _ => true,
            }
        })
        .filter_map(|song| song.get("id").and_then(Value::as_u64))
        .collect()
}

/// A song's lyrics: timed lines from its LRC, without the credits NetEase
/// puts first ("作词 : …"); plain lines when it has no timing.
fn parse(record: &Value) -> Result<Lyrics, Error> {
    if record.get("nolyric").and_then(Value::as_bool) == Some(true)
        || record.get("uncollected").and_then(Value::as_bool) == Some(true)
    {
        return Err(Error::NotFound);
    }
    let text = record
        .pointer("/lrc/lyric")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if text.len() > MAX_TEXT {
        return Err(Error::Format);
    }
    if text.trim().is_empty() {
        return Err(Error::NotFound);
    }
    let mut lyrics = Lyrics {
        lines: Vec::new(),
        synced: false,
        instrumental: false,
        _copyright: String::new(),
        source_url: None,
        tracking_url: None,
    };
    let timed = parse_lrc(text).unwrap_or_default();
    let words = |line: &&Line| !line.text.trim().is_empty() && !credit(&line.text);
    if timed.iter().filter(words).count() > 0 {
        let mut lines: Vec<Line> = timed
            .into_iter()
            .filter(|line| !credit(&line.text))
            .collect();
        // The empty line after the credits opens the song: drop it.
        while lines
            .first()
            .is_some_and(|line| line.text.trim().is_empty())
        {
            lines.remove(0);
        }
        lyrics.instrumental = instrumental(&lines);
        lyrics.synced = !lyrics.instrumental;
        lyrics.lines = if lyrics.instrumental {
            Vec::new()
        } else {
            lines
        };
    } else {
        let lines: Vec<Line> = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('[') && !credit(line))
            .map(|line| Line {
                at: None,
                text: line.trim_end().to_owned(),
            })
            .collect();
        if lines.iter().all(|line| line.text.trim().is_empty()) {
            return Err(Error::NotFound);
        }
        lyrics.instrumental = instrumental(&lines);
        lyrics.lines = if lyrics.instrumental {
            Vec::new()
        } else {
            lines
        };
    }
    validate_lines(&lyrics.lines)?;
    Ok(lyrics)
}

/// A credit line: a role, a colon, names ("作词 : …", "Producer: …"), or
/// NetEase's JSON credit records.
fn credit(text: &str) -> bool {
    let text = text.trim();
    if text.starts_with('{') && text.ends_with('}') {
        return true;
    }
    let Some((role, _)) = text.split_once([':', '：']) else {
        return false;
    };
    let role = role.trim();
    const ROLES: [&str; 24] = [
        "作词",
        "作曲",
        "编曲",
        "制作人",
        "制作",
        "混音",
        "母带",
        "录音",
        "和声",
        "吉他",
        "贝斯",
        "鼓",
        "监制",
        "出品",
        "统筹",
        "词",
        "曲",
        "op",
        "sp",
        "lyrics",
        "composer",
        "arranger",
        "producer",
        "mixing",
    ];
    ROLES.iter().any(|known| role.eq_ignore_ascii_case(known))
}

/// NetEase's line for a song without words: "纯音乐，请欣赏" (pure music).
fn instrumental(lines: &[Line]) -> bool {
    let words: Vec<&str> = lines
        .iter()
        .map(|line| line.text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    words.len() <= 1 && words.first().is_some_and(|text| text.contains("纯音乐"))
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
            album: String::new(),
            duration,
            speed_variant: false,
        }
    }

    #[test]
    fn timed_lyrics_lose_the_credits_and_plain_ones_stay_plain() {
        let record = json!({"lrc": {"lyric":
            "[00:00.000] 作词 : Someone\n[00:00.025] 作曲 : Someone\n[00:00.050] 制作人 : Someone/Else\n[00:00.100]\n[00:01.136]First line\n[00:04.872]Second line\n"}});
        let lyrics = parse(&record).unwrap();
        assert!(lyrics.synced);
        let texts: Vec<_> = lyrics.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["First line", "Second line"]);
        assert_eq!(lyrics.lines[0].at, Some(1.136));
        let plain = parse(&json!({"lrc": {"lyric": "Only words\nNo timing"}})).unwrap();
        assert!(!plain.synced && plain.lines.len() == 2);
        let pure = parse(&json!({"lrc": {"lyric": "[00:00.00] 纯音乐，请欣赏\n"}})).unwrap();
        assert!(pure.instrumental && pure.lines.is_empty());
        for missing in [
            json!({"nolyric": true}),
            json!({"uncollected": true, "lrc": {"lyric": ""}}),
            json!({"lrc": {"lyric": "  "}}),
        ] {
            assert_eq!(parse(&missing).unwrap_err(), Error::NotFound);
        }
        assert!(credit("Producer: Someone") && credit("作词：某人") && !credit("Love: a song"));
    }

    #[test]
    fn only_the_same_song_by_the_same_artist_and_length_is_a_candidate() {
        let found = json!({"result": {"songs": [
            {"id": 1, "name": "Other Song", "artists": [{"name": "Synthetic Artist"}], "duration": 200000},
            {"id": 2, "name": "Synthetic Song", "artists": [{"name": "Somebody"}], "duration": 200000},
            {"id": 3, "name": "Synthetic Song", "artists": [{"name": "Synthetic Artist"}], "duration": 290000},
            {"id": 4, "name": "Synthetic Song", "artists": [{"name": "X"}, {"name": "Synthetic Artist"}], "duration": 201000},
            {"id": 5, "name": "Synthetic Song (Live)", "artists": [{"name": "Synthetic Artist"}], "duration": 199000}
        ]}});
        let ids = candidates(&found, &query(200));
        assert_eq!(ids.first(), Some(&4));
        assert!(!ids.contains(&1) && !ids.contains(&2) && !ids.contains(&3));
        assert!(candidates(&json!({"result": {}}), &query(200)).is_empty());
    }
}
