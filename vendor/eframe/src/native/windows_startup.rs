//! Windows maximize can show an HWND even when it was constructed invisible.
//! Suppress that native exposure until a frame at the final size is presented.

use egui::ViewportBuilder;

pub(super) fn construction_builder(requested: &ViewportBuilder) -> ViewportBuilder {
    requested.clone().with_visible(false).with_maximized(false)
}

trait Target {
    fn cloak(&mut self, enabled: bool) -> std::io::Result<()>;
    fn apply_restored(&mut self, builder: &ViewportBuilder, position_pixels: Option<[i32; 2]>);
    fn maximize(&mut self);
    fn hide(&mut self);
}

fn prepare(
    target: &mut impl Target,
    requested: &ViewportBuilder,
    position_pixels: Option<[i32; 2]>,
    owned: &mut bool,
) -> std::io::Result<()> {
    let maximize = requested.maximized == Some(true);
    if maximize {
        target.cloak(true)?;
        *owned = true;
    }
    // Establish the normal restore placement before maximizing. Never push
    // these normal dimensions back into an already maximized native window.
    let mut restored = requested.clone();
    restored.maximized = None;
    target.apply_restored(&restored, position_pixels);
    if maximize {
        target.maximize();
        target.hide();
    }
    Ok(())
}

struct Monitor {
    bounds: egui::Rect,
    pixels_per_point: f32,
}

/// Monitors are supplied primary-first. Keep desktop coordinates in physical
/// pixels, including negative origins, rather than mixing per-monitor DPI units.
fn clamp_position(
    position: [i32; 2],
    size_points: egui::Vec2,
    monitors: &[Monitor],
) -> Option<[i32; 2]> {
    if !size_points.is_finite() || size_points.min_elem() <= 0.0 {
        return None;
    }
    let position = egui::pos2(position[0] as f32, position[1] as f32);
    let mut selected = None;
    let mut most_overlap = 0.0;
    for monitor in monitors {
        if !monitor.bounds.is_finite()
            || monitor.bounds.size().min_elem() <= 0.0
            || !monitor.pixels_per_point.is_finite()
            || monitor.pixels_per_point <= 0.0
        {
            continue;
        }
        let window = egui::Rect::from_min_size(position, size_points * monitor.pixels_per_point);
        let overlap = window
            .intersect(monitor.bounds)
            .size()
            .max(egui::Vec2::ZERO);
        let area = overlap.x * overlap.y;
        if selected.is_none() || area > most_overlap {
            selected = Some(monitor);
            most_overlap = area;
        }
    }
    let monitor = selected?;
    let size_pixels = size_points * monitor.pixels_per_point;
    let caption = egui::Rect::from_min_size(
        position,
        egui::vec2(
            size_pixels.x - 1.0,
            (32.0 * monitor.pixels_per_point).min(size_pixels.y) - 1.0,
        ),
    );
    // Preserve intentional placement spanning adjacent displays. A window only
    // needs recovery when its top controls would be outside the desktop.
    if [
        caption.left_top(),
        caption.right_top(),
        caption.left_bottom(),
        caption.right_bottom(),
    ]
    .iter()
    .all(|point| monitors.iter().any(|screen| screen.bounds.contains(*point)))
    {
        return Some([position.x.round() as i32, position.y.round() as i32]);
    }
    let room = (monitor.bounds.size() - size_pixels).max(egui::Vec2::ZERO);
    let position = position.clamp(monitor.bounds.min, monitor.bounds.min + room);
    Some([position.x.round() as i32, position.y.round() as i32])
}

fn release_cloak(
    owned: &mut bool,
    release: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    if *owned {
        release()?;
        *owned = false;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use std::{
        io,
        sync::{Arc, Weak},
    };
    use winit::window::{Theme, Window};

    #[allow(unsafe_code)]
    fn cloak(window: &Window, enabled: bool) -> io::Result<()> {
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
        let RawWindowHandle::Win32(handle) =
            window.window_handle().map_err(io::Error::other)?.as_raw()
        else {
            return Err(io::ErrorKind::Unsupported.into());
        };
        let enabled = i32::from(enabled);
        // The HWND is borrowed from a live winit window on its UI thread. The
        // API copies the four-byte BOOL and retains no pointer to this stack.
        let result = unsafe {
            DwmSetWindowAttribute(
                handle.hwnd.get() as _,
                DWMWA_CLOAK as u32,
                (&raw const enabled).cast(),
                size_of::<i32>() as u32,
            )
        };
        if result >= 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "DWM startup cloak failed (HRESULT {result:#010x})"
            )))
        }
    }

    struct NativeTarget<'a> {
        window: &'a Window,
        ctx: &'a egui::Context,
    }
    impl Target for NativeTarget<'_> {
        fn cloak(&mut self, enabled: bool) -> io::Result<()> {
            cloak(self.window, enabled)
        }
        fn apply_restored(&mut self, builder: &ViewportBuilder, position_pixels: Option<[i32; 2]>) {
            let mut builder = builder.clone();
            let mut restored_position = None;
            if let Some(position) = position_pixels {
                let zoom = self.ctx.zoom_factor();
                let scale = self.window.scale_factor() as f32 * zoom;
                let inner = self.window.inner_size();
                let outer = self.window.outer_size();
                let frame = egui::vec2(
                    outer.width.saturating_sub(inner.width) as f32,
                    outer.height.saturating_sub(inner.height) as f32,
                ) / scale;
                let size = builder
                    .inner_size
                    .unwrap_or_else(|| egui::vec2(inner.width as f32, inner.height as f32) / scale)
                    + frame;
                let monitors: Vec<_> = self
                    .window
                    .primary_monitor()
                    .into_iter()
                    .chain(self.window.available_monitors())
                    .map(|monitor| {
                        let origin = monitor.position();
                        let size = monitor.size();
                        Monitor {
                            bounds: egui::Rect::from_min_size(
                                egui::pos2(origin.x as f32, origin.y as f32),
                                egui::vec2(size.width as f32, size.height as f32),
                            ),
                            pixels_per_point: monitor.scale_factor() as f32 * zoom,
                        }
                    })
                    .collect();
                if let Some([x, y]) = clamp_position(position, size, &monitors) {
                    // Moving first lets WM_DPICHANGED select the destination
                    // DPI before the normal client dimensions are reapplied.
                    self.window
                        .set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                    restored_position = Some(winit::dpi::PhysicalPosition::new(x, y));
                }
                builder.position = None;
            }
            egui_winit::apply_viewport_builder_to_window(self.ctx, self.window, &builder);
            // A DPI transition may suggest a shifted outer rectangle. Preserve
            // the requested desktop origin after restoring the normal size.
            if let Some(position) = restored_position
                && self.window.outer_position().ok() != Some(position)
            {
                self.window.set_outer_position(position);
            }
        }
        fn maximize(&mut self) {
            self.window.set_maximized(true);
        }
        fn hide(&mut self) {
            self.window.set_visible(false);
        }
    }

    pub(crate) struct WindowStartup {
        window: Weak<Window>,
        cloaked: bool,
    }
    impl WindowStartup {
        pub(crate) fn prepare(
            ctx: &egui::Context,
            window: &Arc<Window>,
            requested: &ViewportBuilder,
            position_pixels: Option<[i32; 2]>,
        ) -> io::Result<Self> {
            let mut startup = Self {
                window: Arc::downgrade(window),
                cloaked: false,
            };
            window.set_theme(Some(if ctx.global_style().visuals.dark_mode {
                Theme::Dark
            } else {
                Theme::Light
            }));
            super::prepare(
                &mut NativeTarget { window, ctx },
                requested,
                position_pixels,
                &mut startup.cloaked,
            )?;
            Ok(startup)
        }

        /// Called after successful present and the optional automatic Show.
        /// Also release a cloak for intentionally hidden roots, without Show.
        pub(crate) fn presented(&mut self) -> io::Result<()> {
            release_cloak(&mut self.cloaked, || {
                if let Some(window) = self.window.upgrade() {
                    cloak(&window, false)?;
                }
                Ok(())
            })
        }
    }
    impl Drop for WindowStartup {
        fn drop(&mut self) {
            if self.cloaked
                && let Some(window) = self.window.upgrade()
            {
                // Failure/abort must not strand a visible, cloaked native
                // window. Hide before releasing the app-owned cloak.
                window.set_visible(false);
                let _ = cloak(&window, false);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use native::WindowStartup;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_keeps_physical_coordinates_across_monitor_scales_and_negative_origins() {
        let monitors = [
            Monitor {
                bounds: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0)),
                pixels_per_point: 1.0,
            },
            Monitor {
                bounds: egui::Rect::from_min_size(
                    egui::pos2(-2560.0, -200.0),
                    egui::vec2(2560.0, 1440.0),
                ),
                pixels_per_point: 1.5,
            },
        ];
        let size = egui::vec2(1040.0, 750.0);
        assert_eq!(
            clamp_position([-2300, -80], size, &monitors),
            Some([-2300, -80])
        );
        assert_eq!(clamp_position([120, 80], size, &monitors), Some([120, 80]));
        let spanning = [
            Monitor {
                bounds: monitors[0].bounds,
                pixels_per_point: 1.0,
            },
            Monitor {
                bounds: egui::Rect::from_min_size(
                    egui::pos2(1920.0, 0.0),
                    egui::vec2(3840.0, 2160.0),
                ),
                pixels_per_point: 2.0,
            },
        ];
        assert_eq!(
            clamp_position([1700, 80], size, &spanning),
            Some([1700, 80])
        );
        // The old left monitor is disconnected; recover onto the primary.
        assert_eq!(
            clamp_position([-2300, -80], size, &monitors[..1]),
            Some([0, 0])
        );
        assert_eq!(
            clamp_position([5000, 5000], size, &monitors[..1]),
            Some([880, 330])
        );
        assert_eq!(
            clamp_position([400, 400], egui::vec2(5000.0, 4000.0), &monitors[..1]),
            Some([0, 0])
        );
        assert_eq!(clamp_position([120, 80], size, &[]), None);
    }
    struct Fake {
        events: Vec<&'static str>,
        cloaked: bool,
        normal_size: egui::Vec2,
        normal_position: Option<[i32; 2]>,
        fail_cloak: bool,
    }
    impl Target for Fake {
        fn cloak(&mut self, enabled: bool) -> std::io::Result<()> {
            self.events.push(if enabled { "cloak" } else { "uncloak" });
            if self.fail_cloak {
                return Err(std::io::ErrorKind::Unsupported.into());
            }
            self.cloaked = enabled;
            Ok(())
        }
        fn apply_restored(&mut self, builder: &ViewportBuilder, position: Option<[i32; 2]>) {
            assert!(builder.maximized.is_none());
            self.events.push("restored-bounds");
            self.normal_size = builder.inner_size.expect("restored dimensions");
            self.normal_position = position;
        }
        fn maximize(&mut self) {
            assert!(self.cloaked || self.fail_cloak);
            self.events.push("maximize");
        }
        fn hide(&mut self) {
            self.events.push("hide");
        }
    }

    #[test]
    fn saved_maximized_constructor_is_hidden_nonmax_and_cloaks_before_any_maximize() {
        let requested = ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_maximized(true);
        let effective = construction_builder(&requested);
        assert_eq!(effective.visible, Some(false));
        assert_eq!(effective.maximized, Some(false));
        assert_eq!(requested.maximized, Some(true));
        assert_eq!(effective.inner_size, requested.inner_size);
        let mut target = Fake {
            events: vec!["create-hidden-nonmax"],
            cloaked: false,
            normal_size: egui::Vec2::ZERO,
            normal_position: None,
            fail_cloak: false,
        };
        let mut owned = false;
        prepare(&mut target, &requested, Some([-1680, 120]), &mut owned).expect("startup cloak");
        assert!(owned);
        assert_eq!(
            target.events,
            [
                "create-hidden-nonmax",
                "cloak",
                "restored-bounds",
                "maximize",
                "hide"
            ]
        );
        assert_eq!(target.normal_size, egui::vec2(1040.0, 720.0));
        assert_eq!(target.normal_position, Some([-1680, 120]));
    }

    #[test]
    fn unsupported_cloak_aborts_before_any_maximize_or_show_and_keeps_preferences() {
        let requested = ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_maximized(true)
            .with_visible(false);
        let mut target = Fake {
            events: vec![],
            cloaked: false,
            normal_size: egui::Vec2::ZERO,
            normal_position: None,
            fail_cloak: true,
        };
        let mut owned = false;
        assert!(prepare(&mut target, &requested, None, &mut owned).is_err());
        assert!(!owned);
        assert!(!target.cloaked);
        assert_eq!(target.events, ["cloak"]);
        assert_eq!(requested.inner_size, Some(egui::vec2(1040.0, 720.0)));
        assert_eq!(requested.visible, Some(false));
    }

    #[test]
    fn intentionally_hidden_first_present_releases_cloak_without_auto_show() {
        let requested = ViewportBuilder::default()
            .with_inner_size([1040.0, 720.0])
            .with_maximized(true)
            .with_visible(false);
        let mut target = Fake {
            events: vec![],
            cloaked: false,
            normal_size: egui::Vec2::ZERO,
            normal_position: None,
            fail_cloak: false,
        };
        let mut owned = false;
        prepare(&mut target, &requested, None, &mut owned).expect("startup cloak");
        let mut gate = super::super::first_frame::FirstFrame::new(Some(false));
        gate.present(
            || {
                target.events.push("present");
                Ok::<_, ()>(())
            },
            || panic!("explicitly hidden window must not auto-show"),
        )
        .expect("synthetic swap");
        release_cloak(&mut owned, || target.cloak(false)).expect("uncloak after hidden paint");
        assert!(!owned && !target.cloaked);
        assert_eq!(
            &target.events[target.events.len() - 2..],
            ["present", "uncloak"]
        );
    }

    #[test]
    fn failed_uncloak_preserves_ownership_for_retry_or_abort_cleanup() {
        let mut owned = true;
        assert!(release_cloak(&mut owned, || Err(std::io::ErrorKind::Other.into())).is_err());
        assert!(owned);
        release_cloak(&mut owned, || Ok(())).expect("retry succeeds");
        assert!(!owned);
    }

    #[test]
    fn cloak_ownership_is_recorded_before_any_fallible_builder_application() {
        struct PanickingTarget;
        impl Target for PanickingTarget {
            fn cloak(&mut self, _: bool) -> std::io::Result<()> {
                Ok(())
            }
            fn apply_restored(&mut self, _: &ViewportBuilder, _: Option<[i32; 2]>) {
                panic!("synthetic native setup failure");
            }
            fn maximize(&mut self) {
                panic!("unreachable after setup failure");
            }
            fn hide(&mut self) {}
        }
        let mut owned = false;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare(
                &mut PanickingTarget,
                &ViewportBuilder::default().with_maximized(true),
                None,
                &mut owned,
            )
        }));
        assert!(result.is_err());
        assert!(
            owned,
            "the already-created native guard must own abort cleanup"
        );
    }
}
