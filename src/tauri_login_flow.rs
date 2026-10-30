//! Visibility coordination only; cookies and profile verification stay native.
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

/// Feed navigation is a possible login return. A root navigation needs prior
/// provider history, because an anonymous root is also the initial login form.
pub(crate) fn post_login_route(url: &Url) -> bool {
    main_return_route(url).is_some_and(|route| route.path != "/")
}

fn provider_route(url: &Url) -> bool {
    native_https(url)
        && matches!(
            url.host_str(),
            Some("id.vk.ru" | "id.vk.com" | "login.vk.ru" | "login.vk.com")
        )
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
/// Native show/hide calls must be serialized under that same mutex (or checked
/// again on the UI thread), so a delayed worker Show cannot undo a newer Hide.
#[derive(Default)]
pub(crate) struct LoginWindowFlow {
    revision: u64,
    hidden_for_login: bool,
    shown: bool,
    canceled: bool,
    provider_visited: bool,
    current_return: Option<ReturnRoute>,
}

impl LoginWindowFlow {
    /// Returns true when a new sign-in return should be hidden. Finished
    /// events can wake the worker, but must never re-hide a failed verification.
    pub(crate) fn page_event(&mut self, url: &Url, started: bool) -> bool {
        if self.canceled || !started {
            return false;
        }
        let route = main_return_route(url);
        if post_login_route(url)
            || (self.provider_visited && route.is_some_and(|route| route.path == "/"))
        {
            self.revision = self.revision.wrapping_add(1);
            self.hidden_for_login = true;
            self.shown = false;
            self.current_return = route;
            self.provider_visited = false;
            true
        } else {
            if provider_route(url) {
                self.provider_visited = true;
            }
            if interactive_vk_route(url) {
                self.revision = self.revision.wrapping_add(1);
                self.hidden_for_login = false;
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
    /// event. Hide while native verification runs and reject queued old Shows.
    pub(crate) fn verification_started(&mut self, ticket: u64) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.hidden_for_login = true;
        self.shown = false;
        true
    }

    pub(crate) fn show_if_allowed(&mut self) -> bool {
        if self.canceled || self.hidden_for_login || self.shown {
            return false;
        }
        self.shown = true;
        true
    }

    /// Roll back a failed native Show so the next worker wake can retry it.
    pub(crate) fn show_failed(&mut self) {
        self.shown = false;
    }

    /// Failed/absent cookies are not login success. Restore the interactive
    /// window only when this result still belongs to the latest navigation.
    pub(crate) fn verification_failed(&mut self, ticket: u64) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.hidden_for_login = false;
        self.show_if_allowed()
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
    fn only_exact_main_vk_feed_returns_are_hidden_early() {
        for value in [
            "https://vk.ru/feed",
            "https://vk.com/feed/",
            "https://vk.ru:443/feed?section=all#latest",
        ] {
            assert!(post_login_route(&url(value)), "Not recognized: {value}");
        }
        for value in [
            "https://vk.ru/",
            "https://vk.ru/login?redirect=feed",
            "https://vk.ru/auth",
            "https://vk.ru/join",
            "https://vk.ru/restore",
            "https://vk.ru/captcha",
            "https://id.vk.ru/feed",
            "https://login.vk.com/feed",
            "https://vk.ru/feed-elsewhere",
            "https://vk.ru/feed/challenge",
            "https://vk.ru.example.test/feed",
            "https://user@vk.ru/feed",
            "https://vk.ru:444/feed",
            "http://vk.ru/feed",
            "file:///feed",
        ] {
            assert!(!post_login_route(&url(value)), "Unexpected hide: {value}");
        }
    }

    #[test]
    fn an_old_cookie_failure_cannot_reshow_a_window_hidden_by_a_new_navigation() {
        let mut flow = LoginWindowFlow::default();
        assert!(flow.show_if_allowed());
        assert!(!flow.show_if_allowed());
        let stale_cookie_check = flow.ticket();
        assert!(flow.verification_started(stale_cookie_check));
        assert!(!flow.show_if_allowed());
        assert!(flow.page_event(&url("https://vk.ru/feed"), true));
        assert!(!flow.verification_started(stale_cookie_check));
        assert!(!flow.verification_failed(stale_cookie_check));
        assert!(!flow.show_if_allowed());
        assert!(!flow.is_current(stale_cookie_check));
        let current = flow.ticket();
        assert!(flow.is_current(current));
        assert!(flow.verification_failed(current));
        assert!(!flow.show_if_allowed());
        assert!(!flow.page_event(&url("https://id.vk.ru/auth?step=challenge"), true));
        assert!(!flow.is_current(current));
    }

    #[test]
    fn finished_signals_never_rehide_a_failed_check_and_new_navigation_invalidates_it() {
        let mut flow = LoginWindowFlow::default();
        let feed = url("https://vk.ru/feed");
        assert!(flow.page_event(&feed, true));
        let started = flow.ticket();
        assert!(!flow.page_event(&feed, false));
        assert!(flow.is_current(started));
        assert!(flow.verification_failed(started));
        assert!(!flow.page_event(&feed, false));
        assert!(!flow.show_if_allowed(), "Finished must not undo recovery");
        assert!(flow.page_event(&feed, true));
        assert!(!flow.is_current(started));
        assert!(!flow.verification_failed(started));
        assert!(!flow.show_if_allowed());
        assert!(flow.verification_failed(flow.ticket()));
        flow.show_failed();
        assert!(
            flow.show_if_allowed(),
            "A failed native Show can be retried"
        );
    }

    #[test]
    fn cancellation_blocks_late_success_failures_and_visibility_changes() {
        let mut flow = LoginWindowFlow::default();
        assert!(flow.page_event(&url("https://vk.ru/feed"), true));
        let pending = flow.ticket();
        flow.cancel();
        assert!(!flow.is_current(pending));
        assert!(!flow.verification_failed(pending));
        assert!(!flow.show_if_allowed());
        assert!(!flow.page_event(&url("https://vk.ru/feed"), true));
    }

    #[test]
    fn returning_to_a_real_auth_page_restores_visibility_and_invalidates_old_checks() {
        for auth in [
            "https://vk.ru/",
            "https://vk.ru/login",
            "https://vk.com/login.php",
            "https://vk.ru/captcha",
            "https://vk.ru/authcheck/verify",
            "https://id.vk.ru/auth?step=challenge",
            "https://vk.ru/validate",
            "https://m.vk.ru/security/new-checkpoint",
        ] {
            let mut flow = LoginWindowFlow::default();
            assert!(flow.page_event(&url("https://vk.ru/feed"), true));
            let old = flow.ticket();
            assert!(!flow.page_event(&url(auth), true));
            assert!(
                flow.show_if_allowed(),
                "Authentication must remain usable: {auth}"
            );
            assert!(!flow.is_current(old));
            assert!(!flow.verification_failed(old));
        }
    }

    #[test]
    fn root_is_hidden_only_after_a_real_provider_visit_and_finished_matches_the_current_return() {
        let root = url("https://vk.ru/");
        let feed = url("https://vk.ru/feed");
        let mut flow = LoginWindowFlow::default();
        assert!(!flow.page_event(&root, true));
        assert!(
            flow.show_if_allowed(),
            "The anonymous initial form must be visible"
        );
        assert!(!flow.finished_login_return(&root));
        assert!(!flow.page_event(&url("https://id.vk.ru/auth?step=login"), true));
        assert!(
            flow.page_event(&root, true),
            "Hide the official provider return before paint"
        );
        assert!(!flow.show_if_allowed());
        let returned = flow.ticket();
        assert!(!flow.page_event(&root, false));
        assert!(flow.finished_login_return(&root));
        assert!(
            flow.verification_failed(returned),
            "No valid session must restore the form"
        );
        assert!(!flow.page_event(&root, false));
        assert!(
            !flow.show_if_allowed(),
            "Finished must not hide or re-show the recovered form"
        );

        assert!(flow.page_event(&feed, true));
        assert!(
            !flow.finished_login_return(&root),
            "An old root Finished cannot complete the feed"
        );
        assert!(!flow.finished_login_return(&url("https://vk.com/feed")));
        assert!(!flow.finished_login_return(&url("https://vk.ru/feed/")));
        assert!(flow.finished_login_return(&feed));
        assert!(
            !flow.page_event(&root, true),
            "Provider history is consumed, not permanent"
        );
        assert!(flow.show_if_allowed());

        let mut flow = LoginWindowFlow::default();
        assert!(!flow.page_event(&url("https://id.vk.ru.example.test/auth"), true));
        assert!(!flow.page_event(&root, true));
        assert!(flow.show_if_allowed());
    }
}
