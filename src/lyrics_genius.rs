//! Logged-out song lookup. Only title/artist metadata reaches this client.
use super::{
    Error, Lyrics, MAX_RESPONSE, Provider, Query, number, parse_plain, same_artist, same_title,
};
use crate::diagnostics::SendLogged;
use serde_json::{Value, json};
use std::{io::Read, sync::Arc};
use zeroize::Zeroizing;

impl Provider {
    fn genius_request(
        &self,
        path: &str,
        params: &[(&str, &str)],
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
            .get(format!("https://api.genius.com/{path}"))
            .query(params)
            .header("Accept", "application/json")
            .header("X-Genius-iOS-Version", "6.21.0")
            .header("X-Genius-Logged-Out", "true")
            .send_logged("lyrics", "genius request")
            .map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.parse().ok())
            .unwrap_or(300_u64)
            .clamp(30, 86400);
        if status != 200 {
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
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Format)?;
        if let Some(status) = value.pointer("/meta/status").and_then(number)
            && status != 200
        {
            return Err(super::status_error(status.try_into().unwrap_or(500), retry));
        }
        value
            .get_mut("response")
            .map(Value::take)
            .filter(Value::is_object)
            .ok_or(Error::Format)
    }

    pub(super) fn genius(
        &self,
        query: &Query,
        canceled: &impl Fn() -> bool,
    ) -> Result<Arc<Lyrics>, Error> {
        if query.title.trim().is_empty()
            || query.artist.trim().is_empty()
            || query.title.len() > 4096
            || query.artist.len() > 4096
        {
            return Err(Error::NotFound);
        }
        let search = format!("{} {}", query.title, query.artist);
        let data = self.genius_request("search/song", &[("q", &search)], canceled)?;
        let id = matching_song(&data, query)?.to_string();
        let song = self.genius_request(
            &format!("songs/{id}"),
            &[("text_format", "plain")],
            canceled,
        )?;
        parse_song(song.get("song").ok_or(Error::NotFound)?, query).map(Arc::new)
    }
}

fn original(song: &Value) -> bool {
    let artist = song
        .get("artist_names")
        .and_then(Value::as_str)
        .or_else(|| song.pointer("/primary_artist/name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_lowercase();
    let title = song
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    !(artist.contains("genius")
        && (artist.contains("translation") || artist.contains("romanization")))
        && ![
            "(romanized)",
            "(romanization)",
            " translation)",
            " перевод)",
        ]
        .iter()
        .any(|word| title.contains(word))
}

fn song_matches(song: &Value, query: &Query) -> bool {
    if !original(song) {
        return false;
    }
    let Some(title) = song.get("title").and_then(Value::as_str) else {
        return false;
    };
    // Genius often appends an English gloss to the original title, not its
    // lyrics; for Asian songs that gloss can be the title everywhere else
    // ("승인해주세요지니님! (Approve Please, Genie!)").
    let title_matches = same_title(title, &query.title)
        || title.split_once(" (").is_some_and(|(original, gloss)| {
            same_title(original, &query.title)
                || gloss
                    .strip_suffix(')')
                    .is_some_and(|gloss| same_title(gloss, &query.title))
        });
    title_matches
        && [
            song.get("artist_names"),
            song.pointer("/primary_artist/name"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|artist| same_artist(artist, &query.artist))
}

fn matching_song(data: &Value, query: &Query) -> Result<u64, Error> {
    data.get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| section.get("hits").and_then(Value::as_array))
        .flatten()
        .chain(
            data.get("hits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .take(40)
        .filter_map(|hit| hit.get("result"))
        .find_map(|song| {
            song_matches(song, query)
                .then(|| song.get("id").and_then(number).filter(|id| *id > 0))
                .flatten()
        })
        .ok_or(Error::NotFound)
}

fn parse_song(song: &Value, query: &Query) -> Result<Lyrics, Error> {
    if !song_matches(song, query) {
        return Err(Error::NotFound);
    }
    let text = song
        .pointer("/lyrics/plain")
        .and_then(Value::as_str)
        .ok_or(Error::NotFound)?;
    parse_plain(&json!({"lyrics_body":text}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_original_title_with_the_english_one_in_brackets_matches_it() {
        let query = Query {
            key: (1, 2),
            title: "Approve Please, Genie!".into(),
            artist: "TRAP CHICK".into(),
            artists: "TRAP CHICK, 重音テト, 音街ウナ".into(),
            album: String::new(),
            duration: 165,
            speed_variant: false,
        };
        let song = json!({"id":7,"title":"승인해주세요지니님! (Approve Please, Genie!)",
            "artist_names":"TRAP CHICK (Ft. 重音テト (Kasane Teto) & 音街ウナ (Otomachi Una))",
            "primary_artist":{"name":"TRAP CHICK"}});
        assert!(song_matches(&song, &query));
        let translation = json!({"id":8,
            "title":"TRAP CHICK - Approve please, Genie! ft. Kasane Teto & Otomachi Una (English Translation)",
            "artist_names":"Genius English Translations"});
        assert!(!song_matches(&translation, &query));
        let other = json!({"id":9,"title":"Одобряй, Джинн-Сан!( Approve Please, Genie! )",
            "artist_names":"GENRELESS"});
        assert!(!song_matches(&other, &query));
    }

    #[test]
    fn search_skips_wrong_songs_and_translations_and_keeps_original_script() {
        let query = Query {
            key: (1, 2),
            title: "Тестовая песня".into(),
            artist: "Автор".into(),
            artists: "Автор".into(),
            album: String::new(),
            duration: 180,
            speed_variant: false,
        };
        let song = json!({"id":7,"title":"Тестовая песня (Synthetic gloss)","artist_names":"Гость (Ft. Автор)","lyrics":{"plain":"[Куплет]\nТестовая строка\n\nДругой пример"}});
        let data = json!({"sections":[{"hits":[
            {"result":{"id":1,"title":"Чужая песня","artist_names":"Автор"}},
            {"result":{"id":2,"title":"Тестовая песня (English Translation)","artist_names":"Genius English Translations"}},
            {"result":song}
        ]}]});
        assert_eq!(matching_song(&data, &query).unwrap(), 7);
        let lyrics = parse_song(&song, &query).unwrap();
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines[1].text, "Тестовая строка");
        assert!(lyrics.lines.iter().all(|line| line.at.is_none()));
    }
}
