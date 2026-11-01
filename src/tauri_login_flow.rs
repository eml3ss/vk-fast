//! One native window lifetime, with an in-page cover during verification.
use reqwest::Url;

fn native_https(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ReturnRoute {
    host: &'static str,
    path: &'static str,
}

fn main_return_route(url: &Url) -> Option<ReturnRoute> {
    if !native_https(url) {
        return None;
    }
    let host = match url.host_str()? {
        "vk.ru" => "vk.ru",
        "vk.com" => "vk.com",
        _ => return None,
    };
    let path = match url.path() {
        "/" => "/",
        "/feed" => "/feed",
        "/feed/" => "/feed/",
        _ => return None,
    };
    Some(ReturnRoute { host, path })
}

/// Main website documents can be either a login form or the feed. Cover them
/// until the worker checks cookies; this never hides the native window.
pub(crate) fn post_login_route(url: &Url) -> bool {
    main_return_route(url).is_some()
}

fn interactive_vk_route(url: &Url) -> bool {
    // Any other allowed VK navigation may be an interactive challenge. Do not
    // strand it behind an earlier return while waiting on a stale HTTP check.
    native_https(url)
        && url.host_str().is_some_and(|host| {
            host == "vk.ru"
                || host.ends_with(".vk.ru")
                || host == "vk.com"
                || host.ends_with(".vk.com")
        })
}

/// Shared under a short mutex by navigation hooks and the verification worker.
/// Apply the presentation on the UI thread under this mutex. Redirects and
/// cookie checks affect page content, while native visibility changes once.
#[derive(Default)]
pub(crate) struct LoginWindowFlow {
    revision: u64,
    covered: bool,
    shown: bool,
    canceled: bool,
    current_return: Option<ReturnRoute>,
    main_page: bool,
}

impl LoginWindowFlow {
    /// Returns true when a new document needs the in-page cover. Finished
    /// events can wake the worker but do not change the presentation.
    pub(crate) fn page_event(&mut self, url: &Url, started: bool) -> bool {
        if self.canceled || !started || !interactive_vk_route(url) {
            return false;
        }
        let route = main_return_route(url);
        self.main_page = route.is_some();
        if post_login_route(url) {
            self.revision = self.revision.wrapping_add(1);
            self.covered = true;
            self.current_return = route;
            true
        } else {
            if interactive_vk_route(url) {
                self.revision = self.revision.wrapping_add(1);
                self.covered = false;
                self.current_return = None;
            }
            false
        }
    }

    /// A late Finished from another document must not release a newer return.
    /// Store only fixed host/path identity; auth query strings are never kept.
    pub(crate) fn finished_login_return(&self, url: &Url) -> bool {
        !self.canceled
            && self.current_return.is_some()
            && self.current_return == main_return_route(url)
    }

    pub(crate) fn ticket(&self) -> u64 {
        self.revision
    }

    /// Check before publishing success as well as before recovering a failure.
    pub(crate) fn is_current(&self, ticket: u64) -> bool {
        !self.canceled && ticket == self.revision
    }

    /// A cookie candidate may arrive during SPA navigation without a feed
    /// event. Cover the main website while native verification runs.
    pub(crate) fn verification_started(&mut self, ticket: u64) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        // Cookie presence alone is not proof of completed authentication.
        // Keep provider challenges visible; cover only the main website.
        self.covered = self.main_page;
        true
    }

    pub(crate) fn cover_page(&self) -> bool {
        self.covered
    }

    pub(crate) fn show_if_allowed(&mut self) -> bool {
        // Native visibility is independent of redirects and verification.
        // An in-document cover keeps the feed out of view without unmapping
        // and remapping the native window (and stealing focus) repeatedly.
        if self.canceled || self.shown {
            return false;
        }
        self.shown = true;
        true
    }

    /// Roll back a failed native Show so the next worker wake can retry it.
    pub(crate) fn show_failed(&mut self) {
        self.shown = false;
    }

    /// Failed/absent cookies are not login success. Reveal the interactive
    /// page only when this result still belongs to the latest navigation.
    pub(crate) fn verification_failed(&mut self, ticket: u64) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.covered = false;
        true
    }

    pub(crate) fn cancel(&mut self) {
        self.canceled = true;
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn url(value: &str) -> Url {
        value.parse().unwrap()
    }

    #[test]
    fn only_main_site_documents_are_covered_before_session_detection() {
        for value in [
            "https://vk.ru/",
            "https://vk.com/feed/",
            "https://vk.ru:443/feed?section=all",
        ] {
            assert!(post_login_route(&url(value)));
        }
        for value in [
            "https://vk.ru/login",
            "https://id.vk.ru/auth",
            "https://vk.ru/validate",
            "https://vk.ru/feed/challenge",
            "https://vk.ru.example.test/feed",
            "https://user@vk.ru/feed",
            "http://vk.ru/feed",
            "https://vk.ru:444/feed",
        ] {
            assert!(!post_login_route(&url(value)), "{value}");
        }
    }

    #[test]
    fn redirects_and_repeated_cookie_failures_show_the_native_window_only_once() {
        let mut flow = LoginWindowFlow::default();
        let mut native_shows = usize::from(flow.show_if_allowed());
        for _ in 0..3 {
            for page in [
                "https://vk.ru/",
                "https://id.vk.ru/auth",
                "https://login.vk.ru/",
                "https://vk.ru/",
                "https://vk.ru/feed",
            ] {
                flow.page_event(&url(page), true);
                native_shows += usize::from(flow.show_if_allowed());
                let ticket = flow.ticket();
                assert!(flow.verification_started(ticket));
                native_shows += usize::from(flow.show_if_allowed());
                assert!(flow.verification_failed(ticket));
                native_shows += usize::from(flow.show_if_allowed());
                flow.page_event(&url(page), false);
                native_shows += usize::from(flow.show_if_allowed());
            }
        }
        assert_eq!(
            native_shows, 1,
            "Redirects and retries must not remap or refocus the window"
        );
    }

    #[test]
    fn pending_return_is_covered_but_phone_challenges_stay_interactive() {
        let mut flow = LoginWindowFlow::default();
        flow.page_event(&url("https://vk.ru/"), true);
        assert!(flow.cover_page());
        assert!(flow.show_if_allowed());
        assert!(flow.verification_failed(flow.ticket()));
        assert!(
            !flow.cover_page(),
            "Anonymous initial form is revealed after cookie detection"
        );
        for page in [
            "https://id.vk.ru/auth?step=otp",
            "https://vk.ru/validate",
            "https://m.vk.ru/security/check",
        ] {
            flow.page_event(&url(page), true);
            assert!(flow.verification_started(flow.ticket()));
            assert!(
                !flow.cover_page(),
                "Partial cookies must not conceal a challenge"
            );
            assert!(!flow.show_if_allowed());
        }
        flow.page_event(&url("https://vk.ru/feed"), true);
        assert!(flow.cover_page());
        assert!(!flow.show_if_allowed());
    }

    #[test]
    fn stale_cookie_checks_and_finished_events_cannot_reveal_a_new_return() {
        let mut flow = LoginWindowFlow::default();
        flow.page_event(&url("https://vk.ru/"), true);
        let old = flow.ticket();
        flow.page_event(&url("https://vk.ru/feed"), true);
        assert!(!flow.is_current(old));
        assert!(!flow.verification_failed(old));
        assert!(!flow.verification_started(old));
        assert!(flow.cover_page());
        assert!(!flow.finished_login_return(&url("https://vk.ru/")));
        assert!(!flow.finished_login_return(&url("https://vk.com/feed")));
        assert!(flow.finished_login_return(&url("https://vk.ru/feed")));
        flow.page_event(&url("https://vk.ru/feed"), false);
        assert!(flow.cover_page());
        assert!(flow.verification_failed(flow.ticket()));
        flow.page_event(&url("https://vk.ru/feed"), false);
        assert!(!flow.cover_page());
    }

    #[test]
    fn canceled_or_untrusted_navigation_cannot_change_the_native_window() {
        let mut flow = LoginWindowFlow::default();
        flow.page_event(&url("https://vk.ru/feed"), true);
        assert!(flow.show_if_allowed());
        let current = flow.ticket();
        for foreign in [
            "https://vk.ru.example.test/",
            "http://vk.ru/login",
            "https://user@vk.ru/auth",
        ] {
            flow.page_event(&url(foreign), true);
            assert!(flow.is_current(current));
            assert!(flow.cover_page());
        }
        flow.cancel();
        assert!(!flow.is_current(current));
        assert!(!flow.verification_failed(current));
        assert!(!flow.verification_started(current));
        assert!(!flow.show_if_allowed());
    }

    #[test]
    fn a_failed_native_show_can_be_retried_without_resetting_on_navigation() {
        let mut flow = LoginWindowFlow::default();
        assert!(flow.show_if_allowed());
        flow.show_failed();
        assert!(flow.show_if_allowed());
        flow.page_event(&url("https://vk.ru/"), true);
        assert!(!flow.show_if_allowed());
    }
}
