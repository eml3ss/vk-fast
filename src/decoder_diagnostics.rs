//! Drain decoder stderr independently of playback. Only the bounded prefix of
//! its records is reported; the pipe is always drained until EOF so logging
//! cannot leave ffmpeg waiting for a full stderr pipe.
use std::{
    borrow::Cow,
    io::{self, Read},
    process::ChildStderr,
    thread,
};

const LINE_BYTES: usize = 2048;
const RECORDS: usize = 64;
const LONG_LINE: &str = "stderr line omitted (length limit)";
const RECORD_LIMIT: &str = "remaining stderr omitted (record limit)";

/// Call after spawning with piped stderr when diagnostics are enabled. This
/// returns after thread creation; it never waits for the decoder or its output.
pub fn capture(stderr: ChildStderr) -> io::Result<()> {
    thread::Builder::new()
        .name("vk-ffmpeg-stderr".into())
        .spawn(move || {
            if drain(stderr, |line| crate::diagnostics::event("ffmpeg", line)).is_err() {
                crate::diagnostics::event("ffmpeg", "stderr read failed");
            }
        })
        .map(drop)
}

fn drain(mut input: impl Read, mut emit: impl FnMut(&str)) -> io::Result<()> {
    let mut chunk = [0_u8; 4096];
    let mut line = [0_u8; LINE_BYTES];
    let mut length = 0;
    let mut overflow = false;
    let mut records = 0;
    loop {
        let count = match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        for &byte in &chunk[..count] {
            if matches!(byte, b'\n' | b'\r') {
                emit_line(&line[..length], overflow, &mut records, &mut emit);
                length = 0;
                overflow = false;
            } else if records <= RECORDS {
                if length < line.len() {
                    line[length] = byte;
                    length += 1;
                } else {
                    overflow = true;
                }
            }
        }
    }
    emit_line(&line[..length], overflow, &mut records, &mut emit);
    Ok(())
}

fn emit_line(bytes: &[u8], overflow: bool, records: &mut usize, emit: &mut impl FnMut(&str)) {
    if bytes.is_empty() && !overflow {
        return;
    }
    if *records == RECORDS {
        emit(RECORD_LIMIT);
        *records += 1;
    } else if *records < RECORDS {
        // Discard an overlong line as a whole, rather than passing a fragment
        // that could cut a URL/credential marker away from its private value.
        if overflow {
            emit(LONG_LINE);
        } else {
            let line = String::from_utf8_lossy(bytes);
            let line = line.trim();
            if line.is_empty() {
                return;
            }
            // The production sink is diagnostics::event, which strips URLs,
            // credentials, paths and IDs before anything is queued or written.
            emit(&group_error_codes(line));
        }
        *records += 1;
    }
}

fn group_error_codes(line: &str) -> Cow<'_, str> {
    let mut grouped = None;
    let mut copied = 0;
    let mut offset = 0;
    for word in line.split_inclusive(char::is_whitespace) {
        // Leave URL words intact for the logger's URL filter. In particular,
        // never split a query value into another word while formatting a code.
        if !word.contains("://") && !word.to_ascii_lowercase().starts_with("www.") {
            let bytes = word.as_bytes();
            for index in 0..bytes.len().saturating_sub(9) {
                if bytes[index..].starts_with(b"0x")
                    && bytes[index + 2..index + 10]
                        .iter()
                        .all(u8::is_ascii_hexdigit)
                    && (index == 0
                        || (!bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_'))
                    && (index + 10 == bytes.len()
                        || (!bytes[index + 10].is_ascii_alphanumeric()
                            && bytes[index + 10] != b'_'))
                {
                    // A fixed 32-bit OS error such as 0x80090304 must survive
                    // the logger's redaction of runs of five or more digits.
                    let middle = offset + index + 6;
                    let output = grouped.get_or_insert_with(|| String::with_capacity(line.len()));
                    output.push_str(&line[copied..middle]);
                    output.push('_');
                    copied = middle;
                }
            }
        }
        offset += word.len();
    }
    match grouped {
        Some(mut output) => {
            output.push_str(&line[copied..]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct SmallReads<R> {
        input: R,
        interrupted: bool,
    }

    impl<R: Read> Read for SmallReads<R> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = output.len().min(3);
            self.input.read(&mut output[..count])
        }
    }

    #[test]
    fn chunk_boundaries_crlf_interrupted_reads_and_final_line_are_handled() {
        let input = SmallReads {
            input: Cursor::new(
                b"\r\n  Connection refused\r\nTLS failed\r\n\xffinvalid utf8\nlast error",
            ),
            interrupted: false,
        };
        let mut records = Vec::new();
        drain(input, |line| records.push(line.to_owned())).unwrap();
        assert_eq!(
            records,
            [
                "Connection refused",
                "TLS failed",
                "\u{fffd}invalid utf8",
                "last error"
            ]
        );
    }

    #[test]
    fn multi_megabyte_lines_use_fixed_storage_and_do_not_expose_fragments() {
        let private_prefix = Cursor::new(b"password=private-fragment ");
        let huge_line = io::repeat(b'x').take(8 * 1024 * 1024);
        let tail = Cursor::new(b"\nInvalid data found when processing input\n");
        let mut input = private_prefix.chain(huge_line).chain(tail);
        let mut records = Vec::new();
        drain(&mut input, |line| records.push(line.to_owned())).unwrap();
        assert_eq!(
            records,
            [LONG_LINE, "Invalid data found when processing input"]
        );
        assert!(records.iter().all(|line| line.len() <= LINE_BYTES));
        assert!(!records.join(" ").contains("private-fragment"));
        assert_eq!(input.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn record_limit_does_not_stop_pipe_draining() {
        let input = "decoder error\r\n".repeat(RECORDS * 100) + "final omitted error";
        let expected_length = input.len() as u64;
        let mut input = Cursor::new(input);
        let mut records = Vec::new();
        drain(&mut input, |line| records.push(line.to_owned())).unwrap();
        assert_eq!(input.position(), expected_length);
        assert_eq!(records.len(), RECORDS + 1);
        assert!(
            records[..RECORDS]
                .iter()
                .all(|line| line == "decoder error")
        );
        assert_eq!(records.last().unwrap(), RECORD_LIMIT);
    }

    #[test]
    fn read_error_is_returned_without_logging_its_private_payload() {
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("password=private-io-payload"))
            }
        }
        let mut records = Vec::new();
        assert!(drain(Failed, |line| records.push(line.to_owned())).is_err());
        assert!(records.is_empty());
    }

    #[test]
    fn schannel_error_code_survives_as_grouped_hex_without_changing_pointer_addresses() {
        let input =
            "[tls @ 0x000001df3abbc6c0] schannel: Creating security context failed (0x80090304)\n";
        let mut records = Vec::new();
        drain(Cursor::new(input), |line| records.push(line.to_owned())).unwrap();
        assert_eq!(
            records,
            ["[tls @ 0x000001df3abbc6c0] schannel: Creating security context failed (0x8009_0304)"]
        );
        assert_eq!(
            group_error_codes("0x80090304, 0xDEADBEEF 0x8009_0304 0x1234 0x123456789 A0x80090304"),
            "0x8009_0304, 0xDEAD_BEEF 0x8009_0304 0x1234 0x123456789 A0x80090304"
        );
    }

    #[test]
    fn hex_grouping_keeps_private_markers_intact_for_the_log_sanitizer() {
        let url = "https://vk.example/audio?code=0x80090304&token=private-url-value";
        let input = format!(
            "TLS failed (0x80090304) {url} access_token=private-token account_id=123456789 /home/friend/private.txt C:\\Users\\Friend\\private.txt"
        );
        let grouped = group_error_codes(&input);
        assert!(grouped.contains("(0x8009_0304)"));
        assert!(grouped.contains(url));
        assert!(grouped.contains("access_token=private-token"));
        assert!(grouped.contains("account_id=123456789"));
        assert!(grouped.contains("/home/friend/private.txt"));
        assert!(grouped.contains("C:\\Users\\Friend\\private.txt"));
        assert_eq!(
            grouped.split_whitespace().count(),
            input.split_whitespace().count()
        );
    }
}
