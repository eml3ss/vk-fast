This directory contains souvlaki 0.9.3 from crates.io (MIT; see LICENSE).
Original source: https://github.com/Sinono3/souvlaki

VK Fast uses the pure-Rust `use_zbus` backend on Linux and SMTC on Windows.
The local changes are intentionally limited to system-media correctness:

- Stable optional `MediaMetadata::track_id` and targeted MPRIS SetPosition
  events. Stale track IDs are ignored before enqueueing and again by VK Fast.
- MPRIS DesktopEntry matches the configured D-Bus player name (`vk-fast`).
- Playing Position interpolates with a monotonic clock and is bounded by the
  duration. Routine engine progress does not spam PlaybackStatusChanged.
- Committed seeks can emit Seeked, including small or paused seeks.
- Bus startup errors are reported through attach, instead of a detached panic.
- Non-finite volume values cannot pollute the MPRIS state.
- Windows metadata clears the preceding album/artwork, and detach unregisters
  the position callback as well as the media-button callback.

No media decoding, authentication, VK endpoints, or audio output was added to
this dependency. VK Fast owns playback and applies queued OS intents itself.
