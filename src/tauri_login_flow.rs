//! Only human verification opens the webview; ordinary forms stay in egui.
use crate::web_login_state::{Snapshot, Step};

/// Render recognized forms immediately, even before native ContentLoading.
/// Cookie verification waits for a completed navigation and no active form.
pub fn can_verify_session(
    ready: bool,
    epoch: u64,
    completed_epoch: u64,
    step: Step,
    document: &str,
) -> bool {
    ready && epoch == completed_epoch && step == Step::Loading && !document.is_empty()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Show(u16, u16),
    Resize(u16, u16),
    Hide,
}
#[derive(Default)]
pub struct Popup {
    visible: bool,
    manual: bool,
    dismissed: bool,
    absent: u8,
    size: (u16, u16),
}
impl Popup {
    pub fn resume(&mut self) {
        self.dismissed = false;
    }
    pub fn manual(&self) -> bool {
        self.manual
    }
    pub fn update(&mut self, view: &Snapshot, open: bool, dismissed: bool) -> Change {
        if dismissed {
            self.manual = false;
            self.visible = false;
            self.dismissed = true;
            self.absent = 0;
            return Change::None;
        }
        if open {
            self.manual = view.step != Step::Captcha;
            self.dismissed = false;
        } else if view.accepts_input()
            || matches!(view.step, Step::Entry | Step::Methods | Step::Approval)
        {
            // A recovered ordinary form belongs in the native dialog, even
            // when an earlier unrecognized step was opened manually.
            self.manual = false;
        }
        let requested = self.manual || (view.step == Step::Captcha && !self.dismissed);
        if !requested && self.visible {
            self.absent += 1;
            if self.absent >= 2 {
                self.visible = false;
                return Change::Hide;
            }
            return Change::None;
        }
        if !requested {
            return Change::None;
        }
        self.absent = 0;
        let size = if view.step == Step::Captcha {
            (view.width.clamp(340, 620), view.height.clamp(400, 780))
        } else {
            (500, 650)
        };
        if !self.visible || open {
            self.visible = true;
            self.size = size;
            return Change::Show(size.0, size.1);
        }
        if self.size.0.abs_diff(size.0) > 8 || self.size.1.abs_diff(size.1) > 8 {
            self.size = size;
            return Change::Resize(size.0, size.1);
        }
        Change::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_early_document_answer_is_unblocked_after_native_load_completion() {
        assert!(!can_verify_session(
            true,
            0,
            u64::MAX,
            Step::Loading,
            "new-document"
        ));
        assert!(!can_verify_session(
            true,
            1,
            u64::MAX,
            Step::Loading,
            "new-document"
        ));
        assert!(can_verify_session(
            true,
            1,
            1,
            Step::Loading,
            "new-document"
        ));
    }

    #[test]
    fn another_navigation_or_a_human_challenge_prevents_cookie_acceptance() {
        assert!(!can_verify_session(
            true,
            2,
            1,
            Step::Loading,
            "old-document"
        ));
        for step in [
            Step::Identifier,
            Step::Password,
            Step::Code,
            Step::Captcha,
            Step::Manual,
            Step::Approval,
        ] {
            assert!(!can_verify_session(true, 2, 2, step, "current-document"));
        }
        assert!(!can_verify_session(
            false,
            2,
            2,
            Step::Loading,
            "current-document"
        ));
        assert!(!can_verify_session(true, 2, 2, Step::Loading, ""));
    }
    #[test]
    fn hidden_forms_and_single_captcha_popup_preserve_human_control() {
        let mut p = Popup::default();
        let mut v = Snapshot {
            document: "page".into(),
            step: Step::Identifier,
            ..Default::default()
        };
        for _ in 0..5 {
            assert_eq!(p.update(&v, false, false), Change::None);
        }
        v.step = Step::Captcha;
        v.revision = 2;
        v.width = 350;
        v.height = 420;
        assert_eq!(p.update(&v, false, false), Change::Show(350, 420));
        for _ in 0..5 {
            assert_eq!(p.update(&v, false, false), Change::None);
        }
        p.update(&v, false, true);
        assert_eq!(p.update(&v, false, false), Change::None);
        assert_eq!(p.update(&v, true, false), Change::Show(350, 420));
        v.step = Step::Code;
        v.revision = 3;
        assert_eq!(p.update(&v, false, false), Change::None);
        assert_eq!(p.update(&v, false, false), Change::Hide);
        assert_eq!(p.update(&v, false, false), Change::None);
    }
    #[test]
    fn an_opened_fallback_returns_to_native_when_a_password_form_is_recognized() {
        let mut p = Popup::default();
        let mut v = Snapshot {
            step: Step::Manual,
            ..Default::default()
        };
        assert_eq!(p.update(&v, true, false), Change::Show(500, 650));
        v.step = Step::Password;
        assert_eq!(p.update(&v, false, false), Change::None);
        assert!(!p.manual());
        assert_eq!(p.update(&v, false, false), Change::Hide);
        assert_eq!(p.update(&v, false, false), Change::None);
    }
}
