//! Resolve the decoder independently from the current working directory.
//! The Windows portable archive carries an audio-only build in bin/ffmpeg.exe.
#[cfg(any(windows, test))]
use std::{ffi::OsString, path::Path};
use std::{io, process::Command};

#[cfg(any(windows, test))]
fn bundled_program(executable: &Path) -> io::Result<OsString> {
    let directory = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Executable directory unavailable")
        })?;
    Ok(directory.join("bin").join("ffmpeg.exe").into_os_string())
}

pub fn command() -> io::Result<Command> {
    #[cfg(windows)]
    {
        Ok(Command::new(bundled_program(&std::env::current_exe()?)?))
    }
    #[cfg(not(windows))]
    {
        Ok(Command::new("ffmpeg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_decoder_is_always_beside_the_executable_in_a_path_with_spaces() {
        let root = std::env::temp_dir().join("VK Fast portable");
        let executable = root.join("vk-fast.exe");
        assert_eq!(
            bundled_program(&executable).unwrap(),
            root.join("bin/ffmpeg.exe")
        );
        assert!(bundled_program(Path::new("vk-fast.exe")).is_err());
    }

    #[test]
    #[cfg(not(windows))]
    fn linux_keeps_the_existing_system_decoder() {
        assert_eq!(command().unwrap().get_program(), "ffmpeg");
    }
}
