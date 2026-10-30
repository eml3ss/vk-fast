//! Run only inside dbus-run-session; never touches another desktop's player.
use super::*;
use std::{process::Command as ProcessCommand, thread};

const SERVICE: &str = "org.mpris.MediaPlayer2.vk-fast";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

fn bus(args: &[&str]) -> String {
    let output = ProcessCommand::new("busctl")
        .args(["--user", "--no-pager", "--timeout=2", "--"])
        .args(args)
        .output()
        .expect("busctl is required for this explicit smoke test");
    assert!(
        output.status.success(),
        "busctl {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn property(name: &str) -> String {
    bus(&["get-property", SERVICE, PATH, PLAYER, name])
}

fn call(method: &str, args: &[&str]) {
    let mut complete = vec!["call", SERVICE, PATH, PLAYER, method];
    complete.extend(args);
    bus(&complete);
}

fn wait(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !check() {
        assert!(
            Instant::now() < deadline,
            "MPRIS operation did not settle within two seconds"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn received(media: &mut SystemMedia) -> Vec<Command> {
    let mut commands = Vec::new();
    wait(|| {
        commands.extend(media.drain_commands());
        !commands.is_empty()
    });
    commands
}

#[test]
#[ignore = "run explicitly with dbus-run-session; registers a test-only media player"]
fn isolated_mpris_roundtrip_metadata_commands_stale_seeks_and_cleanup() {
    assert_eq!(
        std::env::var("VK_FAST_MEDIA_ISOLATED_TEST").as_deref(),
        Ok("1"),
        "Use VK_FAST_MEDIA_ISOLATED_TEST=1 dbus-run-session -- cargo test --locked system_media::dbus_tests -- --ignored"
    );
    assert!(
        bus(&[
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            SERVICE
        ])
        .contains("false"),
        "The isolated bus must not already own VK Fast's name"
    );
    let ctx = eframe::egui::Context::default();
    let mut media = SystemMedia::new(None, RepaintTarget::new(&ctx)).unwrap();
    let id = "/org/vkfast/track/test_42_7";
    let mut state = Snapshot {
        track: Some(TrackMetadata {
            id: id.into(),
            title: "System media smoke test".into(),
            artist: "Artist One, Artist Two".into(),
            album: Some("Test release".into()),
            cover_url: None,
            duration: Some(Duration::from_secs(200)),
        }),
        status: Status::Playing,
        position: Duration::from_secs(12),
        volume: 0.7,
    };
    media.publish(&state).unwrap();
    wait(|| property("Metadata").contains(id));
    let metadata = property("Metadata");
    for expected in [
        "System media smoke test",
        "Artist One, Artist Two",
        "Test release",
        "200000000",
    ] {
        assert!(
            metadata.contains(expected),
            "Missing {expected}: {metadata}"
        );
    }
    assert!(
        bus(&[
            "get-property",
            SERVICE,
            PATH,
            "org.mpris.MediaPlayer2",
            "DesktopEntry"
        ])
        .contains("vk-fast")
    );
    wait(|| property("PlaybackStatus").contains("Playing"));
    wait(|| {
        property("Position")
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<i64>()
            .unwrap()
            >= 12_000_000
    });
    let position_before = property("Position")
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    thread::sleep(Duration::from_millis(25));
    let position_after = property("Position")
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!(
        position_after > position_before,
        "Playing MPRIS position did not advance between engine publications"
    );

    call("Pause", &[]);
    assert_eq!(received(&mut media), [Command::Pause]);
    state.status = Status::Paused;
    state.position = Duration::from_secs(13);
    media.publish(&state).unwrap();
    wait(|| property("PlaybackStatus").contains("Paused"));
    assert_eq!(property("Position").trim(), "x 13000000");

    call("PlayPause", &[]);
    assert_eq!(received(&mut media), [Command::Toggle]);
    call("Next", &[]);
    assert_eq!(received(&mut media), [Command::Next]);
    call("Previous", &[]);
    assert_eq!(received(&mut media), [Command::Previous]);
    call("Stop", &[]);
    assert_eq!(received(&mut media), [Command::Stop]);
    for (method, expected) in [("Raise", Command::Raise), ("Quit", Command::Quit)] {
        bus(&["call", SERVICE, PATH, "org.mpris.MediaPlayer2", method]);
        assert_eq!(received(&mut media), [expected]);
    }
    call("Seek", &["x", "-5000000"]);
    assert_eq!(received(&mut media), [Command::SeekBy(-5.0)]);

    call(
        "SetPosition",
        &["ox", "/org/vkfast/track/old_id", "5000000"],
    );
    assert!(
        media.drain_commands().is_empty(),
        "A stale track ID reached the application"
    );
    call("SetPosition", &["ox", id, "5000000"]);
    assert_eq!(
        received(&mut media),
        [Command::SetPosition {
            position: Duration::from_secs(5),
            track_id: Some(id.into())
        }]
    );
    call("SetPosition", &["ox", id, "-100"]);
    call("SetPosition", &["ox", id, "201000000"]);
    assert!(
        media.drain_commands().is_empty(),
        "Out-of-range position was accepted"
    );

    bus(&["set-property", SERVICE, PATH, PLAYER, "Volume", "d", "0.25"]);
    assert_eq!(received(&mut media), [Command::SetVolume(0.25)]);
    state.volume = 0.25;
    media.publish(&state).unwrap();
    wait(|| property("Volume").trim() == "d 0.25");
    state.position = Duration::from_millis(13200);
    media.position_changed();
    media.publish(&state).unwrap();
    wait(|| property("Position").trim() == "x 13200000");

    media.publish(&Snapshot::default()).unwrap();
    wait(|| property("PlaybackStatus").contains("Stopped"));
    wait(|| !property("Metadata").contains("System media smoke test"));
    assert!(property("Metadata").contains("/org/mpris/MediaPlayer2/TrackList/NoTrack"));
    assert_eq!(property("CanSeek").trim(), "b false");
    drop(media);
    assert!(
        bus(&[
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            SERVICE
        ])
        .contains("false")
    );
}

#[test]
#[ignore = "run explicitly with an invalid test-only DBUS_SESSION_BUS_ADDRESS"]
fn missing_session_bus_returns_an_error_instead_of_panicking_or_hanging_the_app() {
    assert_eq!(
        std::env::var("VK_FAST_MEDIA_NO_BUS_TEST").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref(),
        Ok("unix:path=/tmp/vk-fast-no-media-bus")
    );
    let ctx = eframe::egui::Context::default();
    let started = Instant::now();
    assert!(SystemMedia::new(None, RepaintTarget::new(&ctx)).is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
}
