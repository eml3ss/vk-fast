//! Reuse data-dependent work while virtualized rows are repainted/scrolled.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::Arc,
};

use crate::model::Track;

type Key = (i64, i64);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Source {
    address: usize,
    length: usize,
    revision: u64,
}

impl Source {
    fn new(tracks: &[Track], revision: u64) -> Self {
        Self {
            address: tracks.as_ptr() as usize,
            length: tracks.len(),
            revision,
        }
    }
}

#[derive(Default)]
struct Membership {
    source: Option<Source>,
    owner: i64,
    first: HashMap<Key, (usize, Key)>,
    #[cfg(test)]
    builds: usize,
}

#[derive(Default)]
struct Filter {
    source: Option<Source>,
    query: String,
    indices: Arc<[usize]>,
    #[cfg(test)]
    builds: usize,
}

pub(super) enum VisibleRows {
    All(usize),
    Matching(Arc<[usize]>),
}

impl VisibleRows {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::All(length) => *length,
            Self::Matching(indices) => indices.len(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn source_index(&self, row: usize) -> usize {
        match self {
            Self::All(_) => row,
            Self::Matching(indices) => indices[row],
        }
    }
}

#[derive(Default)]
pub(super) struct TrackViewCache {
    revision: Cell<u64>,
    membership: RefCell<Membership>,
    filter: RefCell<Filter>,
}

impl TrackViewCache {
    pub(super) fn revision(&self) -> u64 {
        self.revision.get()
    }

    /// Data events invalidate even in-place, same-length metadata changes.
    pub(super) fn invalidate(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
    }

    pub(super) fn visible(&self, tracks: &[Track], query: &str) -> VisibleRows {
        let query = query.trim();
        if query.is_empty() {
            return VisibleRows::All(tracks.len());
        }
        let source = Source::new(tracks, self.revision.get());
        let mut cached = self.filter.borrow_mut();
        if cached.source != Some(source) || cached.query != query {
            let normalized = query.to_lowercase();
            cached.indices = tracks
                .iter()
                .enumerate()
                .filter(|(_, track)| track.matches(&normalized))
                .map(|(index, _)| index)
                .collect();
            cached.query.clear();
            cached.query.push_str(query);
            cached.source = Some(source);
            #[cfg(test)]
            {
                cached.builds += 1;
            }
        }
        VisibleRows::Matching(cached.indices.clone())
    }

    /// Only exact audio IDs and explicitly confirmed owned-copy sources count.
    /// Public track pages can group different versions and are not membership IDs.
    pub(super) fn library_key(&self, library: &[Track], track: &Track, owner: i64) -> Option<Key> {
        let source = Source::new(library, self.revision.get());
        let mut cached = self.membership.borrow_mut();
        if cached.source != Some(source) || cached.owner != owner {
            cached.first.clear();
            for (position, saved) in library.iter().enumerate() {
                if saved.owner_id != owner {
                    continue;
                }
                for key in [Some(saved.key()), saved.library_source_key]
                    .into_iter()
                    .flatten()
                {
                    cached.first.entry(key).or_insert((position, saved.key()));
                }
            }
            cached.source = Some(source);
            cached.owner = owner;
            #[cfg(test)]
            {
                cached.builds += 1;
            }
        }
        [Some(track.key()), track.library_source_key]
            .into_iter()
            .flatten()
            .filter_map(|key| cached.first.get(&key))
            .min_by_key(|entry| entry.0)
            .map(|entry| entry.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(owner_id: i64, id: i64, library_source_key: Option<Key>) -> Track {
        Track {
            owner_id,
            id,
            library_source_key,
            ..crate::model::demo_tracks().remove(0)
        }
    }

    #[test]
    fn membership_requires_exact_audio_or_confirmed_source_for_the_current_account() {
        let library = vec![
            track(-200, 1, None),
            track(42, 2, Some((-300, 5))),
            track(42, 3, None),
            track(99, 4, Some((-300, 5))),
        ];
        let cache = TrackViewCache::default();
        for (query, expected) in [
            (track(-200, 1, None), None),
            (track(-300, 5, None), Some((42, 2))),
            (track(42, 2, Some((-300, 5))), Some((42, 2))),
            (track(42, 3, None), Some((42, 3))),
            (track(-300, 6, None), None),
        ] {
            assert_eq!(cache.library_key(&library, &query, 42), expected);
        }
        assert_eq!(cache.membership.borrow().builds, 1);
        assert_eq!(
            cache.library_key(&library, &track(-300, 5, None), 99),
            Some((99, 4))
        );
        assert_eq!(cache.membership.borrow().builds, 2);
    }

    #[test]
    fn repeated_frames_reuse_filtering_and_empty_filter_never_builds_indices() {
        let mut tracks = vec![track(42, 1, None), track(42, 2, None)];
        tracks[0].title = "МОЙ ТРЕК".into();
        tracks[1].title = "Different".into();
        tracks[1].subtitle = Some("Slowed".into());
        let cache = TrackViewCache::default();
        assert!(matches!(cache.visible(&tracks, "  "), VisibleRows::All(2)));
        assert_eq!(cache.filter.borrow().builds, 0);
        for _ in 0..100 {
            let visible = cache.visible(&tracks, "трек");
            assert_eq!(visible.len(), 1);
            assert_eq!(visible.source_index(0), 0);
        }
        assert_eq!(cache.filter.borrow().builds, 1);
        assert_eq!(cache.visible(&tracks, "slowed").source_index(0), 1);
        assert_eq!(cache.filter.borrow().builds, 2);
    }

    #[test]
    fn invalidation_covers_same_allocation_same_length_metadata_changes_and_replacement() {
        let mut tracks = vec![track(42, 1, Some((-200, 9)))];
        tracks[0].title = "Before".into();
        let cache = TrackViewCache::default();
        assert_eq!(cache.visible(&tracks, "before").len(), 1);
        assert_eq!(
            cache.library_key(&tracks, &track(-200, 9, None), 42),
            Some((42, 1))
        );
        tracks[0] = track(42, 2, Some((-200, 10)));
        tracks[0].title = "After".into();
        cache.invalidate();
        assert!(cache.visible(&tracks, "before").is_empty());
        assert_eq!(cache.library_key(&tracks, &track(-200, 9, None), 42), None);
        assert_eq!(
            cache.library_key(&tracks, &track(-200, 10, None), 42),
            Some((42, 2))
        );
        assert_eq!(cache.membership.borrow().builds, 2);
        assert_eq!(cache.filter.borrow().builds, 2);
    }

    #[test]
    fn library_refresh_and_metadata_hydration_invalidate_same_length_cached_views() {
        use crate::app::{MetadataRequest, Reply, tests::search_test_app};
        use std::sync::{atomic::AtomicBool, mpsc};
        let ctx = eframe::egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut original = track(42, 1, Some((-200, 9)));
        original.title = "Before".into();
        app.library.tracks = vec![original.clone()];
        assert_eq!(
            app.track_view_cache
                .visible(&app.library.tracks, "before")
                .len(),
            1
        );
        assert_eq!(app.owned_library_key(&track(-200, 9, None)), Some((42, 1)));
        let mut fresh = original.clone();
        fresh.title = "After".into();
        fresh.release_key = None;
        fresh.subtitle = Some("Beforeversion".into());
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(Reply::Audio(
                crate::model::AudioPage {
                    count: Some(1),
                    exhausted: Some(true),
                    items: vec![fresh.clone()],
                },
                true,
                None,
            )))
            .unwrap();
        app.pending = Some(receiver);
        app.poll(&ctx);
        assert_eq!(app.library.tracks.len(), 1);
        assert!(
            app.track_view_cache
                .visible(&app.library.tracks, "before")
                .len()
                == 1
        );
        assert_eq!(app.owned_library_key(&track(-200, 9, None)), Some((42, 1)));
        assert_eq!(
            app.track_view_cache
                .visible(&app.library.tracks, "beforeversion")
                .len(),
            1
        );
        fresh.release_key = Some((-200, 10));
        fresh.subtitle = Some("Afterversion".into());
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(vec![fresh])).unwrap();
        app.metadata_request = Some(MetadataRequest {
            owner: 42,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        app.poll_metadata(&ctx);
        assert!(
            app.track_view_cache
                .visible(&app.library.tracks, "beforeversion")
                .is_empty()
        );
        assert_eq!(
            app.track_view_cache
                .visible(&app.library.tracks, "afterversion")
                .len(),
            1
        );
        assert_eq!(app.owned_library_key(&track(-200, 10, None)), None);
        assert_eq!(app.library.tracks[0].library_source_key, Some((-200, 9)));
        assert!(app.demo);
        assert!(
            app.session
                .as_ref()
                .is_some_and(|session| { session.client.is_none() && session.music.is_none() })
        );
    }

    #[test]
    fn optimistic_alias_and_removal_overrides_remain_live_with_a_warm_index() {
        let ctx = eframe::egui::Context::default();
        let mut app = crate::app::tests::search_test_app(&ctx);
        app.library.tracks = vec![track(42, 1, Some((-200, 9)))];
        let mut source = track(-300, 8, None);
        source.library_liked = Some(false);
        assert!(!app.track_is_liked(&source));
        app.like_aliases.insert(source.key(), (42, 1));
        assert!(app.track_is_liked(&source));
        app.removed_from_library.insert(source.key());
        assert!(!app.track_is_liked(&source));
        app.removed_from_library.clear();
        app.like_aliases.remove(&source.key());
        assert!(!app.track_is_liked(&source));
    }
}
