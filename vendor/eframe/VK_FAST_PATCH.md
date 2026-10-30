# VK Fast background event processing

This is eframe 0.36.2, upstream commit
`49682f8baa058bf49e011035cfbd6e825f88a5ef`.
Upstream: https://github.com/emilk/egui/tree/49682f8baa058bf49e011035cfbd6e825f88a5ef/crates/eframe
The upstream MIT and Apache-2.0 license texts are included unchanged.

Changes:

- Consume a native close request synchronously, without depending on a
  compositor `RedrawRequested` callback. A close pass runs both application
  hooks to preserve `ViewportCommand::CancelClose`, retains pending output if
  cancellation occurs, and does not clear, paint or present a GPU surface.
- Every due visible-root wake ticks `App::logic` and applies native commands
  independently of focus and compositor redraw delivery. A focused surface can
  also be covered. Rendering is still requested, but its eventual UI pass does
  not double-run logic already consumed by the wake. Close inputs always run
  fresh logic so tray cancellation and UI `CancelClose` keep working.
- A repaint callback is also a logic wake, not only a paint-generation hint.
  Multi-pass UI layout cannot discard queued worker/media/timer work merely
  because the paint counter has advanced. Egui's own finite settling requests
  and the application's repaint deadlines drive the event loop; no polling
  timer or `ControlFlow::Poll` was added.
- Glow uses non-blocking `SwapInterval::DontWait` on Wayland and calls
  `Window::pre_present_notify` before swapping to let Wayland frame callbacks
  pace rendering. Glutin explicitly documents that blocking EGL vsync can wait
  until a hidden window becomes visible again, freezing the whole UI/event
  thread. Requested vsync is preserved on Windows and X11. See
  https://docs.rs/glutin/0.32.3/glutin/surface/enum.SwapInterval.html.
- Both Glow and WGPU expose the same CPU-only dispatch. Save/destroy/on_exit
  handling is unchanged; a minimized/covered Wayland surface never needs GPU
  presentation to process a close or playback command.

Headless regressions cover accepted/canceled background close, fresh close
logic after an earlier timer tick, three separate external wakes and repeated
100ms deadlines without any UI pass, and no double logic when painting resumes.
`native/run.rs` checks redraw/logic and hidden bootstrap routing; Glow checks
Wayland swap policy while preserving Windows/X11 vsync.

A native probe was also run on Wayland using only its own test
window: after floating/minimizing it, three independent worker-thread repaint
requests were consumed with focus false while the UI paint counter stayed
  fixed. Logic timers continued, and a compositor-native close exited in 64ms
without restoring focus. The earlier blocking-vsync version reproduced the
hang inside EGL swap; non-blocking swap plus pre-present notification fixed it.

The probe also exercised VK Fast's actual resident `window_shell` implementation:
a restore intent closed only the minimized native window with the reopen flag,
created a new visible surface, and handed the same app state to it. Its counters
continued and a worker wake reached the replacement egui context. Neither
`on_exit` nor `Drop` ran between the two windows; both ran only after the final
native close (54ms). A plain Wayland attention/focus request did not restore the
minimized surface in this compositor, which is why VK Fast recreates that native
surface when restoring a Wayland tray window.

Run them with:

```sh
cargo test --manifest-path vendor/eframe/Cargo.toml --target-dir target \
  --no-default-features --features default_fonts,glow,wayland,x11 \
  --lib background_
```
