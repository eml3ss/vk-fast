//! Recreate only the native window when its decoration mode changes. The
//! resident application (and audio/network workers) survives the old renderer.
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use eframe::egui;

use crate::app::MusicApp;

pub trait ResidentApp: eframe::App {
    fn window_reopen_requested(&self) -> bool;
    fn prepare_reopened_window(&mut self, ctx: &egui::Context);
    fn uses_system_titlebar(&self) -> bool;
    fn background_window_hidden(&self) -> bool {
        false
    }
    fn background_quit_requested(&self) -> bool {
        false
    }
    fn background_context(&self) -> egui::Context {
        egui::Context::default()
    }
    fn background_logic(&mut self, _ctx: &egui::Context) {}
    /// Panel widths to save with the window geometry.
    fn panel_widths(&self) -> Option<[f32; 2]> {
        None
    }
    fn restore_panel_widths(&mut self, _widths: [f32; 2]) {}
}

impl ResidentApp for MusicApp {
    fn window_reopen_requested(&self) -> bool {
        MusicApp::window_reopen_requested(self)
    }

    fn prepare_reopened_window(&mut self, ctx: &egui::Context) {
        MusicApp::prepare_reopened_window(self, ctx);
    }

    fn uses_system_titlebar(&self) -> bool {
        MusicApp::uses_system_titlebar(self)
    }
    fn background_window_hidden(&self) -> bool {
        self.tray_background_requested()
    }
    fn background_quit_requested(&self) -> bool {
        self.tray_quit_requested()
    }
    fn background_context(&self) -> egui::Context {
        MusicApp::background_context(self)
    }
    fn background_logic(&mut self, ctx: &egui::Context) {
        self.background_tick(ctx, None);
    }
    fn panel_widths(&self) -> Option<[f32; 2]> {
        Some(MusicApp::panel_widths(self))
    }
    fn restore_panel_widths(&mut self, widths: [f32; 2]) {
        MusicApp::restore_panel_widths(self, widths);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowGeometry {
    size: egui::Vec2,
    position: Option<egui::Pos2>,
    position_pixels: Option<[i32; 2]>,
    maximized: bool,
    fullscreen: bool,
    panels: Option<[f32; 2]>,
}

impl WindowGeometry {
    fn from_viewport(viewport: &egui::ViewportBuilder) -> Self {
        Self {
            size: viewport
                .inner_size
                .filter(|size| usable_size(*size))
                .unwrap_or(egui::vec2(1040.0, 720.0)),
            position: viewport.position.filter(|position| position.is_finite()),
            position_pixels: None,
            maximized: viewport.maximized.unwrap_or(false),
            fullscreen: viewport.fullscreen.unwrap_or(false),
            panels: None,
        }
    }

    fn remember(&mut self, viewport: &egui::ViewportInfo, zoom_factor: f32) {
        if viewport.minimized == Some(true) {
            return;
        }
        self.maximized = viewport.maximized.unwrap_or(self.maximized);
        self.fullscreen = viewport.fullscreen.unwrap_or(self.fullscreen);
        // Keep the last restored size/position, rather than reopening a
        // maximized window with its screen-sized bounds as the restored size.
        if !self.maximized && !self.fullscreen {
            if let Some(rect) = viewport.inner_rect.filter(|rect| usable_size(rect.size())) {
                self.size = rect.size();
            }
            if let Some(rect) = viewport.outer_rect.filter(|rect| rect.min.is_finite()) {
                self.position = Some(rect.min);
                if let Some(scale) = viewport.native_pixels_per_point {
                    let position = rect.min * (scale * zoom_factor);
                    if position.is_finite()
                        && scale > 0.0
                        && zoom_factor > 0.0
                        && position.x.abs() <= 1_000_000.0
                        && position.y.abs() <= 1_000_000.0
                    {
                        self.position_pixels =
                            Some([position.x.round() as i32, position.y.round() as i32]);
                    }
                }
            }
        }
    }

    fn restore(&mut self, saved: crate::window_state::SavedWindow) {
        self.size = egui::Vec2::from(saved.size);
        self.maximized = saved.maximized;
        self.position_pixels = saved.position_pixels;
        self.panels = saved.panels;
    }

    fn saved(self) -> crate::window_state::SavedWindow {
        crate::window_state::SavedWindow {
            size: [self.size.x, self.size.y],
            maximized: self.maximized,
            position_pixels: self.position_pixels,
            panels: self.panels,
        }
    }

    fn apply_options(self, mut options: eframe::NativeOptions) -> eframe::NativeOptions {
        options.viewport = self.apply(options.viewport);
        #[cfg(windows)]
        if let Some(position) = self.position_pixels {
            // Startup applies physical coordinates on the destination monitor
            // before maximizing or presenting the first frame.
            options.viewport.position = None;
            options.window_position_pixels = Some(position);
        }
        options
    }

    fn apply(self, viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let viewport = viewport
            .with_inner_size(self.size)
            .with_maximized(self.maximized)
            .with_fullscreen(self.fullscreen);
        if let Some(position) = self.position {
            viewport.with_position(position)
        } else {
            viewport
        }
    }
}

fn usable_size(size: egui::Vec2) -> bool {
    size.is_finite() && (320.0..=16384.0).contains(&size.x) && (240.0..=16384.0).contains(&size.y)
}

struct Handoff<A> {
    app: Option<A>,
    geometry: WindowGeometry,
}

struct WindowApp<A> {
    app: Option<A>,
    geometry: WindowGeometry,
    handoff: Rc<RefCell<Handoff<A>>>,
}

impl<A: ResidentApp> eframe::App for WindowApp<A> {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let zoom = ctx.zoom_factor();
        ctx.input(|input| self.geometry.remember(input.viewport(), zoom));
        if let Some(app) = &mut self.app {
            app.logic(ctx, frame);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(app) = &mut self.app {
            app.ui(ui, frame);
            self.geometry.panels = app.panel_widths().or(self.geometry.panels);
        }
        let zoom = ui.ctx().zoom_factor();
        ui.input(|input| self.geometry.remember(input.viewport(), zoom));
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        save_geometry(self.geometry);
        if self
            .app
            .as_ref()
            .is_some_and(|app| app.window_reopen_requested() || app.background_window_hidden())
        {
            let mut handoff = self.handoff.borrow_mut();
            handoff.geometry = self.geometry;
            handoff.app = self.app.take();
        } else if let Some(app) = &mut self.app {
            app.on_exit(gl);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(app) = &mut self.app {
            app.save(storage);
        }
    }

    fn auto_save_interval(&self) -> Duration {
        self.app
            .as_ref()
            .map_or(Duration::from_secs(30), eframe::App::auto_save_interval)
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.app
            .as_ref()
            .map_or(crate::theme::BACKGROUND.to_normalized_gamma_f32(), |app| {
                app.clear_color(visuals)
            })
    }

    fn persist_egui_memory(&self) -> bool {
        self.app
            .as_ref()
            .is_none_or(eframe::App::persist_egui_memory)
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(app) = &mut self.app {
            app.raw_input_hook(ctx, raw_input);
        }
    }
}

/// Eframe reuses its thread-local native event loop with `run_and_return`.
/// Moving the application out during on_exit avoids running its Drop handler
/// between windows, so playback and in-flight requests are retained.
pub fn run_native<A: ResidentApp + 'static>(
    app_name: &str,
    mut native_options: impl FnMut() -> eframe::NativeOptions,
    create_app: impl FnOnce(
        &eframe::CreationContext<'_>,
    ) -> Result<A, Box<dyn std::error::Error + Send + Sync>>
    + 'static,
) -> eframe::Result {
    let mut initial = Some(create_app);
    let first_options = native_options();
    let mut first_geometry = WindowGeometry::from_viewport(&first_options.viewport);
    #[cfg(test)]
    let saved = None;
    #[cfg(not(test))]
    let saved = crate::window_state::load().ok().flatten();
    if let Some(saved) = saved {
        first_geometry.restore(saved);
        crate::diagnostics::event("window", "restore: saved_geometry");
    }
    let first_options = first_geometry.apply_options(first_options);
    let handoff = Rc::new(RefCell::new(Handoff::<A> {
        app: None,
        geometry: first_geometry,
    }));
    let mut first_options = Some(first_options);
    loop {
        let mut options = first_options.take().unwrap_or_else(&mut native_options);
        options.run_and_return = true;
        let (retained, geometry) = {
            let mut state = handoff.borrow_mut();
            (state.app.take(), state.geometry)
        };
        if let Some(app) = &retained {
            options = geometry.apply_options(options);
            options.viewport = options
                .viewport
                .with_decorations(app.uses_system_titlebar());
        }
        let creator = if retained.is_none() {
            initial.take()
        } else {
            None
        };
        let destination = handoff.clone();
        eframe::run_native(
            app_name,
            options,
            Box::new(move |cc| {
                let app = if let Some(mut app) = retained {
                    app.prepare_reopened_window(&cc.egui_ctx);
                    app
                } else {
                    let mut app = creator.expect("Initial application constructor")(cc)?;
                    if let Some(panels) = geometry.panels {
                        app.restore_panel_widths(panels);
                    }
                    app
                };
                Ok(Box::new(WindowApp {
                    app: Some(app),
                    geometry,
                    handoff: destination,
                }))
            }),
        )?;
        let background = {
            let mut state = handoff.borrow_mut();
            if state
                .app
                .as_ref()
                .is_some_and(ResidentApp::background_window_hidden)
            {
                state.app.take()
            } else {
                None
            }
        };
        if let Some(app) = background {
            match run_background(app) {
                Some(app) => handoff.borrow_mut().app = Some(app),
                None => return Ok(()),
            }
        }
        if handoff.borrow().app.is_none() {
            return Ok(());
        }
    }
}

fn save_geometry(geometry: WindowGeometry) {
    #[cfg(not(test))]
    if crate::window_state::save(geometry.saved()).is_err() {
        crate::diagnostics::event("window", "save: failed");
    }
    #[cfg(test)]
    let _ = geometry;
}

#[derive(Default)]
struct BackgroundWake {
    deadline: Mutex<Option<Instant>>,
    changed: Condvar,
}

impl BackgroundWake {
    fn request(&self, delay: Duration) {
        let Some(when) = Instant::now().checked_add(delay) else {
            return;
        };
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deadline.is_none_or(|previous| when < previous) {
            *deadline = Some(when);
        }
        self.changed.notify_one();
    }

    fn wait(&self) {
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(when) = *deadline {
                let now = Instant::now();
                if when <= now {
                    *deadline = None;
                    return;
                }
                deadline = self
                    .changed
                    .wait_timeout(deadline, when - now)
                    .unwrap_or_else(|error| error.into_inner())
                    .0;
            } else {
                deadline = self
                    .changed
                    .wait(deadline)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }
    }
}

fn run_background<A: ResidentApp>(mut app: A) -> Option<A> {
    let ctx = app.background_context();
    let wake = Arc::new(BackgroundWake::default());
    let requested = wake.clone();
    ctx.set_request_repaint_callback(move |info| requested.request(info.delay));
    wake.request(Duration::ZERO);
    let mut input = egui::RawInput {
        focused: false,
        ..Default::default()
    };
    if let Some(viewport) = input.viewports.get_mut(&egui::ViewportId::ROOT) {
        viewport.focused = Some(false);
        viewport.minimized = Some(true);
    }
    crate::diagnostics::event("window", "background: native_surface_closed");
    loop {
        wake.wait();
        let _ = ctx.run_logic(&input, |ctx| app.background_logic(ctx));
        if app.background_quit_requested() {
            app.on_exit(None);
            return None;
        }
        if !app.background_window_hidden() {
            crate::diagnostics::event("window", "background: restore_surface");
            return Some(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App;
    use std::cell::Cell;

    struct TestApp {
        reopen: bool,
        hidden: bool,
        exits: Rc<Cell<usize>>,
        drops: Rc<Cell<usize>>,
    }

    impl eframe::App for TestApp {
        fn ui(&mut self, _: &mut egui::Ui, _: &mut eframe::Frame) {}
        fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
            self.exits.set(self.exits.get() + 1);
        }
    }

    impl ResidentApp for TestApp {
        fn window_reopen_requested(&self) -> bool {
            self.reopen
        }
        fn prepare_reopened_window(&mut self, _: &egui::Context) {
            self.reopen = false;
        }
        fn uses_system_titlebar(&self) -> bool {
            true
        }
        fn background_window_hidden(&self) -> bool {
            self.hidden
        }
    }

    impl Drop for TestApp {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn replacing_a_window_retains_the_application_until_an_actual_exit() {
        let exits = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let geometry = WindowGeometry::from_viewport(&egui::ViewportBuilder::default());
        let handoff = Rc::new(RefCell::new(Handoff {
            app: None,
            geometry,
        }));
        let mut window = WindowApp {
            app: Some(TestApp {
                reopen: true,
                hidden: false,
                exits: exits.clone(),
                drops: drops.clone(),
            }),
            geometry,
            handoff: handoff.clone(),
        };
        window.on_exit(None);
        drop(window);
        assert_eq!(exits.get(), 0);
        assert_eq!(drops.get(), 0);
        let mut app = handoff.borrow_mut().app.take().unwrap();
        app.prepare_reopened_window(&egui::Context::default());
        let mut replacement = WindowApp {
            app: Some(app),
            geometry,
            handoff: handoff.clone(),
        };
        replacement.on_exit(None);
        drop(replacement);
        assert!(handoff.borrow().app.is_none());
        assert_eq!(exits.get(), 1);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn closing_the_native_surface_for_the_tray_does_not_exit_or_drop_the_app() {
        let exits = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let geometry = WindowGeometry::from_viewport(&egui::ViewportBuilder::default());
        let handoff = Rc::new(RefCell::new(Handoff {
            app: None,
            geometry,
        }));
        let mut window = WindowApp {
            app: Some(TestApp {
                reopen: false,
                hidden: true,
                exits: exits.clone(),
                drops: drops.clone(),
            }),
            geometry,
            handoff: handoff.clone(),
        };
        window.on_exit(None);
        drop(window);
        assert!(handoff.borrow().app.as_ref().unwrap().hidden);
        assert_eq!(exits.get(), 0);
        assert_eq!(drops.get(), 0);
    }

    struct BackgroundApp {
        ctx: egui::Context,
        ticks: usize,
        quit: bool,
        exits: Rc<Cell<usize>>,
        drops: Rc<Cell<usize>>,
    }

    impl eframe::App for BackgroundApp {
        fn ui(&mut self, _: &mut egui::Ui, _: &mut eframe::Frame) {
            panic!("Hidden application must not render UI");
        }
        fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
            self.exits.set(self.exits.get() + 1);
        }
    }

    impl ResidentApp for BackgroundApp {
        fn window_reopen_requested(&self) -> bool {
            self.ticks >= 3 && !self.quit
        }
        fn prepare_reopened_window(&mut self, _: &egui::Context) {}
        fn uses_system_titlebar(&self) -> bool {
            false
        }
        fn background_window_hidden(&self) -> bool {
            self.ticks < 3
        }
        fn background_quit_requested(&self) -> bool {
            self.ticks >= 3 && self.quit
        }
        fn background_context(&self) -> egui::Context {
            self.ctx.clone()
        }
        fn background_logic(&mut self, ctx: &egui::Context) {
            assert!(!ctx.input(|input| input.focused));
            self.ticks += 1;
            ctx.request_repaint_after(Duration::from_millis(1));
        }
    }

    impl Drop for BackgroundApp {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn hidden_loop_runs_scheduled_logic_and_retains_the_app_on_restore_but_exits_once_on_quit() {
        for quit in [false, true] {
            let exits = Rc::new(Cell::new(0));
            let drops = Rc::new(Cell::new(0));
            let result = run_background(BackgroundApp {
                ctx: egui::Context::default(),
                ticks: 0,
                quit,
                exits: exits.clone(),
                drops: drops.clone(),
            });
            if quit {
                assert!(result.is_none());
                assert_eq!(exits.get(), 1);
                assert_eq!(drops.get(), 1);
            } else {
                let app = result.unwrap();
                assert_eq!(app.ticks, 3);
                assert!(app.window_reopen_requested());
                assert_eq!(exits.get(), 0);
                assert_eq!(drops.get(), 0);
                drop(app);
                assert_eq!(drops.get(), 1);
            }
        }
    }

    #[test]
    fn persisted_position_uses_desktop_pixels_and_survives_maximize_minimize_and_reopen() {
        let mut geometry = WindowGeometry::from_viewport(&egui::ViewportBuilder::default());
        let normal = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(
                egui::pos2(-890.0, 90.0),
                egui::vec2(900.0, 600.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(-900.0, 60.0),
                egui::vec2(920.0, 640.0),
            )),
            native_pixels_per_point: Some(1.5),
            maximized: Some(false),
            fullscreen: Some(false),
            ..Default::default()
        };
        geometry.remember(&normal, 1.25);
        assert_eq!(geometry.position_pixels, Some([-1688, 113]));
        for viewport in [
            egui::ViewportInfo {
                maximized: Some(true),
                ..normal.clone()
            },
            egui::ViewportInfo {
                minimized: Some(true),
                ..normal.clone()
            },
            egui::ViewportInfo {
                fullscreen: Some(true),
                ..normal.clone()
            },
        ] {
            geometry.remember(
                &egui::ViewportInfo {
                    outer_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(-32000.0, -32000.0),
                        egui::vec2(1920.0, 1080.0),
                    )),
                    ..viewport
                },
                1.25,
            );
            assert_eq!(geometry.position_pixels, Some([-1688, 113]));
            assert_eq!(geometry.size, egui::vec2(900.0, 600.0));
        }
        geometry.remember(
            &egui::ViewportInfo {
                maximized: Some(true),
                ..normal
            },
            1.25,
        );
        let mut restored = WindowGeometry::from_viewport(&egui::ViewportBuilder::default());
        restored.restore(geometry.saved());
        assert_eq!(restored.position_pixels, Some([-1688, 113]));
        assert_eq!(restored.size, geometry.size);
        assert!(restored.maximized);
        assert!(!restored.fullscreen);
        let options = restored.apply_options(eframe::NativeOptions::default());
        assert_eq!(options.viewport.inner_size, Some(geometry.size));
        assert_eq!(options.viewport.maximized, Some(true));
        #[cfg(windows)]
        {
            assert_eq!(options.window_position_pixels, restored.position_pixels);
            assert_eq!(options.viewport.position, None);
        }
    }

    #[test]
    fn recreation_keeps_restored_bounds_while_maximized_and_ignores_invalid_geometry() {
        let viewport = egui::ViewportBuilder::default().with_inner_size([1000.0, 700.0]);
        let mut geometry = WindowGeometry::from_viewport(&viewport);
        geometry.remember(
            &egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(40.0, 50.0),
                    egui::vec2(1100.0, 800.0),
                )),
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(34.0, 25.0),
                    egui::vec2(1112.0, 831.0),
                )),
                maximized: Some(false),
                fullscreen: Some(false),
                ..Default::default()
            },
            1.0,
        );
        geometry.remember(
            &egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                maximized: Some(true),
                ..Default::default()
            },
            1.0,
        );
        let builder = geometry.apply(egui::ViewportBuilder::default());
        assert_eq!(builder.inner_size, Some(egui::vec2(1100.0, 800.0)));
        assert_eq!(builder.position, Some(egui::pos2(34.0, 25.0)));
        assert_eq!(builder.maximized, Some(true));
        geometry.remember(
            &egui::ViewportInfo {
                inner_rect: Some(egui::Rect::NOTHING),
                maximized: Some(false),
                ..Default::default()
            },
            1.0,
        );
        assert_eq!(geometry.size, egui::vec2(1100.0, 800.0));
        geometry.remember(
            &egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1.0, 1.0),
                )),
                minimized: Some(true),
                maximized: Some(true),
                ..Default::default()
            },
            1.0,
        );
        assert_eq!(geometry.size, egui::vec2(1100.0, 800.0));
        assert!(!geometry.maximized);
    }
}
