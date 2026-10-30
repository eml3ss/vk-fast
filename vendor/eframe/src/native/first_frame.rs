//! Reveal an automatically hidden root only after its first successful present.

pub(super) struct FirstFrame {
    reveal_pending: bool,
}

impl FirstFrame {
    pub(super) fn new(requested_visible: Option<bool>) -> Self {
        Self {
            reveal_pending: requested_visible != Some(false),
        }
    }

    pub(super) fn present<E>(
        &mut self,
        present: impl FnOnce() -> Result<(), E>,
        reveal: impl FnOnce(),
    ) -> Result<(), E> {
        present()?;
        if core::mem::take(&mut self.reveal_pending) {
            reveal();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};

    #[test]
    fn first_frame_is_presented_before_reveal_and_later_hidden_frames_stay_hidden() {
        let events = RefCell::new(vec!["paint"]);
        let visible = Cell::new(false);
        let mut first_frame = FirstFrame::new(None);
        first_frame
            .present(
                || {
                    events.borrow_mut().push("present");
                    Ok::<_, ()>(())
                },
                || {
                    events.borrow_mut().push("show");
                    visible.set(true);
                },
            )
            .expect("synthetic presentation succeeds");
        assert_eq!(*events.borrow(), ["paint", "present", "show"]);
        assert!(visible.get());

        visible.set(false); // The application later hides to the tray.
        first_frame
            .present(|| Ok::<_, ()>(()), || visible.set(true))
            .expect("synthetic presentation succeeds");
        assert!(!visible.get(), "a later present must not undo hiding");
    }

    #[test]
    fn failed_present_cannot_expose_an_uninitialized_surface_and_can_retry() {
        let shown = Cell::new(0);
        let mut first_frame = FirstFrame::new(Some(true));
        assert!(
            first_frame
                .present(|| Err("surface unavailable"), || shown.set(shown.get() + 1))
                .is_err()
        );
        assert_eq!(shown.get(), 0);
        first_frame
            .present(|| Ok::<_, ()>(()), || shown.set(shown.get() + 1))
            .expect("synthetic presentation succeeds");
        assert_eq!(shown.get(), 1);
    }

    #[test]
    fn explicit_initial_visibility_is_preserved_and_recreated_windows_get_a_new_gate() {
        let shown = Cell::new(0);
        let mut intentionally_hidden = FirstFrame::new(Some(false));
        for _ in 0..3 {
            intentionally_hidden
                .present(|| Ok::<_, ()>(()), || shown.set(shown.get() + 1))
                .expect("synthetic presentation succeeds");
        }
        assert_eq!(shown.get(), 0);

        let mut recreated = FirstFrame::new(None);
        recreated
            .present(|| Ok::<_, ()>(()), || shown.set(shown.get() + 1))
            .expect("synthetic presentation succeeds");
        assert_eq!(shown.get(), 1);
    }
}
