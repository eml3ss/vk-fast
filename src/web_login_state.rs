//! Bounded native-login state. Commands contain transient user input, never Debug.
use crate::repaint_target::RepaintTarget;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroize;

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
    #[serde(default)]
    pub code_length: u8,
    #[serde(default)]
    pub password_option: bool,
    #[serde(default)]
    pub other_option: bool,
    #[serde(default)]
    pub back_option: bool,
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
            state.pending = false;
            drop(state);
            if changed {
                self.repaint.request_repaint();
            }
            true
        } else {
            false
        }
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
