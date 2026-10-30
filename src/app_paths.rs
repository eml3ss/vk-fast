//! Portable Windows data roots. Never resolve writable application state against
//! the process working directory, AppData or environment-variable home paths.

#[cfg(any(windows, test))]
use std::{
    io,
    path::{Path, PathBuf},
};

#[cfg(any(windows, test))]
fn directory_of(executable: &Path) -> io::Result<PathBuf> {
    if !executable.is_absolute() || executable.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Application directory unavailable",
        ));
    }
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Application directory unavailable"))
}

#[cfg(windows)]
pub fn executable_dir() -> io::Result<PathBuf> {
    directory_of(&std::env::current_exe()?)
}

#[cfg(windows)]
pub fn config_dir() -> io::Result<PathBuf> {
    Ok(executable_dir()?.join("config"))
}

#[cfg(windows)]
pub fn state_dir() -> io::Result<PathBuf> {
    config_dir()
}

#[cfg(windows)]
pub fn cache_dir() -> io::Result<PathBuf> {
    Ok(executable_dir()?.join("cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation() -> PathBuf {
        #[cfg(windows)]
        {
            PathBuf::from(r"C:\Music players\Музыка VK")
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/Music players/Музыка VK")
        }
    }

    #[test]
    fn portable_directories_stay_next_to_the_executable_with_spaces_and_unicode() {
        let installation = installation();
        let directory = directory_of(&installation.join("vk-fast.exe")).unwrap();
        assert_eq!(directory.join("config"), installation.join("config"));
        assert_eq!(directory.join("cache"), installation.join("cache"));
        assert_ne!(directory.join("config"), directory.join("cache"));
    }

    #[test]
    fn relative_or_missing_executable_paths_never_fall_back_to_the_working_directory() {
        for path in [
            Path::new("vk-fast.exe"),
            Path::new("./vk-fast.exe"),
            Path::new(""),
        ] {
            assert!(directory_of(path).is_err());
        }
        #[cfg(windows)]
        assert!(directory_of(Path::new(r"C:\")).is_err());
        #[cfg(not(windows))]
        assert!(directory_of(Path::new("/")).is_err());
    }
}
