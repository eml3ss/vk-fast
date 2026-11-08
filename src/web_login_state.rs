//! Bounded native-login state. Commands contain transient user input, never Debug.
use crate::repaint_target::RepaintTarget;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    #[default]
    Loading,
    Entry,
    Identifier,
    Password,
    Code,
    Approval,
    Methods,
    Captcha,
    Manual,
    Finishing,
}

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub document: String,
    pub revision: u64,
    pub step: Step,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub error: bool,
    /// VK's own short message for a rejected form, shown instead of the
    /// generic hint. Never logged.
    #[serde(default)]
    pub error_text: String,
    #[serde(default)]
    pub code_length: u8,
    #[serde(default)]
    pub password_option: bool,
    #[serde(default)]
    pub other_option: bool,
    #[serde(default)]
    pub back_option: bool,
    #[serde(default)]
    pub open_page_available: bool,
    #[serde(default)]
    pub retry_available: bool,
    #[serde(default)]
    pub width: u16,
    #[serde(default)]
    pub height: u16,
}
impl Snapshot {
    pub fn valid(&self) -> bool {
        self.document.len() <= 80
            && self
                .document
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && (self.step != Step::Code || (4..=16).contains(&self.code_length))
            && (!self.document.is_empty()
                || matches!(self.step, Step::Loading | Step::Manual | Step::Finishing))
            && self.code_length <= 16
            && self.error_text.chars().count() <= 160
            && !self.error_text.chars().any(char::is_control)
            && self.width <= 1200
            && self.height <= 1200
    }
    pub fn accepts_input(&self) -> bool {
        matches!(self.step, Step::Identifier | Step::Password | Step::Code)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Submit,
    Password,
    Other,
    Back,
    OpenPage,
    Reload,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub document: String,
    pub revision: u64,
    pub action: Action,
    pub value: String,
}
impl Drop for Command {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
impl Command {
    pub fn valid(&self) -> bool {
        self.document.len() <= 80
            && self
                .document
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && self.value.len() <= 2048
            && !self.value.contains(['\0', '\r', '\n'])
            && (self.action == Action::Submit || self.value.is_empty())
    }
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    command: Option<Command>,
    pending: bool,
    credentials: Option<Credentials>,
}

/// One explicit sign-in can span identifier, verification and password forms.
/// Keep the password only until that attempt reaches its password form.
struct Credentials {
    password: Zeroizing<String>,
    requested_password: bool,
}

impl State {
    fn continue_credentials(&mut self) {
        if self.snapshot.error
            || matches!(
                self.snapshot.step,
                Step::Entry | Step::Manual | Step::Finishing
            )
        {
            self.credentials = None;
            return;
        }
        if self.pending || self.snapshot.busy || self.command.is_some() {
            return;
        }
        let Some(credentials) = self.credentials.as_mut() else {
            return;
        };
        let (action, value) = if self.snapshot.step == Step::Password {
            let value = std::mem::take(&mut *credentials.password);
            self.credentials = None;
            (Action::Submit, value)
        } else if matches!(
            self.snapshot.step,
            Step::Code | Step::Approval | Step::Methods
        ) && self.snapshot.password_option
            && !credentials.requested_password
        {
            credentials.requested_password = true;
            (Action::Password, String::new())
        } else {
            return;
        };
        self.command = Some(Command {
            document: self.snapshot.document.clone(),
            revision: self.snapshot.revision,
            action,
            value,
        });
        self.pending = true;
    }
}

#[derive(Clone, Default)]
pub struct Control {
    canceled: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    repaint: RepaintTarget,
}
impl Control {
    pub fn bind(&self, ctx: &eframe::egui::Context) {
        self.repaint.set_context(ctx);
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.command = None;
            state.pending = false;
            state.credentials = None;
        }
        self.repaint.request_repaint();
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .map(|s| {
                let mut v = s.snapshot.clone();
                v.busy |= s.pending;
                v
            })
            .unwrap_or_default()
    }
    pub fn publish(&self, snapshot: Snapshot) -> bool {
        if !snapshot.valid() || self.is_canceled() {
            return false;
        }
        if let Ok(mut state) = self.state.lock() {
            let changed = state.snapshot != snapshot || state.pending;
            state.snapshot = snapshot;
            state.pending = state.command.is_some();
            state.continue_credentials();
            drop(state);
            if changed {
                self.repaint.request_repaint();
            }
            true
        } else {
            false
        }
    }
    /// Both fields are collected together; the website still owns each step.
    pub fn send_credentials(&self, identifier: String, password: String) -> bool {
        let mut identifier = Zeroizing::new(identifier);
        let mut password = Zeroizing::new(password);
        if self.is_canceled()
            || password.is_empty()
            || password.len() > 2048
            || password.contains(['\0', '\r', '\n'])
            || identifier.trim().is_empty()
        {
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.snapshot.step != Step::Identifier
            || state.snapshot.busy
            || state.pending
            || state.command.is_some()
        {
            return false;
        }
        let command = Command {
            document: state.snapshot.document.clone(),
            revision: state.snapshot.revision,
            action: Action::Submit,
            value: std::mem::take(&mut *identifier),
        };
        if !command.valid() {
            return false;
        }
        state.credentials = Some(Credentials {
            password: Zeroizing::new(std::mem::take(&mut *password)),
            requested_password: false,
        });
        state.command = Some(command);
        state.pending = true;
        drop(state);
        self.repaint.request_repaint();
        true
    }
    pub fn send(&self, action: Action, mut value: String) -> bool {
        if self.is_canceled() {
            value.zeroize();
            return false;
        }
        let Ok(mut state) = self.state.lock() else {
            value.zeroize();
            return false;
        };
        let snapshot = &state.snapshot;
        let allowed = match action {
            Action::Submit => snapshot.accepts_input() && !value.is_empty(),
            Action::Password => snapshot.password_option,
            Action::Other => snapshot.other_option,
            Action::Back => snapshot.back_option,
            Action::OpenPage => true,
            Action::Reload => snapshot.step == Step::Loading && snapshot.retry_available,
        };
        if !allowed
            || state.command.is_some()
            || (state.pending || (action != Action::OpenPage && snapshot.busy))
        {
            value.zeroize();
            return false;
        }
        let command = Command {
            document: snapshot.document.clone(),
            revision: snapshot.revision,
            action,
            value,
        };
        if !command.valid() {
            return false;
        }
        if matches!(action, Action::Back | Action::Other | Action::Reload)
            || (action == Action::OpenPage && snapshot.step != Step::Captcha)
            || (action == Action::Submit && snapshot.step != Step::Code)
        {
            state.credentials = None;
        }
        state.command = Some(command);
        state.pending = true;
        drop(state);
        self.repaint.request_repaint();
        true
    }
    pub fn take_command(&self) -> Option<Command> {
        self.state.lock().ok()?.command.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view(step: Step, revision: u64) -> Snapshot {
        Snapshot {
            document: format!("synthetic-{revision}"),
            revision,
            step,
            code_length: if step == Step::Code { 6 } else { 0 },
            ..Default::default()
        }
    }

    #[test]
    fn combined_credentials_follow_verified_forms_without_another_ui_frame() {
        let c = Control::default();
        c.publish(view(Step::Identifier, 1));
        assert!(c.send_credentials("user@example.test".into(), " synthetic secret ".into()));
        assert!(!c.send_credentials("other@example.test".into(), "duplicate".into()));
        let identifier = c.take_command().unwrap();
        assert_eq!(identifier.value, "user@example.test");
        assert_eq!(identifier.document, "synthetic-1");

        c.publish(view(Step::Captcha, 2));
        assert!(c.take_command().is_none());
        assert!(c.send(Action::OpenPage, String::new()));
        assert_eq!(c.take_command().unwrap().action, Action::OpenPage);
        c.publish(view(Step::Loading, 3));
        assert!(c.take_command().is_none());
        c.publish(Snapshot {
            busy: true,
            ..view(Step::Password, 4)
        });
        assert!(c.take_command().is_none());
        c.publish(view(Step::Password, 4));
        let password = c.take_command().unwrap();
        assert_eq!(password.value, " synthetic secret ");
        assert_eq!(password.document, "synthetic-4");
        assert_eq!(password.revision, 4);
        assert_eq!(password.action, Action::Submit);
        assert!(c.state.lock().unwrap().credentials.is_none());
        c.publish(view(Step::Password, 4));
        assert!(
            c.take_command().is_none(),
            "Never repeat a password submission"
        );
    }

    #[test]
    fn combined_sign_in_selects_the_offered_password_method_only_once() {
        let c = Control::default();
        c.publish(view(Step::Identifier, 1));
        c.send_credentials("user@example.test".into(), "synthetic".into());
        c.take_command().unwrap();
        let methods = Snapshot {
            password_option: true,
            ..view(Step::Code, 2)
        };
        c.publish(methods.clone());
        let command = c.take_command().unwrap();
        assert_eq!(command.action, Action::Password);
        assert!(command.value.is_empty());
        c.publish(methods);
        assert!(c.take_command().is_none());

        // If VK requires a code first, only the user's explicit code is sent.
        assert!(c.send(Action::Submit, "012345".into()));
        assert_eq!(c.take_command().unwrap().value, "012345");
        c.publish(view(Step::Approval, 3));
        assert!(c.take_command().is_none());
        c.publish(view(Step::Password, 4));
        assert_eq!(c.take_command().unwrap().value, "synthetic");
    }

    #[test]
    fn slow_page_loading_does_not_discard_the_password_when_manual_help_is_offered() {
        let c = Control::default();
        c.publish(view(Step::Identifier, 1));
        assert!(c.send_credentials("user@example.test".into(), "synthetic".into()));
        c.take_command().unwrap();
        for _ in 0..60 {
            c.publish(Snapshot {
                open_page_available: true,
                ..view(Step::Loading, 2)
            });
            assert!(c.take_command().is_none());
        }
        c.publish(view(Step::Password, 3));
        assert_eq!(c.take_command().unwrap().value, "synthetic");
        assert!(c.state.lock().unwrap().credentials.is_none());
    }

    #[test]
    fn reload_requires_an_explicit_stalled_loading_step_and_discards_saved_credentials() {
        let c = Control::default();
        c.publish(view(Step::Identifier, 1));
        assert!(!c.send(Action::Reload, String::new()));
        assert!(c.send_credentials("user@example.test".into(), "synthetic".into()));
        c.take_command().unwrap();
        c.publish(Snapshot {
            retry_available: true,
            ..view(Step::Loading, 2)
        });
        assert!(c.send(Action::Reload, String::new()));
        assert!(c.state.lock().unwrap().credentials.is_none());
        let command = c.take_command().unwrap();
        assert_eq!(command.action, Action::Reload);
        assert_eq!(command.revision, 2);
        c.publish(Snapshot {
            retry_available: true,
            ..view(Step::Code, 3)
        });
        assert!(!c.send(Action::Reload, String::new()));
    }

    #[test]
    fn abandoned_or_failed_attempts_cannot_reuse_a_password() {
        for snapshot in [
            Snapshot {
                error: true,
                ..view(Step::Identifier, 2)
            },
            view(Step::Manual, 2),
            view(Step::Finishing, 2),
            view(Step::Entry, 2),
        ] {
            let c = Control::default();
            c.publish(view(Step::Identifier, 1));
            c.send_credentials("user@example.test".into(), "synthetic".into());
            c.take_command().unwrap();
            c.publish(snapshot);
            assert!(c.state.lock().unwrap().credentials.is_none());
            c.publish(view(Step::Password, 5));
            assert!(c.take_command().is_none());
        }
        for action in [
            None,
            Some(Action::Back),
            Some(Action::Other),
            Some(Action::OpenPage),
        ] {
            let c = Control::default();
            c.publish(view(Step::Identifier, 1));
            c.send_credentials("user@example.test".into(), "synthetic".into());
            c.take_command().unwrap();
            c.publish(Snapshot {
                back_option: true,
                other_option: true,
                ..view(Step::Approval, 2)
            });
            if let Some(action) = action {
                assert!(c.send(action, String::new()));
                c.take_command().unwrap();
            } else {
                c.cancel();
            }
            assert!(c.state.lock().unwrap().credentials.is_none());
            c.publish(view(Step::Password, 5));
            assert!(c.take_command().is_none());
        }
    }

    #[test]
    fn vk_error_messages_are_short_single_line_text() {
        let mut snapshot = view(Step::Password, 1);
        snapshot.error = true;
        snapshot.error_text = "Неверный пароль".into();
        assert!(snapshot.valid());
        snapshot.error_text = "x".repeat(161);
        assert!(!snapshot.valid());
        snapshot.error_text = "first\nsecond".into();
        assert!(!snapshot.valid());
    }

    #[test]
    fn combined_credentials_require_both_fields_and_an_identifier_form() {
        let c = Control::default();
        c.publish(view(Step::Identifier, 1));
        for (identifier, password) in [
            ("", "synthetic".into()),
            ("   ", "synthetic".into()),
            ("user@example.test", String::new()),
            ("user@example.test", "x".repeat(2049)),
            ("user@example.test", "secret\n".into()),
        ] {
            assert!(!c.send_credentials(identifier.into(), password));
            assert!(c.take_command().is_none());
            assert!(c.state.lock().unwrap().credentials.is_none());
        }
        c.publish(view(Step::Captcha, 2));
        assert!(!c.send_credentials("user@example.test".into(), "synthetic".into()));
    }

    #[test]
    fn commands_are_bounded_step_bound_and_cannot_submit_captcha_or_stale_forms() {
        let c = Control::default();
        c.publish(Snapshot {
            document: "page-1".into(),
            revision: 2,
            step: Step::Identifier,
            ..Default::default()
        });
        assert!(c.send(Action::Submit, "user@example.test".into()));
        assert!(!c.send(Action::Submit, "duplicate".into()));
        let cmd = c.take_command().unwrap();
        assert_eq!(cmd.revision, 2);
        assert!(cmd.valid());
        c.publish(Snapshot {
            document: "page-2".into(),
            step: Step::Captcha,
            ..Default::default()
        });
        assert!(!c.send(Action::Submit, "synthetic".into()));
        assert!(!c.send(Action::Password, String::new()));
        assert!(c.send(Action::OpenPage, String::new()));
        c.cancel();
        assert!(c.take_command().is_none());
        assert!(!c.send(Action::OpenPage, String::new()));
    }
    #[test]
    fn protocol_rejects_unbounded_metadata_and_non_submit_payloads() {
        assert!(
            !Snapshot {
                document: "x".repeat(81),
                ..Default::default()
            }
            .valid()
        );
        assert!(
            !Command {
                document: "x".into(),
                revision: 0,
                action: Action::OpenPage,
                value: "secret".into()
            }
            .valid()
        );
        assert!(
            !Command {
                document: "x".into(),
                revision: 0,
                action: Action::Submit,
                value: "x".repeat(2049)
            }
            .valid()
        );
    }
}
