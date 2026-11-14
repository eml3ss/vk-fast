//! The account's recent music searches, newest first, shown above VK's
//! popular searches. Kept in the private state directory beside the recent
//! tracks, never logged, and deleted when the user turns history off.
use std::{
    io,
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

use serde::{Deserialize, Serialize};

use crate::library_cache::{load_private_bytes, save_private_bytes};

/// Searches kept and shown.
pub const MAX_ENTRIES: usize = 10;
const MAX_QUERY_BYTES: usize = 512;
const MAX_BYTES: u64 = 64 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Stored {
    format_version: u32,
    account_id: i64,
    queries: Vec<String>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid search history")
}

fn valid(query: &str) -> bool {
    !query.trim().is_empty()
        && query.len() <= MAX_QUERY_BYTES
        && !query.chars().any(char::is_control)
}

/// The same search typed again: case and surrounding spaces do not count.
fn same(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

/// `queries` with `query` moved (or added) to the front as typed now.
pub fn remember(queries: &mut Vec<String>, query: &str) {
    let query = query.trim();
    if !valid(query) {
        return;
    }
    queries.retain(|old| !same(old, query));
    queries.insert(0, query.to_owned());
    queries.truncate(MAX_ENTRIES);
}

/// `queries` without `query`.
pub fn forget(queries: &mut Vec<String>, query: &str) {
    queries.retain(|old| !same(old, query));
}

fn load_at(path: &Path, account_id: i64) -> io::Result<Vec<String>> {
    let Some(bytes) = load_private_bytes(path, MAX_BYTES)? else {
        return Ok(Vec::new());
    };
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if stored.format_version != FORMAT_VERSION || stored.account_id != account_id {
        return Err(invalid());
    }
    let mut queries = Vec::new();
    // Oldest first, so each newer search moves to the front as when typed.
    for query in stored.queries.iter().take(MAX_ENTRIES).rev() {
        remember(&mut queries, query);
    }
    Ok(queries)
}

fn save_at(path: &Path, account_id: i64, queries: &[String]) -> io::Result<()> {
    if queries.is_empty() {
        return remove_at(path);
    }
    let stored = Stored {
        format_version: FORMAT_VERSION,
        account_id,
        queries: queries
            .iter()
            .filter(|query| valid(query))
            .take(MAX_ENTRIES)
            .cloned()
            .collect(),
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| invalid())?;
    save_private_bytes(path, &bytes, MAX_BYTES)
}

fn remove_at(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn path(account_id: i64) -> io::Result<std::path::PathBuf> {
    crate::recent_cache::account_state_path("search-history", account_id)
}

/// Missing history is an empty list. Call off the UI thread.
pub fn load(account_id: i64) -> io::Result<Vec<String>> {
    load_at(&path(account_id)?, account_id)
}

/// Replaces the account's history; an empty one deletes the file.
fn save(account_id: i64, queries: &[String]) -> io::Result<()> {
    save_at(&path(account_id)?, account_id, queries)
}

type Pending = (i64, Vec<String>);

/// Writes the latest history in the background; closing the app flushes it.
#[derive(Default)]
pub struct Writer {
    latest: Arc<Mutex<Option<Pending>>>,
    wake: Option<SyncSender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Writer {
    fn start() -> io::Result<Self> {
        let latest: Arc<Mutex<Option<Pending>>> = Arc::new(Mutex::new(None));
        let pending = latest.clone();
        let (wake, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("vk-search-history".into())
            .spawn(move || {
                while receive.recv().is_ok() {
                    let next = pending.lock().ok().and_then(|mut next| next.take());
                    if let Some((account, queries)) = next
                        && save(account, &queries).is_err()
                    {
                        crate::diagnostics::event("search", "history_save_failed");
                    }
                }
            })?;
        Ok(Self {
            latest,
            wake: Some(wake),
            worker: Some(worker),
        })
    }

    /// Saves `queries` as the account's history; empty deletes it.
    pub fn save(&mut self, account_id: i64, queries: &[String]) {
        if cfg!(test) {
            return;
        }
        if self.wake.is_none() {
            match Self::start() {
                Ok(writer) => *self = writer,
                Err(_) => {
                    crate::diagnostics::event("search", "history_writer_unavailable");
                    return;
                }
            }
        }
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some((account_id, queries.to_vec()));
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.wake.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_moves_to_the_front_once_and_the_list_stays_short() {
        let mut queries = Vec::new();
        for query in ["first", "second", "  First  ", "", "bad\nquery"] {
            remember(&mut queries, query);
        }
        assert_eq!(queries, ["First", "second"]);
        for index in 0..20 {
            remember(&mut queries, &format!("query {index}"));
        }
        assert_eq!(queries.len(), MAX_ENTRIES);
        assert_eq!(queries[0], "query 19");
        forget(&mut queries, "QUERY 19");
        assert_eq!(queries[0], "query 18");
    }

    #[test]
    fn history_is_saved_per_account_and_an_empty_one_removes_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("search-history").join("7.json");
        assert!(load_at(&path, 7).unwrap().is_empty());
        let queries = vec!["newest".to_owned(), "older".to_owned()];
        save_at(&path, 7, &queries).unwrap();
        assert_eq!(load_at(&path, 7).unwrap(), queries);
        assert!(
            load_at(&path, 8).is_err(),
            "another account's file is refused"
        );
        save_at(&path, 7, &[]).unwrap();
        assert!(!path.exists());
        assert!(load_at(&path, 7).unwrap().is_empty());
    }
}
