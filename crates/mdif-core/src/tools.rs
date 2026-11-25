//! Where the programs we run are, on each OS: the system's own at fixed
//! places (on Windows asked of the system, never of an environment variable
//! or `PATH`, which another program may have changed), and the ones a user installs (git, Docker) where their
//! installers put them, or on `PATH` where that is the way.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// `curl`, the system's own: it speaks TLS with the system's trust store.
pub fn curl() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/bin/curl")
    }
    #[cfg(windows)]
    {
        crate::os::sys::system_directory().join("curl.exe")
    }
}

/// A command that opens a URL or a file with what the user has for it; add
/// the URL or path with `.arg`.
pub fn opener() -> Command {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
    }
    #[cfg(windows)]
    {
        let mut c = Command::new(crate::os::sys::system_directory().join("rundll32.exe"));
        c.arg("url.dll,FileProtocolHandler");
        c
    }
}

/// The first `exe` in a folder of `PATH`. Never the current folder, which
/// Windows would otherwise search first.
pub fn on_path(exe: &str) -> Option<PathBuf> {
    find_in(&std::env::var_os("PATH")?, exe)
}

fn find_in(path: &OsStr, exe: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

/// A path under `%ProgramFiles%`, where installers put programs: none unless
/// that is an absolute folder (a relative one would be searched from the
/// current folder).
#[cfg(windows)]
pub fn program_files(rest: &str) -> Option<PathBuf> {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_found_in_the_folders_of_the_path() {
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let exe = if cfg!(windows) { "tool.exe" } else { "tool" };
        std::fs::write(two.path().join(exe), "").unwrap();
        let path = std::env::join_paths([one.path(), two.path()]).unwrap();
        assert_eq!(find_in(&path, exe), Some(two.path().join(exe)));
        assert_eq!(find_in(&path, "no-such-program-here"), None);
    }

    #[test]
    fn curl_is_where_the_system_keeps_it() {
        assert!(curl().is_file(), "{}", curl().display());
        assert!(!opener().get_program().is_empty());
    }
}
