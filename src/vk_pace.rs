//! Keeps the client's requests to VK at a person's pace. A browser tab
//! never fires a dozen API calls in one second, and VK's own limit for a
//! user is three a second; bursts like that look like a bot and can get
//! the account frozen. Every request to VK waits here for its turn:
//!
//! - at most `PER_SECOND` requests start within any second;
//! - background fills (album names, release titles, dates) go one at a
//!   time, at least `BACKGROUND_GAP` apart, and only while nothing the
//!   user is waiting for has just gone out.
//!
//! Waiting happens on the worker threads that send requests, never on the
//! UI thread.
use std::{
    cell::Cell,
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Requests that may start within one second.
const PER_SECOND: usize = 3;
const WINDOW: Duration = Duration::from_secs(1);
/// Between two background requests.
const BACKGROUND_GAP: Duration = Duration::from_millis(1200);
/// A background request waits this long after anything else.
const BACKGROUND_YIELD: Duration = Duration::from_millis(600);

#[derive(Default)]
struct Pacer {
    /// Starts within the last second, oldest first.
    recent: VecDeque<Instant>,
    last_foreground: Option<Instant>,
    last_background: Option<Instant>,
}

impl Pacer {
    /// When a request of this kind may start, at the earliest.
    fn earliest(&mut self, now: Instant, background: bool) -> Instant {
        while self
            .recent
            .front()
            .is_some_and(|start| now.duration_since(*start) >= WINDOW)
        {
            self.recent.pop_front();
        }
        // Background requests leave one slot of the second free.
        let limit = if background {
            PER_SECOND - 1
        } else {
            PER_SECOND
        };
        let mut at = if self.recent.len() >= limit {
            self.recent[self.recent.len() - limit] + WINDOW
        } else {
            now
        };
        if background {
            if let Some(last) = self.last_background {
                at = at.max(last + BACKGROUND_GAP);
            }
            if let Some(last) = self.last_foreground {
                at = at.max(last + BACKGROUND_YIELD);
            }
        }
        at.max(now)
    }

    fn start(&mut self, now: Instant, background: bool) {
        self.recent.push_back(now);
        if background {
            self.last_background = Some(now);
        } else {
            self.last_foreground = Some(now);
        }
    }
}

static PACER: Mutex<Option<Pacer>> = Mutex::new(None);

thread_local! {
    static BACKGROUND: Cell<bool> = const { Cell::new(false) };
}

/// Runs `work` with its requests to VK counted as a background fill.
pub fn background<R>(work: impl FnOnce() -> R) -> R {
    let before = BACKGROUND.with(|flag| flag.replace(true));
    let result = work();
    BACKGROUND.with(|flag| flag.set(before));
    result
}

/// Waits until a request to VK may start, then counts it.
pub fn wait() {
    // Tests talk to local fixtures, not VK.
    if cfg!(test) {
        return;
    }
    let background = BACKGROUND.with(Cell::get);
    let mut waited = Duration::ZERO;
    loop {
        let now = Instant::now();
        let at = {
            let Ok(mut guard) = PACER.lock() else { return };
            let pacer = guard.get_or_insert_with(Pacer::default);
            let at = pacer.earliest(now, background);
            if at <= now {
                pacer.start(now, background);
                if waited >= Duration::from_millis(200) {
                    crate::diagnostics::event(
                        "vk-pace",
                        &format!(
                            "{} request waited ms={}",
                            if background { "background" } else { "request" },
                            waited.as_millis()
                        ),
                    );
                }
                return;
            }
            at
        };
        let pause = at - now;
        std::thread::sleep(pause);
        waited += pause;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_a_second_and_background_fills_one_at_a_time_after_the_rest() {
        let mut pacer = Pacer::default();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // Three go at once; the fourth waits for the first to be a second old.
        for _ in 0..3 {
            assert_eq!(pacer.earliest(t0, false), t0);
            pacer.start(t0, false);
        }
        assert_eq!(pacer.earliest(ms(10), false), ms(1000));
        pacer.start(ms(1000), false);
        // A background fill also waits for the user's requests to settle.
        assert!(pacer.earliest(ms(1000), true) >= ms(1600));
        let at = pacer.earliest(ms(1700), true);
        pacer.start(at, true);
        // The next one only after the gap, even with the second free.
        assert_eq!(pacer.earliest(at, true), at + BACKGROUND_GAP);
        // A user's request does not wait for background fills.
        assert_eq!(
            pacer.earliest(at + Duration::from_millis(5), false),
            at + Duration::from_millis(5)
        );
    }

    #[test]
    fn background_marks_only_the_work_it_wraps() {
        assert!(!BACKGROUND.with(Cell::get));
        let inside = background(|| BACKGROUND.with(Cell::get));
        assert!(inside);
        assert!(!BACKGROUND.with(Cell::get));
    }
}
