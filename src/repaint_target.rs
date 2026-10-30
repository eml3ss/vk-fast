//! Workers wake whichever native window currently hosts the resident app.
//! Keeping a retired egui context as the wake target can coalesce away all
//! later notifications once its last renderer frame has completed.
use std::sync::{Arc, Mutex};

use eframe::egui;

#[derive(Clone, Default)]
pub struct RepaintTarget(Arc<Mutex<Option<egui::Context>>>);

impl RepaintTarget {
    pub fn new(context: &egui::Context) -> Self {
        let target = Self::default();
        target.set_context(context);
        target
    }

    pub fn set_context(&self, context: &egui::Context) {
        if let Ok(mut current) = self.0.lock() {
            *current = Some(context.clone());
        }
    }

    pub fn with_context(&self, context: &egui::Context) -> Self {
        self.set_context(context);
        self.clone()
    }

    pub fn request_repaint(&self) {
        let context = self.0.lock().ok().and_then(|current| current.clone());
        if let Some(context) = context {
            context.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn an_existing_worker_wakes_the_replacement_context_without_polling() {
        let old = egui::Context::default();
        let target = RepaintTarget::new(&old);
        let worker = target.clone();
        // Saturate the old context's repaint request before its window exits.
        old.request_repaint();
        for _ in 0..2 {
            let next = egui::Context::default();
            let requests = Arc::new(AtomicUsize::new(0));
            let observed = requests.clone();
            next.set_request_repaint_callback(move |_| {
                observed.fetch_add(1, Ordering::Relaxed);
            });
            target.set_context(&next);
            worker.request_repaint();
            assert_eq!(requests.load(Ordering::Relaxed), 1);
        }
    }
}
