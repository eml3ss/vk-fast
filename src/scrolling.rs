// SPDX-License-Identifier: MIT
// Vendored from fastframe-scroll v0.15.2.
// Source: https://github.com/crmne/fastframe/tree/4d115b5e5838c207c0879205e6b1bc16fc1e9a9c/crates/fastframe-scroll
// VK Fast adaptation: import egui through eframe, inline upstream tests, and
// retain upstream introspection accessors only for those tests.
//
// MIT License
//
// Copyright (c) 2026 Carmine Paolino
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Scrolling for egui apps that feels like the platform's.
//!
//! egui scrolls 40 points per wheel notch, about a third of what other apps
//! scroll, and on Linux a touchpad's deltas reach it one to one, with no
//! momentum: scrolling there feels slow next to macOS, where the system adds
//! acceleration and momentum before egui sees a delta. [`Scrolling`] makes up
//! for both, once per frame, before the app lays anything out:
//!
//! - a wheel notch scrolls [`WHEEL_STEP`] points, unless the app set its own
//!   step;
//! - on Linux, touchpad gestures go [`TOUCHPAD_SCALE`] times further and
//!   glide on after the fingers lift, slowing down, as on macOS;
//! - a touchpad gesture holds the axis it started on, so a vertical swipe
//!   does not drift sideways, and a modifier that turns the wheel sideways
//!   (Shift by default) turns the gesture and its glide with it.
//!
//! ```no_run
//! # use eframe::egui;
//! # struct App { scrolling: crate::scrolling::Scrolling }
//! # impl App {
//! fn ui(&mut self, ui: &mut egui::Ui) {
//!     // First thing in the frame, before any scroll area reads the input.
//!     self.scrolling.apply(ui.ctx());
//!     egui::ScrollArea::vertical().show(ui, |ui| {
//!         // ...
//!     });
//! }
//! # }
//! ```

use eframe::egui::{self, Context, Event, MouseWheelUnit, TouchPhase, Vec2};

/// Points a wheel notch scrolls, about what browsers and native lists scroll
/// per notch. egui's own default is 40.
pub const WHEEL_STEP: f32 = 120.0;

/// How much further a Linux touchpad gesture scrolls than its deltas say, to
/// match a macOS touchpad.
pub const TOUCHPAD_SCALE: f32 = 1.8;

/// A gesture that pauses this long, in seconds, has ended where the platform
/// never says when the fingers lift (X11). Measured on the frame's input
/// clock, so a frame that is slow to draw is not taken for a pause.
const GESTURE_GAP: f64 = 0.15;
/// The glide's exponential decay time, in seconds.
const GLIDE_DECAY: f32 = 0.35;
/// The release speed below which a lift starts no glide, points per second.
const GLIDE_START: f32 = 120.0;
/// The speed at which a glide stops, points per second.
const GLIDE_STOP: f32 = 40.0;
/// How long fingers may rest on the pad before lifting and still glide, in
/// seconds: the span the release speed is measured over.
const GLIDE_REST: f64 = 0.1;
/// A first movement this much wider than it is tall starts a sideways
/// gesture.
const SIDEWAYS: f32 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Scrolling state for one window: the gesture in progress and its glide.
///
/// Call [`apply`](Self::apply) once per frame, before anything that scrolls.
/// It rewrites the frame's `InputState::smooth_scroll_delta`, so every
/// `ScrollArea` picks the change up without knowing about it.
#[derive(Debug)]
pub struct Scrolling {
    /// Whether Linux touchpad gestures get the scale and the glide.
    touchpad_help: bool,
    /// Whether the latest scroll input came in points, from a touchpad.
    from_trackpad: bool,
    /// Recent positions of the gesture, for its speed at the lift.
    history: egui::util::History<Vec2>,
    /// Where the gesture has scrolled to so far, for the history.
    travelled: Vec2,
    /// The speed still carrying the page after the fingers lifted.
    glide: Option<Vec2>,
    /// Input time of the gesture's latest movement.
    last_movement: Option<f64>,
    /// The platform says when fingers touch and lift (Wayland does, X11 does
    /// not), so a pause with fingers resting is not taken for a lift.
    lift_announced: bool,
    /// The axis the gesture holds, and the input time it last moved on it.
    lock: Option<(Axis, f64)>,
}

impl Default for Scrolling {
    fn default() -> Self {
        Self::with_touchpad_help(cfg!(target_os = "linux"))
    }
}

impl Scrolling {
    fn with_touchpad_help(touchpad_help: bool) -> Self {
        Self {
            touchpad_help,
            from_trackpad: false,
            history: egui::util::History::new(2..16, 0.1),
            travelled: Vec2::ZERO,
            glide: None,
            last_movement: None,
            lift_announced: false,
            lock: None,
        }
    }

    /// Whether the latest scroll input came from a touchpad (deltas in
    /// points) rather than a wheel (deltas in lines), for an app that pans
    /// with one and zooms with the other.
    #[cfg(test)]
    #[must_use]
    pub fn is_trackpad(&self) -> bool {
        self.from_trackpad
    }

    /// Whether a lifted touchpad gesture is still gliding.
    #[cfg(test)]
    #[must_use]
    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// Ends the gesture in progress: its glide stops and its axis is free.
    /// For an app that takes scrolling over, as middle-click autoscroll
    /// does. What the platform has said about lifts is kept.
    pub fn stop(&mut self) {
        self.glide = None;
        self.history.clear();
        self.travelled = Vec2::ZERO;
        self.last_movement = None;
        self.lock = None;
    }

    /// Applies this frame's scrolling: the wheel step, the touchpad scale and
    /// glide, and the axis lock. Call it once per frame, before anything
    /// scrolls.
    pub fn apply(&mut self, ctx: &Context) {
        // Checked each frame, so a window made again (with a new context)
        // gets the step too. The app's own step wins: only egui's default is
        // replaced.
        let default = egui::InputOptions::default().line_scroll_speed;
        if ctx.options(|options| options.input_options.line_scroll_speed) == default {
            ctx.options_mut(|options| options.input_options.line_scroll_speed = WHEEL_STEP);
        }

        let options = ctx.options(|options| options.input_options);
        let wheel = ctx.input(|input| read_wheel(&input.events, &options));
        let (now, first_pass) = (ctx.input(|input| input.time), ctx.current_pass_index() == 0);
        let moved = wheel.delta != Vec2::ZERO;
        self.lift_announced |= wheel.announced;
        if moved {
            self.from_trackpad = wheel.in_points;
        }

        let helped = self.touchpad_help && self.from_trackpad;
        if helped {
            ctx.input_mut(|input| input.smooth_scroll_delta *= TOUCHPAD_SCALE);
        }
        if helped && moved {
            self.glide = None;
            self.travelled += wheel.delta * TOUCHPAD_SCALE;
            self.history.add(now, self.travelled);
            self.last_movement = Some(now);
            // Where nothing says when the fingers lift, the quiet-gap check
            // below needs a frame to run in.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(GESTURE_GAP / 2.0));
        } else if moved || ctx.input(|input| input.pointer.any_down()) {
            // A wheel notch or a press stops a glide.
            self.glide = None;
            self.history.clear();
            self.last_movement = None;
        }

        // A redone pass (after `Context::request_discard`) gets no input
        // events: that is no pause in the gesture, and the frame's glide step
        // was already taken. Where the platform announces the lift, resting
        // fingers are not taken for one either.
        let quiet = first_pass
            && !self.lift_announced
            && self.last_movement.is_some_and(|at| now - at > GESTURE_GAP);
        if wheel.lifted || quiet {
            self.release(now, wheel.lifted);
        }
        if let Some(speed) = self.glide {
            if !moved && first_pass {
                let dt = ctx.input(|input| input.stable_dt).clamp(0.001, 0.05);
                ctx.input_mut(|input| input.smooth_scroll_delta += speed * dt);
                let slower = speed * (-dt / GLIDE_DECAY).exp();
                self.glide = (slower.length() > GLIDE_STOP).then_some(slower);
            }
            ctx.request_repaint();
        }

        self.lock_axis(ctx, &wheel, now);
    }

    /// The fingers lifted (or, without announcements, paused): only the
    /// movement just before carries on as a glide.
    fn release(&mut self, now: f64, announced: bool) {
        let rested = announced
            && self
                .history
                .iter()
                .last()
                .is_some_and(|(time, _)| now - time > GLIDE_REST);
        let mut speed = if rested {
            Vec2::ZERO
        } else {
            self.history.velocity().unwrap_or(Vec2::ZERO)
        };
        match self.lock.map(|(axis, _)| axis) {
            Some(Axis::Horizontal) => speed.y = 0.0,
            Some(Axis::Vertical) => speed.x = 0.0,
            None => {}
        }
        self.glide = (speed.length() > GLIDE_START).then_some(speed);
        self.history.clear();
        self.travelled = Vec2::ZERO;
        self.last_movement = None;
    }

    /// Holds a touchpad gesture on the axis it started on. Separate wheel
    /// notches may change direction at once, including when the modifier
    /// that turns them sideways is pressed or released.
    fn lock_axis(&mut self, ctx: &Context, wheel: &Wheel, now: f64) {
        let moved = wheel.delta != Vec2::ZERO;
        let held = wheel.forced.or_else(|| {
            self.lock
                .filter(|(_, at)| now - at < GESTURE_GAP && (!moved || wheel.in_points))
                .map(|(axis, _)| axis)
        });
        let axis = match held {
            Some(axis) => axis,
            None if moved && wheel.delta.x.abs() > wheel.delta.y.abs() * SIDEWAYS => {
                Axis::Horizontal
            }
            None if moved => Axis::Vertical,
            None => {
                self.lock = None;
                return;
            }
        };
        if moved {
            self.lock = Some((axis, now));
        }
        ctx.input_mut(|input| match axis {
            Axis::Horizontal => input.smooth_scroll_delta.y = 0.0,
            Axis::Vertical => input.smooth_scroll_delta.x = 0.0,
        });
    }
}

/// What a frame's wheel events add up to.
#[derive(Debug, Default)]
struct Wheel {
    /// The movement, turned the way egui turns it for the modifiers.
    delta: Vec2,
    /// Whether any delta came in points, as a touchpad's do.
    in_points: bool,
    /// Whether the fingers lifted.
    lifted: bool,
    /// Whether the platform said when fingers touched or lifted.
    announced: bool,
    /// The axis a modifier forces, as egui applies it to the smooth delta.
    forced: Option<Axis>,
}

fn read_wheel(events: &[Event], options: &egui::InputOptions) -> Wheel {
    let mut wheel = Wheel::default();
    for event in events {
        if let Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        } = event
        {
            let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
            let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
            wheel.forced = match (horizontal, vertical) {
                (true, false) => Some(Axis::Horizontal),
                (false, true) => Some(Axis::Vertical),
                _ => None,
            };
            wheel.delta += match wheel.forced {
                Some(Axis::Horizontal) => Vec2::new(delta.x + delta.y, 0.0),
                Some(Axis::Vertical) => Vec2::new(0.0, delta.x + delta.y),
                None => *delta,
            };
            wheel.in_points |= *unit == MouseWheelUnit::Point;
            wheel.lifted |= matches!(phase, TouchPhase::End | TouchPhase::Cancel);
            wheel.announced |= *phase != TouchPhase::Move;
        }
    }
    wheel
}

#[cfg(test)]
mod tests {
    use eframe::egui::{
        Context, Event, Modifiers, MouseWheelUnit, RawInput, TouchPhase, Vec2, vec2,
    };

    use super::*;

    #[test]
    fn mouse_navigation_buttons_remain_available_after_scroll_processing() {
        for button in [egui::PointerButton::Extra1, egui::PointerButton::Extra2] {
            let context = Context::default();
            let mut scrolling = linux();
            swipe(&context, &mut scrolling, TouchPhase::Start);
            frame(
                &context,
                &mut scrolling,
                0.1,
                vec![touch(TouchPhase::End, 0.0)],
            );
            assert!(scrolling.gliding());
            let mut output = context.run_ui(
                RawInput {
                    time: Some(0.12),
                    events: vec![
                        Event::PointerMoved(egui::pos2(10.0, 10.0)),
                        Event::PointerButton {
                            pos: egui::pos2(10.0, 10.0),
                            button,
                            pressed: true,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    scrolling.apply(ui.ctx());
                    assert!(ui.input(|input| input.pointer.button_pressed(button)));
                    assert!(ui.input(|input| input.events.iter().any(|event| matches!(
                        event,
                        Event::PointerButton { button: seen, pressed: true, .. } if *seen == button
                    ))));
                },
            );
            output.textures_delta.clear();
            assert!(!scrolling.gliding());
        }
    }

    /// One wheel event.
    fn wheel(unit: MouseWheelUnit, delta: Vec2, phase: TouchPhase, modifiers: Modifiers) -> Event {
        Event::MouseWheel {
            unit,
            delta,
            phase,
            modifiers,
        }
    }

    /// A touchpad movement straight down the pad.
    fn touch(phase: TouchPhase, y: f32) -> Event {
        wheel(MouseWheelUnit::Point, vec2(0.0, y), phase, Modifiers::NONE)
    }

    /// Runs one frame at `time` and returns its smooth scroll delta after
    /// `apply`.
    fn frame(ctx: &Context, scrolling: &mut Scrolling, time: f64, events: Vec<Event>) -> Vec2 {
        let mut delta = Vec2::ZERO;
        let input = RawInput {
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            scrolling.apply(ui.ctx());
            delta = ui.input(|input| input.smooth_scroll_delta);
        });
        output.textures_delta.clear();
        delta
    }

    /// A fast swipe down the pad over six frames, the first in `phase`.
    fn swipe(ctx: &Context, scrolling: &mut Scrolling, phase: TouchPhase) {
        frame(ctx, scrolling, 0.0, vec![touch(phase, 20.0)]);
        for at in 1..6 {
            frame(
                ctx,
                scrolling,
                f64::from(at) * 0.016,
                vec![touch(TouchPhase::Move, 20.0)],
            );
        }
    }

    fn linux() -> Scrolling {
        Scrolling::with_touchpad_help(true)
    }

    #[test]
    fn a_wheel_notch_scrolls_the_wheel_step_unless_the_app_chose_one() {
        let ctx = Context::default();
        frame(&ctx, &mut Scrolling::default(), 0.0, Vec::new());
        assert_eq!(
            ctx.options(|o| o.input_options.line_scroll_speed),
            WHEEL_STEP
        );

        let ctx = Context::default();
        ctx.options_mut(|o| o.input_options.line_scroll_speed = 60.0);
        frame(&ctx, &mut Scrolling::default(), 0.0, Vec::new());
        assert_eq!(ctx.options(|o| o.input_options.line_scroll_speed), 60.0);
    }

    #[test]
    fn wheel_notches_can_change_direction_without_waiting_for_a_gesture_gap() {
        let ctx = Context::default();
        let mut scrolling = linux();
        let notches = [
            (Modifiers::NONE, vec2(0.0, -3.0), false),
            (Modifiers::SHIFT, vec2(0.0, -3.0), true),
            (Modifiers::NONE, vec2(0.0, -3.0), false),
            (Modifiers::NONE, vec2(-3.0, 0.0), true),
        ];
        for (at, (modifiers, delta, sideways)) in notches.into_iter().enumerate() {
            let event = wheel(MouseWheelUnit::Line, delta, TouchPhase::Move, modifiers);
            let scrolled = frame(&ctx, &mut scrolling, at as f64 / 60.0, vec![event]);
            if sideways {
                assert!(scrolled.x < 0.0, "notch {at} goes sideways: {scrolled:?}");
                assert_eq!(scrolled.y, 0.0, "notch {at}");
            } else {
                assert!(scrolled.y < 0.0, "notch {at} goes down: {scrolled:?}");
                assert_eq!(scrolled.x, 0.0, "notch {at}");
            }
        }
        assert!(!scrolling.is_trackpad());
    }

    #[test]
    fn a_linux_touchpad_scrolls_further_than_its_deltas() {
        // The second frame: egui scrolls nothing on the event that starts a
        // gesture.
        let second_frame = |mut scrolling: Scrolling| {
            let ctx = Context::default();
            frame(
                &ctx,
                &mut scrolling,
                0.0,
                vec![touch(TouchPhase::Start, 10.0)],
            );
            let scrolled = frame(
                &ctx,
                &mut scrolling,
                0.016,
                vec![touch(TouchPhase::Move, 10.0)],
            );
            assert!(scrolling.is_trackpad());
            scrolled.y
        };
        assert_eq!(second_frame(Scrolling::with_touchpad_help(false)), 10.0);
        assert_eq!(second_frame(linux()), 10.0 * TOUCHPAD_SCALE);
    }

    #[test]
    fn a_touchpad_flick_glides_after_the_lift_and_slows_to_a_stop() {
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Start);
        frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
        assert!(scrolling.gliding());

        let mut steps = Vec::new();
        let mut at = 0.1;
        while scrolling.gliding() && steps.len() < 1000 {
            at += 1.0 / 60.0;
            steps.push(frame(&ctx, &mut scrolling, at, Vec::new()).y);
        }
        assert!(!scrolling.gliding(), "the glide stops");
        assert!(steps[0] > 0.0, "it carries on the way the fingers went");
        assert!(
            steps.windows(2).all(|pair| pair[1] <= pair[0]),
            "it slows down"
        );
    }

    #[test]
    fn resting_fingers_do_not_glide_where_the_lift_is_announced() {
        // Wayland reports explicit finger contact and lift events.
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Start);
        frame(&ctx, &mut scrolling, 0.3, Vec::new());
        assert!(!scrolling.gliding(), "resting is not a lift");
        frame(&ctx, &mut scrolling, 1.0, vec![touch(TouchPhase::End, 0.0)]);
        assert!(!scrolling.gliding(), "a lift after resting has no speed");
    }

    #[test]
    fn a_quiet_gap_glides_where_the_lift_is_never_announced() {
        // X11 sends every touchpad delta as a plain movement.
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Move);
        frame(&ctx, &mut scrolling, 0.3, Vec::new());
        assert!(scrolling.gliding());
    }

    #[test]
    fn a_slow_frame_is_no_pause_in_the_gesture() {
        // Pauses are measured on the frames' input clock, not the wall clock: a
        // frame that took long to draw, 16 ms of input after the last, changes
        // nothing about the gesture (ZapFast 99adc7e).
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Move);
        frame(&ctx, &mut scrolling, 6.0 * 0.016, Vec::new());
        assert!(!scrolling.gliding(), "a slow frame lets the gesture glide");
        // Sideways input just after keeps the vertical axis.
        let sideways = wheel(
            MouseWheelUnit::Point,
            vec2(30.0, 0.0),
            TouchPhase::Move,
            Modifiers::NONE,
        );
        let scrolled = frame(&ctx, &mut scrolling, 7.0 * 0.016, vec![sideways]);
        assert_eq!(scrolled.x, 0.0, "the gesture holds its axis");
    }

    #[test]
    fn a_redone_pass_neither_ends_the_gesture_nor_glides_again() {
        // egui redoes a discarded pass without the frame's events (ZapFast
        // 11eff4d).
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Move);
        let mut redone_delta = None;
        let input = RawInput {
            time: Some(0.5),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            if ctx.current_pass_index() == 0 {
                ctx.request_discard("measured rows above the view");
            } else {
                scrolling.apply(&ctx);
                redone_delta = Some(ctx.input(|input| input.smooth_scroll_delta));
            }
        });
        output.textures_delta.clear();
        assert_eq!(redone_delta, Some(Vec2::ZERO), "the redone pass scrolls");
        assert!(!scrolling.gliding(), "the redone pass ended the gesture");
    }

    #[test]
    fn a_sideways_modifier_turns_the_gesture_and_its_glide() {
        let ctx = Context::default();
        let mut scrolling = linux();
        let shifted = |phase| {
            wheel(
                MouseWheelUnit::Point,
                vec2(0.0, 20.0),
                phase,
                Modifiers::SHIFT,
            )
        };
        frame(&ctx, &mut scrolling, 0.0, vec![shifted(TouchPhase::Start)]);
        for at in 1..6 {
            let scrolled = frame(
                &ctx,
                &mut scrolling,
                f64::from(at) * 0.016,
                vec![shifted(TouchPhase::Move)],
            );
            assert_eq!(scrolled.y, 0.0, "frame {at} stays sideways");
        }
        frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
        let glide = frame(&ctx, &mut scrolling, 0.1 + 1.0 / 60.0, Vec::new());
        assert!(
            glide.x > 0.0 && glide.y == 0.0,
            "the glide goes sideways: {glide:?}"
        );
    }

    #[test]
    fn a_press_stops_a_glide() {
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Start);
        frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
        assert!(scrolling.gliding());
        let press = Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        };
        frame(
            &ctx,
            &mut scrolling,
            0.12,
            vec![Event::PointerMoved(egui::pos2(10.0, 10.0)), press],
        );
        assert!(!scrolling.gliding());
    }

    #[test]
    fn elsewhere_the_system_scales_and_glides_touchpads_itself() {
        // macOS adds its own acceleration and momentum before egui sees a delta.
        let ctx = Context::default();
        let mut scrolling = Scrolling::with_touchpad_help(false);
        swipe(&ctx, &mut scrolling, TouchPhase::Start);
        frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
        assert!(scrolling.is_trackpad());
        assert!(!scrolling.gliding());
    }

    #[test]
    fn stop_ends_a_glide_and_frees_the_axis() {
        let ctx = Context::default();
        let mut scrolling = linux();
        swipe(&ctx, &mut scrolling, TouchPhase::Start);
        frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
        assert!(scrolling.gliding());
        scrolling.stop();
        assert!(!scrolling.gliding());
        assert_eq!(frame(&ctx, &mut scrolling, 0.12, Vec::new()), Vec2::ZERO);
        // A sideways gesture right after is not held to the old axis.
        let sideways = wheel(
            MouseWheelUnit::Point,
            vec2(30.0, 0.0),
            TouchPhase::Start,
            Modifiers::NONE,
        );
        frame(&ctx, &mut scrolling, 0.13, vec![sideways]);
        let sideways = wheel(
            MouseWheelUnit::Point,
            vec2(30.0, 0.0),
            TouchPhase::Move,
            Modifiers::NONE,
        );
        let scrolled = frame(&ctx, &mut scrolling, 0.146, vec![sideways]);
        assert!(scrolled.x > 0.0, "{scrolled:?}");
    }

    #[test]
    fn a_window_made_again_gets_the_wheel_step_too() {
        let mut scrolling = Scrolling::default();
        frame(&Context::default(), &mut scrolling, 0.0, Vec::new());
        let again = Context::default();
        frame(&again, &mut scrolling, 0.016, Vec::new());
        assert_eq!(
            again.options(|o| o.input_options.line_scroll_speed),
            WHEEL_STEP
        );
    }
}
