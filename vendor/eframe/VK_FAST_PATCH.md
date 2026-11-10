# VK Fast background event processing

This is eframe 0.36.2, upstream commit
`49682f8baa058bf49e011035cfbd6e825f88a5ef`.
Upstream: https://github.com/emilk/egui/tree/49682f8baa058bf49e011035cfbd6e825f88a5ef/crates/eframe
The upstream MIT and Apache-2.0 license texts are included unchanged.

Changes:

- Windows viewport focus commands bypass winit's synthetic-Alt foreground
  workaround. A direct Win32 request is made only for a visible, restored HWND
  owned by this process; an already foreground window is left alone. If Windows
  denies activation, only taskbar attention is requested. No keyboard events,
  keyboard-state resets or input-queue attachment are used. Glow and WGPU share
  the same ordered dispatch.
- A process-local popup-menu palette wrapper resolves the supported system
  theme functions once. Windows 10 1809 and 1903+ use their respective function
  signatures; older/unsupported builds keep the native default. High contrast
  overrides forced palettes. The application reapplies the current Windows
  preference immediately before the tray popup opens, without a polling timer.

- Windows Glow accepts a root outer position in physical desktop pixels.
  Startup clamps it to a connected monitor, moves the still-hidden normal
  window before applying DPI-scaled dimensions, and only then maximizes under
  the existing DWM cloak. Both immediate WGL and deferred creation use this
  order; a different monitor scale does not multiply the stored desktop origin.

- A safe Windows `windows_small_icon_size()` wrapper reads `SM_CXSMICON`,
  so the application builds its tray image at the notification area's exact
  size instead of letting the shell rescale a larger one.

- A safe Windows `allow_foreground_activation(pid)` wrapper lets a newly
  user-launched secondary process transfer foreground permission to the
  existing instance before notifying it. Zero and the all-processes sentinel
  are rejected; the application itself remains free of unsafe code.

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
- On Windows, Glow reveals the initially hidden root only after its first
  successful `swap_buffers`. Previously `post_rendering` made the HWND visible
  while the first frame was still in the OpenGL back buffer, exposing an
  uninitialized/default surface. Before showing, the native window theme is
  matched to the rendered egui visuals using winit's safe `set_theme` API.
  WGPU already invokes its reveal hook after its painter presents. Other
  platforms keep their existing bootstrap/present order, including Wayland's
  non-blocking swap and frame-callback notification.
- First-frame reveal is one-shot per native window lifetime: a failed present
  cannot reveal the window, later presents cannot undo hiding to the tray,
  and an explicitly requested initial `visible(false)` stays hidden until an
  application visibility command. Recreated windows receive a fresh gate.
- A saved maximized Windows Glow root is constructed hidden and non-maximized.
  Winit's maximize operation calls `ShowWindow(SW_MAXIMIZE)` even for a hidden
  builder, so a DWM app cloak is acquired before applying restored bounds and
  maximization in both the WGL `DisplayBuilder` and deferred/EGL creation paths.
  The GL surface is created at the resulting maximized client size. Restored
  dimensions are never reapplied to that maximized native window.
  After a successful full-size swap and the requested automatic Show, the
  app cloak is released. This completion also runs for explicitly hidden roots,
  without showing them. A pending startup gets one bootstrap UI/present even if
  Windows reports occlusion. Cloak ownership is established before native setup;
  abort cleanup hides before uncloaking. Failure to enable the cloak aborts
  maximized startup before any maximize/show, while failed uncloak retains
  ownership and propagates the error for cleanup. Non-maximized startup and
  non-Windows rendering order are unchanged. DWM cloak requires Windows 8 or
  newer; VK Fast's supported Windows target is Windows 10 or newer.
- CPU-only logic output retains the cursor, custom cursor and IME state from
  the last actual UI pass. A timer tick no longer alternates the native hover
  cursor with the default arrow or resets text composition/caret state. IME
  interruption actions are not replayed, while clipboard and native viewport
  commands produced by application logic remain live.
- Root UI animation prediction has its own clock, updated only by actual
  `run_ui` frames. Multiple CPU-only `run_logic` wakes consume egui's repaint
  history; without a separate UI prediction this made `stable_dt` fall back to
  16.667ms even while a 280Hz window painted every 3.57ms. The clock is seeded
  from native monitor refresh when available and learns the real interval of
  continuous, immediately requested UI frames. Long pauses and 100ms playback
  timers retain the last continuous cadence instead of becoming animation
  steps. No additional repaint requests, timer, FPS cap or vsync change is used.

Headless regressions cover accepted/canceled background close, fresh close
logic after an earlier timer tick, three separate external wakes and repeated
100ms deadlines without any UI pass, and no double logic when painting resumes.
`native/run.rs` checks redraw/logic and hidden bootstrap routing; Glow checks
Wayland swap policy while preserving Windows/X11 vsync.
`native/first_frame.rs` checks present-before-show, failure/retry, intentional
hidden startup, and fresh reveal state for a recreated window without a GPU.
`native/windows_startup.rs` additionally checks hidden/non-maximized constructor
attributes, cloak-before-maximize ordering, normal restore dimensions, rejected
cloak startup, ownership during setup failure and hidden-paint uncloak behavior.
`native/ui_frame_clock.rs` and an actual egui integration regression cover
60/120/144/240/280/360Hz, three CPU-only wakes per UI frame, discarded UI passes
and the first wheel event after playback-only idle timers.

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

- Linux: `set_wayland_activation_token(token)` stores the activation token of
  the application that launched a second VK Fast process. The next native
  Glow window is created with it through winit's safe startup-notify API
  (`WindowAttributesExtStartupNotify::with_activation_token`), so KWin,
  Mutter and other Wayland compositors focus the re-created window. No unsafe
  code and no new dependencies.
