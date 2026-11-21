//! Artists the user blocked from the artist menu, kept on this computer
//! only (`blocked-artists.json` beside the preferences): their tracks show
//! greyed out like unavailable ones and are skipped when playing.
//!
//! A track is blocked when any artist credited on it is: by VK's artist ID
//! where both sides have one (two artists may share a name), by name
//! otherwise.
use serde::{Deserialize, Serialize};

use crate::model::{ArtistRef, Track};

/// At most this many artists are kept.
pub const MAX_ARTISTS: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// VK's artist ID; empty when VK gave only a name.
    #[serde(default)]
    pub id: String,
    pub name: String,
}

impl Entry {
    fn of(artist: &ArtistRef) -> Self {
        Self {
            id: artist.id.trim().to_owned(),
            name: artist.name.trim().to_owned(),
        }
    }

    fn matches(&self, artist: &ArtistRef) -> bool {
        let id = artist.id.trim();
        if !self.id.is_empty() && !id.is_empty() {
            return self.id.eq_ignore_ascii_case(id);
        }
        same_name(&self.name, &artist.name)
    }
}

fn same_name(left: &str, right: &str) -> bool {
    let normal = |name: &str| {
        name.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let left = normal(left);
    !left.is_empty() && left == normal(right)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    #[serde(default)]
    artists: Vec<Entry>,
}

impl Blocked {
    /// A list read from disk, without blanks or repeats, within the limit.
    pub fn cleaned(self) -> Self {
        let mut artists: Vec<Entry> = Vec::new();
        for entry in self.artists {
            let entry = Entry {
                id: entry.id.trim().chars().take(64).collect(),
                name: entry.name.trim().chars().take(512).collect(),
            };
            if entry.name.is_empty() && entry.id.is_empty() {
                continue;
            }
            if artists.iter().any(|kept| {
                (!kept.id.is_empty() && kept.id == entry.id)
                    || (kept.id.is_empty()
                        && entry.id.is_empty()
                        && same_name(&kept.name, &entry.name))
            }) {
                continue;
            }
            artists.push(entry);
            if artists.len() >= MAX_ARTISTS {
                break;
            }
        }
        Self { artists }
    }

    pub fn is_empty(&self) -> bool {
        self.artists.is_empty()
    }

    pub fn len(&self) -> usize {
        self.artists.len()
    }

    /// Whether `artist` is blocked.
    pub fn contains(&self, artist: &ArtistRef) -> bool {
        self.artists.iter().any(|entry| entry.matches(artist))
    }

    /// Whether any artist credited on `track` is blocked. A track that came
    /// without its artists list is judged by the names in its artist line.
    pub fn blocks(&self, track: &Track) -> bool {
        if self.is_empty() {
            return false;
        }
        if !track.artists.is_empty() {
            return track.artists.iter().any(|artist| self.contains(artist));
        }
        // The whole line too: the artist menu offers it as one artist, and
        // a name may hold a separator itself ("A/B", "A & B").
        std::iter::once(track.artist.as_str())
            .chain(credited_names(&track.artist))
            .any(|name| {
                self.contains(&ArtistRef {
                    id: String::new(),
                    name: name.to_owned(),
                })
            })
    }

    /// Blocks `artist`, or unblocks it when it already is; true when it is
    /// blocked afterwards.
    pub fn toggle(&mut self, artist: &ArtistRef) -> bool {
        if self.contains(artist) {
            self.artists.retain(|entry| !entry.matches(artist));
            return false;
        }
        let entry = Entry::of(artist);
        if entry.name.is_empty() && entry.id.is_empty() {
            return false;
        }
        self.artists.push(entry);
        if self.artists.len() > MAX_ARTISTS {
            self.artists.remove(0);
        }
        true
    }
}

/// The names in an artist line such as "A feat. B, C & D".
fn credited_names(line: &str) -> impl Iterator<Item = &str> {
    line.split([',', '&', '/'])
        .flat_map(|part| {
            // ASCII only: the separators are ASCII, and a full lowercase
            // can change a capital's length in bytes ("İ"), which would cut
            // the original line in the wrong place.
            let lower = part.to_ascii_lowercase();
            let cut = [" feat. ", " feat ", " ft. ", " ft "]
                .iter()
                .find_map(|word| lower.find(word).map(|at| (at, word.len())));
            match cut {
                Some((at, len)) => vec![&part[..at], &part[at + len..]],
                _ => vec![part],
            }
        })
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(id: &str, name: &str) -> ArtistRef {
        ArtistRef {
            id: id.into(),
            name: name.into(),
        }
    }

    fn track(line: &str, artists: Vec<ArtistRef>) -> Track {
        Track {
            id: 1,
            owner_id: 1,
            artist: line.into(),
            title: "Synthetic song".into(),
            subtitle: None,
            duration: 180,
            url: None,
            cover_url: None,
            artists,
            album: None,
            library_liked: None,
            release_key: None,
            library_source_key: None,
            unavailable: None,
            added: None,
            explicit: false,
        }
    }

    #[test]
    fn a_blocked_artist_blocks_every_track_it_is_credited_on() {
        let mut blocked = Blocked::default();
        assert!(blocked.toggle(&artist("101", "Synthetic Artist")));
        // By ID, as main or featured artist, even if the name differs.
        let main = track("Synthetic Artist", vec![artist("101", "Synthetic Artist")]);
        let featured = track(
            "Other feat. Renamed",
            vec![artist("7", "Other"), artist("101", "Renamed")],
        );
        assert!(blocked.blocks(&main) && blocked.blocks(&featured));
        // Another artist with the same name but its own ID is not blocked.
        let namesake = track("Synthetic Artist", vec![artist("202", "Synthetic Artist")]);
        assert!(!blocked.blocks(&namesake));
        // Without IDs the name decides, in any case and spacing, inside an
        // artist line too.
        let no_ids = track("Someone feat.  synthetic   artist", Vec::new());
        assert!(blocked.blocks(&no_ids));
        assert!(!blocked.blocks(&track("Someone Else", Vec::new())));
        // A second toggle unblocks.
        assert!(!blocked.toggle(&artist("101", "Synthetic Artist")));
        assert!(!blocked.blocks(&main) && blocked.is_empty());
    }

    #[test]
    fn a_name_only_block_and_a_saved_list_are_kept_clean() {
        let mut blocked = Blocked::default();
        assert!(blocked.toggle(&artist("", "Name Only")));
        assert!(blocked.blocks(&track("Name Only", vec![artist("55", "Name Only")])));
        let saved = Blocked {
            artists: vec![
                Entry {
                    id: "1".into(),
                    name: "A".into(),
                },
                Entry {
                    id: "1".into(),
                    name: "A again".into(),
                },
                Entry {
                    id: String::new(),
                    name: "B".into(),
                },
                Entry {
                    id: String::new(),
                    name: " b ".into(),
                },
                Entry {
                    id: String::new(),
                    name: "  ".into(),
                },
            ],
        }
        .cleaned();
        assert_eq!(saved.len(), 2);
        assert_eq!(
            credited_names("A, B & C feat. D").collect::<Vec<_>>(),
            ["A", "B", "C", "D"]
        );
    }
    #[test]
    fn a_line_in_capitals_or_with_a_separator_in_a_name_is_blocked_like_the_menu_says() {
        let mut blocked = Blocked::default();
        // Capitals whose lowercase is longer in bytes: the line is still cut
        // at "feat." and the name found, in either case.
        assert!(blocked.toggle(&artist("", "İSTANBUL ŞEHİR")));
        let caps = track("İSTANBUL ŞEHİR feat. Guest", Vec::new());
        assert!(blocked.blocks(&caps));
        assert!(blocked.blocks(&track("Guest & İSTANBUL ŞEHİR", Vec::new())));
        assert_eq!(
            credited_names("İSTANBUL ŞEHİR FEAT. Guest").collect::<Vec<_>>(),
            ["İSTANBUL ŞEHİR", "Guest"]
        );
        // A track without its artists list offers the whole line in the
        // menu: blocked there, it is blocked in the lists too.
        let line = track("Alpha/Beta", Vec::new());
        let menu = artist("", "Alpha/Beta");
        assert!(blocked.toggle(&menu));
        assert!(blocked.contains(&menu) && blocked.blocks(&line));
        // Same ID in another case is the same artist.
        assert!(blocked.toggle(&artist("Domain", "Some name")));
        assert!(blocked.blocks(&track("x", vec![artist("domain", "Other spelling")])));
    }
}
