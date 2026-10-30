//! Common tools used by [`super::glow_integration`] and [`super::wgpu_integration`].

use web_time::Instant;

use std::{path::PathBuf, sync::Arc};
use winit::event_loop::ActiveEventLoop;

use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

use egui::{DeferredViewportUiCallback, ViewportBuilder, ViewportId};
use egui_winit::{EventResponse, WindowSettings};

use crate::epi;

#[cfg_attr(target_os = "ios", allow(dead_code, unused_variables, unused_mut))]
pub fn viewport_builder(
    egui_zoom_factor: f32,
    event_loop: &ActiveEventLoop,
    native_options: &mut epi::NativeOptions,
    window_settings: Option<WindowSettings>,
) -> ViewportBuilder {
    profiling::function_scope!();

    let mut viewport_builder = native_options.viewport.clone();

    // On some Linux systems, a window size larger than the monitor causes crashes,
    // and on Windows the window does not appear at all.
    let clamp_size_to_monitor_size = viewport_builder.clamp_size_to_monitor_size.unwrap_or(true);

    // Always use the default window size / position on iOS. Trying to restore the previous position
    // causes the window to be shown too small.
    #[cfg(not(target_os = "ios"))]
    let inner_size_points = if let Some(mut window_settings) = window_settings {
        // Restore pos/size from previous session

        if clamp_size_to_monitor_size {
            window_settings.clamp_size_to_sane_values(largest_monitor_point_size(
                egui_zoom_factor,
                event_loop,
            ));
        }
        window_settings.clamp_position_to_monitors(egui_zoom_factor, event_loop);

        viewport_builder = window_settings.initialize_viewport_builder(
            egui_zoom_factor,
            event_loop,
            viewport_builder,
        );
        window_settings.inner_size_points()
    } else {
        if let Some(pos) = viewport_builder.position {
            viewport_builder = viewport_builder.with_position(pos);
        }

        if clamp_size_to_monitor_size && let Some(initial_window_size) = viewport_builder.inner_size
        {
            let initial_window_size = egui::NumExt::at_most(
                initial_window_size,
                largest_monitor_point_size(egui_zoom_factor, event_loop),
            );
            viewport_builder = viewport_builder.with_inner_size(initial_window_size);
        }

        viewport_builder.inner_size
    };

    #[cfg(not(target_os = "ios"))]
    if native_options.centered {
        profiling::scope!("center");
        if let Some(monitor) = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
        {
            let monitor_size = monitor
                .size()
                .to_logical::<f32>(egui_zoom_factor as f64 * monitor.scale_factor());
            let inner_size = inner_size_points.unwrap_or(egui::Vec2 { x: 800.0, y: 600.0 });
            if 0.0 < monitor_size.width && 0.0 < monitor_size.height {
                let x = (monitor_size.width - inner_size.x) / 2.0;
                let y = (monitor_size.height - inner_size.y) / 2.0;
                viewport_builder = viewport_builder.with_position([x, y]);
            }
        }
    }

    match core::mem::take(&mut native_options.window_builder) {
        Some(hook) => hook(viewport_builder),
        None => viewport_builder,
    }
}

pub fn apply_window_settings(
    window: &winit::window::Window,
    window_settings: Option<WindowSettings>,
) {
    profiling::function_scope!();
    if let Some(window_settings) = window_settings {
        window_settings.initialize_window(window);
    }
}

#[cfg(not(target_os = "ios"))]
fn largest_monitor_point_size(egui_zoom_factor: f32, event_loop: &ActiveEventLoop) -> egui::Vec2 {
    profiling::function_scope!();
    let mut max_size = egui::Vec2::ZERO;

    let available_monitors = {
        profiling::scope!("available_monitors");
        event_loop.available_monitors()
    };

    for monitor in available_monitors {
        let size = monitor
            .size()
            .to_logical::<f32>(egui_zoom_factor as f64 * monitor.scale_factor());
        let size = egui::vec2(size.width, size.height);
        max_size = max_size.max(size);
    }

    if max_size == egui::Vec2::ZERO {
        egui::Vec2::splat(16000.0)
    } else {
        max_size
    }
}

// ----------------------------------------------------------------------------

/// For loading/saving app state and/or egui memory to disk.
pub fn create_storage(_app_name: &str) -> Option<Box<dyn epi::Storage>> {
    #[cfg(feature = "persistence")]
    if let Some(storage) = super::file_storage::FileStorage::from_app_id(_app_name) {
        return Some(Box::new(storage));
    }
    None
}

#[allow(clippy::allow_attributes, clippy::unnecessary_wraps)]
pub fn create_storage_with_file(_file: impl Into<PathBuf>) -> Option<Box<dyn epi::Storage>> {
    #[cfg(feature = "persistence")]
    return Some(Box::new(
        super::file_storage::FileStorage::from_ron_filepath(_file),
    ));
    #[cfg(not(feature = "persistence"))]
    None
}

// ----------------------------------------------------------------------------

/// Everything needed to make a winit-based integration for [`epi`].
///
/// Only one instance per app (not one per viewport).
pub struct EpiIntegration {
    pub frame: epi::Frame,
    last_auto_save: Instant,
    pub beginning: Instant,
    first_frame: super::first_frame::FirstFrame,
    pub egui_ctx: egui::Context,

    /// Input that we have received, but not yet given to egui,
    /// because we haven't run any pass since (see [`Self::update_logic_only`]).
    pending_raw_input: egui::RawInput,

    pending_full_output: egui::FullOutput,

    ui_platform_state: super::winit_integration::UiPlatformState,
    ui_frame_clock: super::ui_frame_clock::UiFrameClock,

    /// A due native wake already ticked logic, even if its compositor redraw
    /// has not arrived yet. Its eventual UI pass must not double-apply it.
    logic_updated_without_ui: bool,

    /// When set, it is time to close the native window.
    close: bool,

    can_drag_window: bool,
    #[cfg(feature = "persistence")]
    persist_window: bool,
    app_icon_setter: super::app_icon::AppTitleIconSetter,
}

impl EpiIntegration {
    #[allow(clippy::allow_attributes, clippy::too_many_arguments)]
    pub fn new(
        egui_ctx: egui::Context,
        window: &Arc<winit::window::Window>,
        app_name: &str,
        native_options: &crate::NativeOptions,
        storage: Option<Box<dyn epi::Storage>>,
        #[cfg(feature = "glow")] gl: Option<std::sync::Arc<glow::Context>>,
        #[cfg(feature = "glow")] glow_register_native_texture: Option<
            Box<dyn FnMut(glow::Texture) -> egui::TextureId>,
        >,
        #[cfg(feature = "wgpu_no_default_features")] wgpu_render_state: Option<
            egui_wgpu::RenderState,
        >,
    ) -> Self {
        let frame = epi::Frame {
            info: epi::IntegrationInfo { cpu_usage: None },
            storage,
            #[cfg(feature = "glow")]
            gl,
            #[cfg(feature = "glow")]
            glow_register_native_texture,
            #[cfg(feature = "wgpu_no_default_features")]
            wgpu_render_state,
            window: Some(Arc::clone(window)),
            raw_display_handle: window.display_handle().map(|h| h.as_raw()),
            raw_window_handle: window.window_handle().map(|h| h.as_raw()),
        };

        let icon = native_options
            .viewport
            .icon
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(load_default_egui_icon()));

        let app_icon_setter = super::app_icon::AppTitleIconSetter::new(
            native_options
                .viewport
                .title
                .clone()
                .unwrap_or_else(|| app_name.to_owned()),
            Some(icon),
        );

        Self {
            frame,
            last_auto_save: Instant::now(),
            pending_raw_input: Default::default(),
            pending_full_output: Default::default(),
            ui_platform_state: Default::default(),
            ui_frame_clock: super::ui_frame_clock::UiFrameClock::new(
                window
                    .current_monitor()
                    .and_then(|monitor| monitor.refresh_rate_millihertz()),
            ),
            logic_updated_without_ui: false,
            close: false,
            can_drag_window: false,
            #[cfg(feature = "persistence")]
            persist_window: native_options.persist_window,
            app_icon_setter,
            beginning: Instant::now()
                .checked_sub(web_time::Duration::from_secs_f64(egui_ctx.time()))
                .unwrap_or_else(Instant::now),
            first_frame: super::first_frame::FirstFrame::new(native_options.viewport.visible),
            egui_ctx,
        }
    }

    /// If `true`, it is time to close the native window.
    pub fn should_close(&self) -> bool {
        self.close
    }

    pub fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        egui_winit: &mut egui_winit::State,
        event: &winit::event::WindowEvent,
    ) -> EventResponse {
        profiling::function_scope!(egui_winit::short_window_event_description(event));

        use winit::event::{ElementState, MouseButton, WindowEvent};

        if let WindowEvent::MouseInput {
            button: MouseButton::Left,
            state: ElementState::Pressed,
            ..
        } = event
        {
            self.can_drag_window = true;
        }

        egui_winit.on_window_event(window, event)
    }

    pub fn pre_update(&mut self) {
        self.app_icon_setter.update();
    }

    /// Run user code - this can create immediate viewports, so hold no locks over this!
    ///
    /// If `viewport_ui_cb` is None, we are in the root viewport and will call
    /// [`crate::App::logic`] and [`crate::App::ui`].
    ///
    /// Only call this when the ui will actually be shown;
    /// use [`Self::update_logic_only`] otherwise.
    pub fn update(
        &mut self,
        app: &mut dyn epi::App,
        viewport_ui_cb: Option<&DeferredViewportUiCallback>,
        raw_input: egui::RawInput,
    ) -> egui::FullOutput {
        let mut raw_input = self.prepare_raw_input(app, raw_input);
        let is_root_viewport = viewport_ui_cb.is_none();
        if is_root_viewport {
            raw_input.predicted_dt = self.ui_frame_clock.begin_ui(raw_input.time);
        }

        let close_requested = raw_input.viewport().close_requested();

        let logic_updated_without_ui =
            is_root_viewport && core::mem::take(&mut self.logic_updated_without_ui);

        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            if let Some(viewport_ui_cb) = viewport_ui_cb {
                // Child viewport
                profiling::scope!("viewport_callback");
                viewport_ui_cb(ui);
            } else {
                if !logic_updated_without_ui {
                    profiling::scope!("App::logic");
                    app.logic(ui.ctx(), &mut self.frame);
                }
                {
                    profiling::scope!("App::ui");
                    app.ui(ui, &mut self.frame);
                }
            }
        });

        if is_root_viewport && close_requested {
            let canceled = full_output.viewport_output[&ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose);
            self.handle_close_request(canceled);
        }

        if is_root_viewport {
            self.ui_frame_clock.end_ui(
                full_output
                    .viewport_output
                    .get(&ViewportId::ROOT)
                    .is_some_and(|output| output.repaint_delay.is_zero()),
            );
            self.ui_platform_state
                .remember_ui(&full_output.platform_output);
        }
        self.pending_full_output.append(full_output);
        core::mem::take(&mut self.pending_full_output)
    }

    /// Resolve a native close request immediately, even when the compositor
    /// withholds redraw callbacks for an unfocused or covered window. Run both
    /// app hooks so UI-only `CancelClose` handlers still get their normal chance.
    /// Output is retained if closing is canceled; no GPU work is needed here.
    pub fn update_close_request(
        &mut self,
        app: &mut dyn epi::App,
        raw_input: egui::RawInput,
    ) -> bool {
        // Closing is a new input event; its logic must see the close request
        // even if a preceding media/timer wake already ran ordinary logic.
        self.logic_updated_without_ui = false;
        let mut output = self.update(app, None, raw_input);
        if self.should_close() {
            // This pass will never be painted. Explicitly retire newly created
            // CPU texture deltas instead of triggering their debug Drop guard.
            output.textures_delta.clear();
            self.pending_full_output.textures_delta.clear();
            true
        } else {
            self.pending_full_output.append(output);
            false
        }
    }

    /// Let the app tick its logic without showing any ui,
    /// because the window is minimized or occluded.
    ///
    /// No egui pass is run, so all ui state is left untouched:
    /// the app will find everything where it left it once the window is visible again.
    ///
    /// Only call this for the root viewport: only it has [`crate::App::logic`].
    pub fn update_logic_only(
        &mut self,
        app: &mut dyn epi::App,
        raw_input: egui::RawInput,
    ) -> egui::LogicOutput {
        let raw_input = self.prepare_raw_input(app, raw_input);

        let close_requested = raw_input.viewport().close_requested();

        let mut logic_output = self.egui_ctx.run_logic(&raw_input, |ctx| {
            profiling::scope!("App::logic");
            app.logic(ctx, &mut self.frame);
        });
        self.ui_platform_state
            .preserve_for_logic(&mut logic_output.platform_output);
        self.logic_updated_without_ui = true;

        // No pass consumed the input, so save it for the next one:
        self.pending_raw_input = raw_input;

        if close_requested {
            let canceled = logic_output
                .viewport_commands
                .get(&ViewportId::ROOT)
                .is_some_and(|commands| commands.contains(&egui::ViewportCommand::CancelClose));
            self.handle_close_request(canceled);
        }

        logic_output
    }

    /// Prepend any input we couldn't give to egui earlier, set the time, and run the app hook.
    fn prepare_raw_input(
        &mut self,
        app: &mut dyn epi::App,
        new_input: egui::RawInput,
    ) -> egui::RawInput {
        let mut raw_input = core::mem::take(&mut self.pending_raw_input);
        raw_input.append(new_input); // The new input wins where they overlap

        raw_input.time = Some(self.beginning.elapsed().as_secs_f64());

        app.raw_input_hook(&self.egui_ctx, &mut raw_input);

        raw_input
    }

    fn handle_close_request(&mut self, canceled: bool) {
        if canceled {
            log::debug!("Closing of root viewport canceled with ViewportCommand::CancelClose");
        } else {
            log::debug!("Closing root viewport (ViewportCommand::CancelClose was not sent)");
            self.close = true;
        }
    }

    pub fn report_frame_time(&mut self, seconds: f32) {
        self.frame.info.cpu_usage = Some(seconds);
    }

    #[cfg(any(not(target_os = "windows"), feature = "wgpu_no_default_features"))]
    pub fn post_rendering(&mut self, window: &winit::window::Window) {
        profiling::function_scope!();
        self.present_and_show_first_frame(window, || Ok::<_, core::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {});
    }

    /// Windows must have a front buffer before showing the HWND. Painting into
    /// an OpenGL back buffer alone still leaves a default surface on screen.
    pub fn present_and_show_first_frame<E>(
        &mut self,
        window: &winit::window::Window,
        present: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        #[cfg(target_os = "windows")]
        let theme = if self.egui_ctx.global_style().visuals.dark_mode {
            winit::window::Theme::Dark
        } else {
            winit::window::Theme::Light
        };
        self.first_frame.present(present, || {
            #[cfg(target_os = "windows")]
            window.set_theme(Some(theme));
            window.set_visible(true);
        })
    }

    // ------------------------------------------------------------------------
    // Persistence stuff:

    pub fn maybe_autosave(
        &mut self,
        app: &mut dyn epi::App,
        window: Option<&winit::window::Window>,
    ) {
        let now = Instant::now();
        if now - self.last_auto_save > app.auto_save_interval() {
            self.save(app, window);
            self.last_auto_save = now;
        }
    }

    pub fn save(&mut self, app: &mut dyn epi::App, window: Option<&winit::window::Window>) {
        #[cfg(not(feature = "persistence"))]
        let _ = (self, app, window);

        #[cfg(feature = "persistence")]
        if let Some(storage) = self.frame.storage_mut() {
            profiling::function_scope!();

            if let Some(window) = window
                && self.persist_window
            {
                profiling::scope!("native_window");
                epi::set_value(
                    storage,
                    STORAGE_WINDOW_KEY,
                    &WindowSettings::from_window(self.egui_ctx.zoom_factor(), window),
                );
            }
            if app.persist_egui_memory() {
                profiling::scope!("egui_memory");
                self.egui_ctx
                    .memory(|mem| epi::set_value(storage, STORAGE_EGUI_MEMORY_KEY, mem));
            }
            {
                profiling::scope!("App::save");
                app.save(storage);
            }

            profiling::scope!("Storage::flush");
            storage.flush();
        }
    }
}

fn load_default_egui_icon() -> egui::IconData {
    profiling::function_scope!();
    #[expect(clippy::unwrap_used)]
    crate::icon_data::from_png_bytes(&include_bytes!("../../data/icon.png")[..]).unwrap()
}

#[cfg(feature = "persistence")]
const STORAGE_EGUI_MEMORY_KEY: &str = "egui";

#[cfg(feature = "persistence")]
const STORAGE_WINDOW_KEY: &str = "window";

pub fn load_window_settings(_storage: Option<&dyn epi::Storage>) -> Option<WindowSettings> {
    profiling::function_scope!();
    #[cfg(feature = "persistence")]
    {
        epi::get_value(_storage?, STORAGE_WINDOW_KEY)
    }
    #[cfg(not(feature = "persistence"))]
    None
}

pub fn load_egui_memory(_storage: Option<&dyn epi::Storage>) -> Option<egui::Memory> {
    profiling::function_scope!();
    #[cfg(feature = "persistence")]
    {
        epi::get_value(_storage?, STORAGE_EGUI_MEMORY_KEY)
    }
    #[cfg(not(feature = "persistence"))]
    None
}

#[cfg(test)]
mod background_event_tests {
    use super::*;
    use std::time::Duration;

    struct App {
        cancel_close: bool,
        logic: usize,
        ui: usize,
        saw_unfocused_close: bool,
        send_command: bool,
    }

    impl epi::App for App {
        fn logic(&mut self, ctx: &egui::Context, _: &mut epi::Frame) {
            self.logic += 1;
            self.saw_unfocused_close =
                ctx.input(|input| !input.focused && input.viewport().close_requested());
            if self.send_command {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("media-command-handled".into()));
            }
        }

        fn ui(&mut self, ui: &mut egui::Ui, _: &mut epi::Frame) {
            self.ui += 1;
            ui.label("Close confirmation state survives until a visible repaint");
            if self.cancel_close {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
    }

    fn integration() -> EpiIntegration {
        EpiIntegration {
            frame: epi::Frame::_new_kittest(),
            last_auto_save: Instant::now(),
            beginning: Instant::now(),
            first_frame: super::super::first_frame::FirstFrame::new(Some(false)),
            egui_ctx: egui::Context::default(),
            pending_raw_input: egui::RawInput::default(),
            pending_full_output: egui::FullOutput::default(),
            ui_platform_state: Default::default(),
            ui_frame_clock: super::super::ui_frame_clock::UiFrameClock::new(None),
            logic_updated_without_ui: false,
            close: false,
            can_drag_window: false,
            #[cfg(feature = "persistence")]
            persist_window: false,
            app_icon_setter: super::super::app_icon::AppTitleIconSetter::new(
                "close-test".into(),
                None,
            ),
        }
    }

    #[test]
    fn background_logic_and_discarded_passes_preserve_high_refresh_ui_time_after_idle() {
        struct ClockApp {
            time: f64,
            immediate: bool,
            stable: f32,
        }
        impl epi::App for ClockApp {
            fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
                input.time = Some(self.time);
            }
            fn logic(&mut self, ctx: &egui::Context, _: &mut epi::Frame) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            fn ui(&mut self, ui: &mut egui::Ui, _: &mut epi::Frame) {
                if ui.ctx().current_pass_index() == 0 {
                    self.stable = ui.input(|input| input.stable_dt);
                    ui.ctx().request_discard("synthetic second layout pass");
                }
                if self.immediate {
                    ui.ctx().request_repaint();
                }
            }
        }
        for hz in [60.0, 120.0, 144.0, 240.0, 280.0, 360.0] {
            let mut integration = integration();
            integration.ui_frame_clock =
                super::super::ui_frame_clock::UiFrameClock::new(Some((hz * 1000.0) as u32));
            let mut app = ClockApp {
                time: 0.0,
                immediate: true,
                stable: 0.0,
            };
            for frame in 0..12 {
                app.time = f64::from(frame) / hz;
                for _ in 0..3 {
                    integration.update_logic_only(&mut app, input(false));
                }
                let mut output = integration.update(&mut app, None, input(false));
                output.textures_delta.clear();
                assert!(
                    (f64::from(app.stable) - 1.0 / hz).abs() < 0.000_01,
                    "UI prediction must follow the actual frame period, not logic wakes"
                );
            }
            app.immediate = false;
            for tick in 1..12 {
                app.time = 1.0 + f64::from(tick) * 0.1;
                for _ in 0..3 {
                    integration.update_logic_only(&mut app, input(false));
                }
                let mut output = integration.update(&mut app, None, input(false));
                output.textures_delta.clear();
            }
            app.time = 10.0;
            app.immediate = true;
            for _ in 0..3 {
                integration.update_logic_only(&mut app, input(false));
            }
            let mut wheel = input(false);
            wheel.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            let mut output = integration.update(&mut app, None, wheel);
            output.textures_delta.clear();
            assert!(
                (f64::from(app.stable) - 1.0 / hz).abs() < 0.000_01,
                "idle playback timers must not become the first wheel timestep"
            );
        }
    }

    fn input(close: bool) -> egui::RawInput {
        let mut input = egui::RawInput {
            focused: false,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500.0, 300.0),
            )),
            ..Default::default()
        };
        let viewport = input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .expect("Raw input contains its root viewport");
        viewport.focused = Some(false);
        if close {
            viewport.events.push(egui::ViewportEvent::Close);
        }
        input
    }

    fn app(cancel_close: bool) -> App {
        App {
            cancel_close,
            logic: 0,
            ui: 0,
            saw_unfocused_close: false,
            send_command: false,
        }
    }

    #[test]
    fn unfocused_close_is_accepted_without_a_native_repaint_or_gpu() {
        let mut integration = integration();
        let mut app = app(false);
        assert!(integration.update_close_request(&mut app, input(true)));
        assert!(integration.should_close() && app.saw_unfocused_close);
        assert_eq!((app.logic, app.ui), (1, 1));
    }

    #[test]
    fn unfocused_close_honors_ui_cancel_and_preserves_output_until_the_next_frame() {
        let mut integration = integration();
        let mut app = app(true);
        assert!(!integration.update_close_request(&mut app, input(true)));
        assert!(app.saw_unfocused_close);
        assert!(!integration.pending_full_output.shapes.is_empty());
        assert!(!integration.should_close());
        app.cancel_close = false;
        assert!(integration.update_close_request(&mut app, input(true)));
        assert_eq!((app.logic, app.ui), (2, 2));
    }

    #[test]
    fn unfocused_logic_wake_processes_commands_without_running_ui() {
        let mut integration = integration();
        let mut app = app(false);
        app.send_command = true;
        let output = integration.update_logic_only(&mut app, input(false));
        assert_eq!((app.logic, app.ui), (1, 0));
        assert!(!integration.should_close());
        assert!(output.viewport_commands[&ViewportId::ROOT].iter().any(|command|
            matches!(command, egui::ViewportCommand::Title(title) if title == "media-command-handled")));
    }

    struct BackgroundWork {
        logic: usize,
        ui: usize,
        queued: usize,
        handled: usize,
    }

    impl epi::App for BackgroundWork {
        fn logic(&mut self, ctx: &egui::Context, _: &mut epi::Frame) {
            self.logic += 1;
            if self.queued > 0 {
                self.handled += self.queued;
                self.queued = 0;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "handled-{}",
                    self.handled
                )));
            }
            // Models VK Fast's engine/completion tick. There must be another
            // callback even if the compositor never paints the current frame.
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        fn ui(&mut self, ui: &mut egui::Ui, _: &mut epi::Frame) {
            self.ui += 1;
            ui.label("A pending compositor frame must not own the player clock");
        }
    }

    #[test]
    fn background_timers_and_three_external_wakes_continue_without_any_ui_pass_even_with_focus_reported()
     {
        let mut integration = integration();
        let (sender, receiver) = std::sync::mpsc::channel();
        integration
            .egui_ctx
            .set_request_repaint_callback(move |request| {
                sender.send(request).unwrap();
            });
        let mut app = BackgroundWork {
            logic: 0,
            ui: 0,
            queued: 0,
            handled: 0,
        };
        let mut raw = input(false);
        // The compositor can withhold a frame even when a backend reports
        // focus. The scheduling route must not use focus as a logic gate.
        raw.focused = true;
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().focused = Some(true);

        for _ in 0..6 {
            integration.update_logic_only(&mut app, raw.clone());
            let wakes: Vec<_> = receiver.try_iter().collect();
            assert!(
                !wakes.is_empty(),
                "A background timer was coalesced behind an unpainted pass"
            );
            if app.logic > 1 {
                assert!(
                    wakes.iter().any(|wake| wake.delay > Duration::ZERO
                        && wake.delay <= Duration::from_millis(100))
                );
            }
        }
        assert_eq!((app.logic, app.ui), (6, 0));
        assert_eq!(integration.egui_ctx.cumulative_pass_nr(), 0);

        for expected in 1..=3 {
            app.queued = 1;
            integration.egui_ctx.request_repaint();
            assert!(
                receiver.try_iter().any(|wake| wake.delay == Duration::ZERO),
                "External wake {expected} disappeared"
            );
            let output = integration.update_logic_only(&mut app, raw.clone());
            assert_eq!(app.handled, expected);
            assert!(output.viewport_commands[&ViewportId::ROOT].iter().any(
                |command| matches!(command,
                    egui::ViewportCommand::Title(title) if title == &format!("handled-{expected}")
                )
            ));
            // Consume egui's finite settling wakes, then prove the timer is
            // armed again before another independent command arrives.
            for _ in 0..3 {
                receiver.try_iter().for_each(drop);
                integration.update_logic_only(&mut app, raw.clone());
            }
            assert!(receiver.try_iter().any(|wake| wake.delay > Duration::ZERO));
        }
        assert_eq!(app.ui, 0);
        assert_eq!(integration.egui_ctx.cumulative_pass_nr(), 0);
    }

    #[test]
    fn background_logic_is_not_applied_twice_when_the_compositor_finally_paints() {
        let mut integration = integration();
        let mut app = app(false);
        integration.update_logic_only(&mut app, input(false));
        assert_eq!((app.logic, app.ui), (1, 0));
        let mut output = integration.update(&mut app, None, input(false));
        output.textures_delta.clear();
        assert_eq!((app.logic, app.ui), (1, 1));
        let mut output = integration.update(&mut app, None, input(false));
        output.textures_delta.clear();
        assert_eq!((app.logic, app.ui), (2, 2));
    }

    #[test]
    fn background_close_runs_fresh_logic_after_a_timer_tick_and_still_honors_cancel_close() {
        let mut integration = integration();
        let mut app = app(true);
        integration.update_logic_only(&mut app, input(false));
        assert_eq!((app.logic, app.ui), (1, 0));
        assert!(!integration.update_close_request(&mut app, input(true)));
        assert_eq!((app.logic, app.ui), (2, 1));
        assert!(app.saw_unfocused_close);
        assert!(!integration.should_close());
        // As with a tray hide, no render is required for another close input.
        app.cancel_close = false;
        assert!(integration.update_close_request(&mut app, input(true)));
        assert_eq!((app.logic, app.ui), (3, 2));
    }

    #[test]
    fn background_logic_keeps_the_hover_cursor_until_the_next_real_ui_pass() {
        struct Hover {
            rect: egui::Rect,
            ui: usize,
        }
        impl epi::App for Hover {
            fn logic(&mut self, ctx: &egui::Context, _: &mut epi::Frame) {
                ctx.copy_text("synthetic clipboard command".into());
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("logic remains active".into()));
            }
            fn ui(&mut self, ui: &mut egui::Ui, _: &mut epi::Frame) {
                self.ui += 1;
                self.rect = ui
                    .button("Clickable")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .rect;
            }
        }
        let mut integration = integration();
        let mut app = Hover {
            rect: egui::Rect::NOTHING,
            ui: 0,
        };
        let mut raw = input(false);
        raw.focused = true;
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().focused = Some(true);
        for _ in 0..3 {
            let mut output = integration.update(&mut app, None, raw.clone());
            output.textures_delta.clear();
        }
        raw.events
            .push(egui::Event::PointerMoved(app.rect.center()));
        let mut output = integration.update(&mut app, None, raw.clone());
        output.textures_delta.clear();
        assert_eq!(
            output.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        let ui_calls = app.ui;
        raw.events.clear();
        for _ in 0..10 {
            let output = integration.update_logic_only(&mut app, raw.clone());
            assert_eq!(
                output.platform_output.cursor_icon,
                egui::CursorIcon::PointingHand
            );
            assert!(output.platform_output.commands.iter().any(|command|
                matches!(command, egui::OutputCommand::CopyText(text) if text == "synthetic clipboard command")));
            assert!(output.viewport_commands[&ViewportId::ROOT].iter().any(|command|
                matches!(command, egui::ViewportCommand::Title(title) if title == "logic remains active")));
        }
        assert_eq!(app.ui, ui_calls);

        // A real UI hover transition still changes the native cursor normally.
        raw.events
            .push(egui::Event::PointerMoved(egui::pos2(450.0, 250.0)));
        let mut output = integration.update(&mut app, None, raw.clone());
        output.textures_delta.clear();
        assert_eq!(
            output.platform_output.cursor_icon,
            egui::CursorIcon::Default
        );
        raw.events.clear();
        for _ in 0..3 {
            let output = integration.update_logic_only(&mut app, raw.clone());
            assert_eq!(
                output.platform_output.cursor_icon,
                egui::CursorIcon::Default
            );
        }
    }

    #[test]
    fn background_logic_preserves_text_ime_and_does_not_reset_the_caret_blink() {
        struct Text {
            value: String,
            request_focus: bool,
            rect: egui::Rect,
        }
        impl epi::App for Text {
            fn ui(&mut self, ui: &mut egui::Ui, _: &mut epi::Frame) {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.value).id(egui::Id::new("caret-blink")),
                );
                self.rect = response.rect;
                if self.request_focus {
                    response.request_focus();
                    self.request_focus = false;
                }
            }
        }
        fn has_caret(shape: &egui::Shape, stroke: egui::Stroke) -> bool {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| has_caret(shape, stroke)),
                egui::Shape::LineSegment { stroke: actual, .. } => *actual == stroke,
                _ => false,
            }
        }
        let mut integration = integration();
        let stroke = egui::Stroke::new(3.0, egui::Color32::from_rgb(253, 17, 229));
        integration.egui_ctx.all_styles_mut(|style| {
            style.visuals.text_cursor.stroke = stroke;
            style.visuals.text_cursor.on_duration = 0.5;
            style.visuals.text_cursor.off_duration = 0.5;
            style.visuals.text_cursor.preview = false;
            style.visuals.text_cursor.blink = true;
        });
        let mut app = Text {
            value: "synthetic".into(),
            request_focus: true,
            rect: egui::Rect::NOTHING,
        };
        let mut raw = input(false);
        raw.focused = true;
        raw.viewports.get_mut(&ViewportId::ROOT).unwrap().focused = Some(true);
        for _ in 0..3 {
            integration.beginning = Instant::now();
            let mut output = integration.update(&mut app, None, raw.clone());
            output.textures_delta.clear();
        }
        raw.events
            .push(egui::Event::PointerMoved(app.rect.center()));
        for (time, visible) in [
            (0.1, true),
            (0.4, true),
            (0.6, false),
            (0.9, false),
            (1.1, true),
        ] {
            // The integration normally takes this clock from Instant; advancing
            // its origin makes the blink test deterministic without sleeping.
            integration.beginning = Instant::now() - Duration::from_secs_f64(time);
            let mut output = integration.update(&mut app, None, raw.clone());
            output.textures_delta.clear();
            assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::Text);
            assert!(output.platform_output.ime.is_some());
            assert_eq!(
                output
                    .shapes
                    .iter()
                    .any(|shape| has_caret(&shape.shape, stroke)),
                visible,
                "caret at {time}"
            );
            let ime = output.platform_output.ime;
            raw.events.clear();
            for _ in 0..8 {
                let output = integration.update_logic_only(&mut app, raw.clone());
                assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::Text);
                assert_eq!(output.platform_output.ime, ime);
            }
        }

        integration
            .egui_ctx
            .memory_mut(|memory| memory.surrender_focus(egui::Id::new("caret-blink")));
        let mut output = integration.update(&mut app, None, raw.clone());
        output.textures_delta.clear();
        assert!(output.platform_output.ime.is_none());
        assert!(
            integration
                .update_logic_only(&mut app, raw)
                .platform_output
                .ime
                .is_none()
        );
    }
}
