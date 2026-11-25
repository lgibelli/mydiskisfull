//! The PC and the user: when it started, their folders, the system folder,
//! standard error into a log, and the background priority.

use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetTickCount64};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_MODE_BACKGROUND_BEGIN, PROCESS_MODE_BACKGROUND_END, SetPriorityClass,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Profile, SHGetKnownFolderPath,
};
use windows_sys::core::{GUID, PWSTR};

use crate::unsupported;

pub use crate::private::user_sid;
pub use crate::registry::{
    get_string as user_text, machine_number, set_user_number, user_number, user_subkeys,
};

/// Windows asks nothing before a program opens a folder (there's no TCC), so
/// there's nothing to probe.
pub fn access_probes(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// Always `Some(true)`: nothing is guarded the way macOS guards folders.
pub fn full_disk_access(_probes: &[PathBuf]) -> Option<bool> {
    Some(true)
}

/// A folder of the user's, from Windows, not from an environment variable
/// another program may have changed.
fn known_folder(id: &GUID) -> Option<PathBuf> {
    let mut path: PWSTR = null_mut();
    // SAFETY: `id` is a valid GUID; on success `path` is a NUL-ended string
    // that is ours to free with `CoTaskMemFree`, which is done on every path.
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, null_mut::<core::ffi::c_void>() as HANDLE, &mut path);
        let found = (hr >= 0 && !path.is_null()).then(|| {
            let len = (0..).take_while(|&i| *path.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
                path, len,
            )))
        });
        CoTaskMemFree(path.cast());
        found.filter(|p| p.is_absolute())
    }
}

/// The user's profile folder, `C:\Users\me`.
pub fn account_home() -> Option<PathBuf> {
    known_folder(&FOLDERID_Profile)
}

/// `C:\Users\me\AppData\Local`: per user, and not roamed to other PCs.
pub fn local_app_data() -> Option<PathBuf> {
    known_folder(&FOLDERID_LocalAppData)
}

/// Where Windows says which it is.
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Which Windows this is, as Settings → System → About says it: "11 24H2",
/// "10 22H2" (an older one without a feature update's name: its build).
pub fn windows_version() -> io::Result<String> {
    let build = registry_text(CURRENT_VERSION, "CurrentBuildNumber")
        .ok_or_else(|| io::Error::other("Windows' build number isn't in the registry"))?;
    let feature = registry_text(CURRENT_VERSION, "DisplayVersion")
        .or_else(|| registry_text(CURRENT_VERSION, "ReleaseId"));
    Ok(version_named(&build, feature.as_deref()))
}

/// "11 24H2" from the build ("26100") and the feature update's name: 11
/// from build 22000 on (its registry still says "Windows 10" elsewhere).
fn version_named(build: &str, feature: Option<&str>) -> String {
    let n: u32 = build.trim().parse().unwrap_or(0);
    let product = if n >= 22_000 { "11" } else { "10" };
    match feature.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => format!("{product} {f}"),
        None => format!("{product} (build {n})"),
    }
}

/// Windows' build (26100 for 11 24H2), if the registry says.
pub fn windows_build() -> Option<u32> {
    registry_text(CURRENT_VERSION, "CurrentBuildNumber")?
        .trim()
        .parse()
        .ok()
}

/// A text value under `HKEY_LOCAL_MACHINE`, if it's there.
fn registry_text(key: &str, value: &str) -> Option<String> {
    crate::registry::machine_text(key, value).ok().flatten()
}

/// The system folder, `C:\Windows\System32`, where Windows keeps its own
/// programs: asked of Windows, so it names the real one.
pub fn system_directory() -> PathBuf {
    let mut buf = vec![0u16; 260];
    loop {
        // SAFETY: `buf` is the length passed.
        let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            // Can't happen; the usual place, rather than none.
            return PathBuf::from(r"C:\Windows\System32");
        }
        if n < buf.len() {
            return PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
        }
        buf.resize(n + 1, 0);
    }
}

/// Points standard error at `file` for the rest of the process, so what the
/// watcher prints lands in its log, with or without a console.
pub fn redirect_stderr(file: &File) -> io::Result<()> {
    // A copy of the handle: the process's standard error must outlive `file`.
    let copy = file.try_clone()?;
    // SAFETY: the handle is valid, and stays open (below) once it's set.
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, copy.as_raw_handle() as HANDLE) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Standard error owns it now, for the rest of the process.
    let _ = copy.into_raw_handle();
    Ok(())
}

/// When the PC started, in Unix seconds.
pub fn boot_time() -> io::Result<i64> {
    let up_ms = unsafe { GetTickCount64() };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs() as i64;
    Ok(now - (up_ms / 1000) as i64)
}

/// Bytes of swap in use. Not written yet (Windows has a page file, and its
/// use isn't the same thing).
pub fn swap_used() -> io::Result<u64> {
    Err(unsupported("the swap in use"))
}

/// Seconds the local time is ahead of UTC at Unix time `ts`, daylight saving
/// included.
pub fn utc_offset(ts: i64) -> i64 {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map_or(0, |t| i64::from(t.offset().local_minus_utc()))
}

/// How many [`Background`] guards are alive: the process goes to the
/// background state with the first and back with the last.
static GUARDS: Mutex<usize> = Mutex::new(0);

/// While one lives, this process runs in the background state: the lowest
/// scheduling priority, throttled disk I/O. Dropping the last puts the
/// process back as it was.
pub struct Background(());

/// Puts this process in the background state ([`Background`]).
pub fn background() -> io::Result<Background> {
    let mut guards = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
    if *guards == 0 {
        set_background(true)?;
    }
    *guards += 1;
    Ok(Background(()))
}

/// Whether this process is in the background state through [`background`].
pub fn in_background() -> bool {
    *GUARDS.lock().unwrap_or_else(|e| e.into_inner()) > 0
}

fn set_background(on: bool) -> io::Result<()> {
    let mode = if on {
        PROCESS_MODE_BACKGROUND_BEGIN
    } else {
        PROCESS_MODE_BACKGROUND_END
    };
    // SAFETY: plain call on this process's pseudo handle.
    if unsafe { SetPriorityClass(GetCurrentProcess(), mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl Drop for Background {
    fn drop(&mut self) {
        let mut guards = GUARDS.lock().unwrap_or_else(|e| e.into_inner());
        *guards -= 1;
        if *guards == 0 {
            let _ = set_background(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_said_as_settings_says_it() {
        assert_eq!(version_named("26100", Some("24H2")), "11 24H2");
        assert_eq!(version_named("19045", Some("22H2")), "10 22H2");
        assert_eq!(version_named("17763", None), "10 (build 17763)");
        assert_eq!(version_named("22000", Some(" ")), "11 (build 22000)");
        // This PC's.
        let here = windows_version().unwrap();
        println!("Windows {here}");
        assert!(here.starts_with("10 ") || here.starts_with("11 "), "{here}");
        let build = windows_build().unwrap();
        assert_eq!(here.starts_with("11 "), build >= 22_000, "{here} {build}");
    }

    #[test]
    fn the_users_folders_and_the_system_folder_come_from_windows() {
        let home = account_home().unwrap();
        assert!(home.is_dir(), "{home:?}");
        // Usually under the profile, but it can be redirected.
        let local = local_app_data().unwrap();
        assert!(local.is_absolute() && local.is_dir(), "{local:?}");
        let system = system_directory();
        assert!(system.join("curl.exe").is_file(), "{system:?}");
    }

    #[test]
    fn nothing_is_guarded() {
        assert!(access_probes(Path::new(r"C:\Users\x")).is_empty());
        assert_eq!(full_disk_access(&[]), Some(true));
    }

    #[test]
    fn the_pc_started_in_the_past_and_time_has_an_offset() {
        let boot = boot_time().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(boot < now && now - boot < 3650 * 86_400, "{boot} {now}");
        assert!(utc_offset(now).abs() <= 14 * 3600);
    }

    #[test]
    fn the_background_state_is_held_while_it_lives() {
        assert!(!in_background());
        {
            let _b = background().unwrap();
            assert!(in_background());
            // Nested: the outer one keeps it.
            drop(background().unwrap());
            assert!(in_background());
        }
        assert!(!in_background());
    }

    #[test]
    fn what_isnt_written_yet_says_so() {
        assert_eq!(swap_used().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
