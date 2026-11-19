//! What starts at login: the per-user `Run` key, and Task Manager's switch
//! for it (`StartupApproved`). Windows only; there is no macOS twin (a
//! LaunchAgent is another thing).

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW};

/// The right to wait on an object (`SYNCHRONIZE`), enough to see a mutex exists.
const SYNCHRONIZE: u32 = 0x0010_0000;

/// The programs that start when this user logs in.
pub const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Task Manager's Startup apps: a value named like the `RUN` one, whose first
/// byte says whether the user turned it off.
pub const RUN_APPROVED: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

/// An open key under `HKEY_CURRENT_USER`, closed on drop.
struct Key(HKEY);

impl Key {
    /// `None` if the key doesn't exist.
    fn open(subkey: &str, access: u32) -> io::Result<Option<Key>> {
        let mut h: HKEY = std::ptr::null_mut();
        let name = wide(subkey);
        // SAFETY: `name` is NUL-ended; `h` is written on success only.
        let code = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, access, &mut h) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        Ok(Some(Key(h)))
    }

    /// The key, made if it isn't there.
    fn create(subkey: &str) -> io::Result<Key> {
        let mut h: HKEY = std::ptr::null_mut();
        let name = wide(subkey);
        // SAFETY: `name` is NUL-ended; `h` is written on success only.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null(),
                &mut h,
                std::ptr::null_mut(),
            )
        };
        check(code)?;
        Ok(Key(h))
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> io::Result<()> {
        let name = wide(name);
        // SAFETY: the key is open for writing; `data` is as long as passed.
        check(unsafe {
            RegSetValueExW(
                self.0,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr(),
                u32::try_from(data.len()).map_err(io::Error::other)?,
            )
        })
    }

    /// The value's type and bytes, or `None` if there is none.
    fn get(&self, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
        let name = wide(name);
        let mut kind: REG_VALUE_TYPE = 0;
        let mut len = 0u32;
        // SAFETY: a null buffer asks only for the size.
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        // The value can grow between the calls: ask until it fits.
        loop {
            let mut buf = vec![0u8; len as usize + 2];
            let mut got = buf.len() as u32;
            // SAFETY: `buf` is `got` long.
            let code = unsafe {
                RegQueryValueExW(
                    self.0,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    buf.as_mut_ptr(),
                    &mut got,
                )
            };
            // ERROR_MORE_DATA
            if code == 234 {
                len = got;
                continue;
            }
            if code == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            check(code)?;
            buf.truncate(got as usize);
            return Ok(Some((kind, buf)));
        }
    }

    fn delete(&self, name: &str) -> io::Result<bool> {
        let name = wide(name);
        // SAFETY: the key is open for writing.
        let code = unsafe { RegDeleteValueW(self.0, name.as_ptr()) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(code)?;
        Ok(true)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key is ours, and closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// A text value under `subkey` of the current user's registry, if there is one.
pub fn get_string(subkey: &str, name: &str) -> io::Result<Option<String>> {
    let Some(key) = Key::open(subkey, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    let Some((kind, bytes)) = key.get(name)? else {
        return Ok(None);
    };
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Ok(None);
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)))
}

/// Writes a text value, making the key if need be.
pub fn set_string(subkey: &str, name: &str, value: &str) -> io::Result<()> {
    let bytes: Vec<u8> = value
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    Key::create(subkey)?.set(name, REG_SZ, &bytes)
}

/// A binary value, if there is one.
pub fn get_binary(subkey: &str, name: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(key) = Key::open(subkey, KEY_QUERY_VALUE)? else {
        return Ok(None);
    };
    Ok(key
        .get(name)?
        .and_then(|(kind, bytes)| (kind == REG_BINARY).then_some(bytes)))
}

/// Writes a binary value, making the key if need be.
pub fn set_binary(subkey: &str, name: &str, value: &[u8]) -> io::Result<()> {
    Key::create(subkey)?.set(name, REG_BINARY, value)
}

/// Removes a value; whether there was one.
pub fn delete_value(subkey: &str, name: &str) -> io::Result<bool> {
    match Key::open(subkey, KEY_SET_VALUE)? {
        Some(key) => key.delete(name),
        None => Ok(false),
    }
}

/// Removes a key and everything under it; whether there was one.
pub fn delete_tree(subkey: &str) -> io::Result<bool> {
    let name = wide(subkey);
    // SAFETY: `name` is NUL-ended.
    let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    check(code)?;
    Ok(true)
}

/// A program that starts at login, as the user's `Run` key and Task Manager
/// have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItem {
    /// The command line it runs.
    pub command: String,
    /// False once the user has turned it off in Task Manager's Startup apps
    /// (Windows then keeps the `Run` value and doesn't start it).
    pub enabled: bool,
}

/// Whether `Run` is switched off by the first byte of the `StartupApproved`
/// value: its low bit is set when the user disabled it (02 is on, 03 is off).
fn approved(value: Option<&[u8]>) -> bool {
    value.and_then(|v| v.first()).is_none_or(|b| b & 1 == 0)
}

/// Starts `command` at every login of this user under `name`, replacing an
/// earlier `name`. The user asked for it, so a switch they flipped off in
/// Task Manager for the old one is cleared.
pub fn add_login_item(name: &str, command: &str) -> io::Result<()> {
    add_login_item_in(RUN, RUN_APPROVED, name, command)
}

/// The login item `name`, if there is one.
pub fn login_item(name: &str) -> io::Result<Option<LoginItem>> {
    login_item_in(RUN, RUN_APPROVED, name)
}

/// Stops starting `name` at login; whether it was there.
pub fn remove_login_item(name: &str) -> io::Result<bool> {
    remove_login_item_in(RUN, RUN_APPROVED, name)
}

fn add_login_item_in(run: &str, approved_key: &str, name: &str, command: &str) -> io::Result<()> {
    set_string(run, name, command)?;
    delete_value(approved_key, name)?;
    Ok(())
}

fn login_item_in(run: &str, approved_key: &str, name: &str) -> io::Result<Option<LoginItem>> {
    let Some(command) = get_string(run, name)? else {
        return Ok(None);
    };
    let flag = get_binary(approved_key, name)?;
    Ok(Some(LoginItem {
        command,
        enabled: approved(flag.as_deref()),
    }))
}

fn remove_login_item_in(run: &str, approved_key: &str, name: &str) -> io::Result<bool> {
    let had = delete_value(run, name)?;
    delete_value(approved_key, name)?;
    Ok(had)
}

/// Being the one process of its kind: held for as long as it lives.
pub struct Instance(HANDLE);

// SAFETY: a mutex handle may be closed from any thread.
unsafe impl Send for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle is ours, and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Takes the name `name` (in this logon session) for this process: `None` if
/// another process has it. Windows frees it when the process ends, however it
/// ends, so a crash leaves nothing stale.
pub fn claim(name: &str) -> io::Result<Option<Instance>> {
    let wide = wide(&format!("Local\\{name}"));
    // SAFETY: `wide` is NUL-ended; the last error is read at once.
    let (h, err) = unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        (h, io::Error::last_os_error())
    };
    if h.is_null() {
        return Err(err);
    }
    let mine = Instance(h);
    if err.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
        // Another has it: this handle is only a second one on the same mutex.
        return Ok(None);
    }
    Ok(Some(mine))
}

/// Whether some process holds `name` ([`claim`]).
pub fn is_claimed(name: &str) -> bool {
    let wide = wide(&format!("Local\\{name}"));
    // SAFETY: `wide` is NUL-ended; the handle, if any, is closed at once.
    let h = unsafe { OpenMutexW(SYNCHRONIZE, 0, wide.as_ptr()) };
    if h.is_null() {
        return false;
    }
    // SAFETY: the handle is ours.
    unsafe { CloseHandle(h) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch key, removed (with what's under it) when dropped.
    struct Scratch(String);

    impl Scratch {
        fn new() -> Scratch {
            let id = format!(
                "Software\\MyDiskIsFullTest\\{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            );
            delete_tree(&id).unwrap();
            Scratch(id)
        }
        fn sub(&self, name: &str) -> String {
            format!("{}\\{name}", self.0)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(&self.0);
        }
    }

    #[test]
    fn text_and_binary_values_round_trip_and_a_missing_one_is_none() {
        let s = Scratch::new();
        let key = s.sub("a");
        assert_eq!(get_string(&key, "v").unwrap(), None);
        set_string(
            &key,
            "v",
            r#""C:\Program Files\x y\mdif-watch.exe" --interval 300"#,
        )
        .unwrap();
        assert_eq!(
            get_string(&key, "v").unwrap().as_deref(),
            Some(r#""C:\Program Files\x y\mdif-watch.exe" --interval 300"#)
        );
        // Text with characters outside ASCII.
        set_string(&key, "v", "Gelöscht ✓").unwrap();
        assert_eq!(
            get_string(&key, "v").unwrap().as_deref(),
            Some("Gelöscht ✓")
        );
        // A binary value isn't text, and a text one isn't binary.
        set_binary(&key, "b", &[3, 0, 0, 0]).unwrap();
        assert_eq!(get_binary(&key, "b").unwrap(), Some(vec![3, 0, 0, 0]));
        assert_eq!(get_string(&key, "b").unwrap(), None);
        assert_eq!(get_binary(&key, "v").unwrap(), None);
        assert!(delete_value(&key, "v").unwrap());
        assert!(!delete_value(&key, "v").unwrap());
        assert!(!delete_value(&s.sub("nokey"), "v").unwrap());
    }

    #[test]
    fn a_name_is_held_by_one_process_until_it_lets_go() {
        let name = format!("MyDiskIsFullTest-{}", std::process::id());
        assert!(!is_claimed(&name));
        let first = claim(&name).unwrap().expect("the first takes it");
        assert!(is_claimed(&name));
        assert!(claim(&name).unwrap().is_none(), "the second doesn't");
        // Still the first's after the second let go of its handle.
        assert!(is_claimed(&name));
        drop(first);
        assert!(!is_claimed(&name));
        assert!(claim(&name).unwrap().is_some());
    }

    #[test]
    fn task_managers_switch_is_the_low_bit_of_the_first_byte() {
        assert!(approved(None));
        assert!(approved(Some(&[2, 0, 0, 0])));
        assert!(approved(Some(&[6, 0, 0, 0])));
        assert!(!approved(Some(&[3, 0, 0, 0])));
        assert!(!approved(Some(&[1, 0, 0, 0])));
        assert!(approved(Some(&[])));
    }

    #[test]
    fn a_login_item_is_added_seen_switched_off_and_removed() {
        let s = Scratch::new();
        let (run, ok) = (s.sub("Run"), s.sub("Approved"));
        assert_eq!(login_item_in(&run, &ok, "mdif").unwrap(), None);
        add_login_item_in(&run, &ok, "mdif", r"C:\a\mdif-watch.exe").unwrap();
        let item = login_item_in(&run, &ok, "mdif").unwrap().unwrap();
        assert_eq!(item.command, r"C:\a\mdif-watch.exe");
        assert!(item.enabled);
        // The user turns it off in Task Manager.
        set_binary(&ok, "mdif", &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!(!login_item_in(&run, &ok, "mdif").unwrap().unwrap().enabled);
        // Asking for it again is asking for it on.
        add_login_item_in(&run, &ok, "mdif", r"C:\b\mdif-watch.exe").unwrap();
        let item = login_item_in(&run, &ok, "mdif").unwrap().unwrap();
        assert_eq!(item.command, r"C:\b\mdif-watch.exe");
        assert!(item.enabled);
        assert!(remove_login_item_in(&run, &ok, "mdif").unwrap());
        assert!(!remove_login_item_in(&run, &ok, "mdif").unwrap());
        assert_eq!(login_item_in(&run, &ok, "mdif").unwrap(), None);
    }
}
