//! Release metadata stays separate from the references used by tracks/playlists.
use crate::{i18n::Language, model::AlbumRef};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Album,
    Single,
    Ep,
    #[default]
    Unknown,
}
impl Kind {
    pub fn label(self, language: Language) -> &'static str {
        match self {
            Self::Album => language.text("Альбом", "Album"),
            Self::Single => language.text("Сингл", "Single"),
            Self::Ep => "EP",
            Self::Unknown => language.text("Релиз", "Release"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub album: AlbumRef,
    pub kind: Kind,
    pub year: Option<u32>,
}
impl Release {
    pub fn from_value(value: &Value) -> Option<Self> {
        let kind = ["album_type", "release_type"]
            .into_iter()
            .filter_map(|field| value.get(field).and_then(Value::as_str))
            .find_map(|kind| match kind.trim().to_ascii_lowercase().as_str() {
                "album" | "main" | "lp" | "compilation" => Some(Kind::Album),
                "single" => Some(Kind::Single),
                "ep" | "extended_play" => Some(Kind::Ep),
                _ => None,
            })
            .unwrap_or_default();
        Some(Self {
            album: AlbumRef::from_value(value)?,
            kind,
            year: value
                .get("year")
                .and_then(Value::as_u64)
                .and_then(|year| u32::try_from(year).ok())
                .filter(|year| (1000..=9999).contains(year)),
        })
    }
    pub fn caption(&self, language: Language) -> String {
        match self.year {
            Some(year) => format!("{year} · {}", self.kind.label(language)),
            None => self.kind.label(language).into(),
        }
    }
    pub fn enrich(&mut self, release: Self) {
        if release.kind != Kind::Unknown {
            self.kind = release.kind;
        }
        if release.year.is_some() {
            self.year = release.year;
        }
        if release.album.cover_url.is_some() {
            self.album.cover_url = release.album.cover_url;
        }
        if release.album.access_key.is_some() {
            self.album.access_key = release.album.access_key;
        }
    }
}

/// Preserve VK's order and canonical identities across catalog and API pages.
pub fn merge(releases: &mut Vec<Release>, incoming: impl IntoIterator<Item = Release>) -> usize {
    let mut seen: HashSet<_> = releases
        .iter()
        .map(|release| (release.album.owner_id, release.album.id))
        .collect();
    let mut added = 0;
    for release in incoming {
        let key = (release.album.owner_id, release.album.id);
        if seen.insert(key) {
            releases.push(release);
            added += 1;
        } else if let Some(existing) = releases
            .iter_mut()
            .find(|existing| (existing.album.owner_id, existing.album.id) == key)
        {
            existing.enrich(release);
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn release_types_use_metadata_instead_of_track_count_and_merging_keeps_identity() {
        let parse = |id, kind, count| {
            Release::from_value(&json!({"owner_id":-42,"id":id,"title":"Synthetic","album_type":kind,"count":count,"year":2026})).unwrap()
        };
        assert_eq!(parse(1, "single", 12).kind, Kind::Single);
        assert_eq!(parse(2, "album", 1).kind, Kind::Album);
        assert_eq!(parse(3, "EP", 5).kind, Kind::Ep);
        assert_eq!(parse(4, "unrecognized", 1).kind, Kind::Unknown);
        let mut releases = vec![parse(1, "unrecognized", 12)];
        assert_eq!(
            merge(
                &mut releases,
                [parse(1, "single", 12), parse(2, "album", 1)]
            ),
            1
        );
        assert_eq!(releases[0].kind, Kind::Single);
        assert_eq!(
            releases.iter().map(|r| r.album.id).collect::<Vec<_>>(),
            [1, 2]
        );
    }
}
