//! Audio-thread diagnostics use only atomics and borrowed error text. Formatting
//! and log delivery happen on the output owner thread.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct Status {
    pub started: AtomicBool,
    failure: AtomicU64,
}

pub(super) fn native_code(message: &str) -> Option<u32> {
    if let Some(start) = message.find("0x").or_else(|| message.find("0X")) {
        let mut value = 0u32;
        let mut count = 0;
        for byte in message.as_bytes()[start + 2..].iter().copied() {
            if byte == b'_' {
                continue;
            }
            let digit = char::from(byte).to_digit(16);
            let Some(digit) = digit else {
                break;
            };
            if count == 8 {
                break;
            }
            value = (value << 4) | digit;
            count += 1;
        }
        if count == 8 {
            return Some(value);
        }
    }
    let start = message.find("os error ")? + "os error ".len();
    let end = message[start..]
        .find(|character: char| !character.is_ascii_digit() && character != '-')
        .map_or(message.len(), |end| start + end);
    message[start..end]
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= i64::from(i32::MIN) && *value <= i64::from(u32::MAX))
        .map(|value| value as u32)
}

pub(super) fn code_text(code: Option<u32>) -> String {
    code.map_or_else(
        || "unavailable".into(),
        |code| format!("0x{:04x}_{:04x}", code >> 16, code & 0xffff),
    )
}

impl Status {
    pub(super) fn failed(&self) -> bool {
        self.failure.load(Ordering::Acquire) != 0
    }

    pub(super) fn fail(&self, kind: u8, message: Option<&str>) -> bool {
        let code = message
            .and_then(native_code)
            .map_or(0, |code| (1u64 << 32) | u64::from(code));
        self.failure
            .compare_exchange(
                0,
                (u64::from(kind) << 33) | code,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn diagnostic(&self) -> String {
        let encoded = self.failure.load(Ordering::Acquire);
        let code = (encoded & (1u64 << 32) != 0).then_some(encoded as u32);
        let kind = match encoded >> 33 {
            1 => "DeviceNotAvailable",
            2 => "StreamInvalidated",
            3 => "DeviceBusy",
            4 => "UnsupportedConfig",
            5 => "PermissionDenied",
            7 => "HostUnavailable",
            8 => "ResourceExhausted",
            9 => "UnsupportedOperation",
            10 => "InvalidInput",
            11 => "Other",
            _ => "BackendError",
        };
        format!("kind={kind} native={}", code_text(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_hresult_survives_hex_decimal_and_missing_error_details() {
        for message in [
            "Initialize failed (0x88890008)",
            "Initialize failed 0x8889_0008",
            "Failed (os error -2004287480)",
        ] {
            assert_eq!(native_code(message), Some(0x88890008), "{message}");
        }
        assert_eq!(native_code("No device"), None);
        assert_eq!(native_code("os error 13"), Some(13));
        assert_eq!(native_code("os error 9999999999999999999"), None);
        let status = Status::default();
        assert!(!status.failed());
        assert!(status.fail(3, Some("failed 0x88890008")));
        assert!(!status.fail(4, Some("failed 0x80070005")));
        assert!(status.failed());
        assert_eq!(status.diagnostic(), "kind=DeviceBusy native=0x8889_0008");
        assert!(!status.started.load(Ordering::Relaxed));
    }

    #[test]
    fn concurrent_errors_publish_one_complete_diagnostic() {
        let status = std::sync::Arc::new(Status::default());
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let status = status.clone();
                std::thread::spawn(move || {
                    if index % 2 == 0 {
                        status.fail(3, Some("failed 0x8889000a"));
                    } else {
                        status.fail(5, Some("failed 0x80070005"));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(
            [
                "kind=DeviceBusy native=0x8889_000a",
                "kind=PermissionDenied native=0x8007_0005"
            ]
            .contains(&status.diagnostic().as_str())
        );
    }
}
