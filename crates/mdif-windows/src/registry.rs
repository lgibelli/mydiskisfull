//! The registry: the current user's values (text, binary, numbers) and keys,
//! and policies under `HKEY_LOCAL_MACHINE`, read only. What starts at login
//! (`startup`) and what a tweak changes (Storage Sense's numbers,
//! `sys::user_number`) both go through it. A value of another type than
//! asked for is never read as one.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStringExt;

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE,
    KEY_SET_VALUE, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};

fn wide(s: &str) -> Vec<u16> {
    crate::wide(OsStr::new(s))
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Key {
    /// A key under `HKEY_CURRENT_USER`; `None` if it doesn't exist.
    fn open(subkey: &str, access: u32) -> io::Result<Option<Key>> {
        Key::open_in(HKEY_CURRENT_USER, subkey, access)
    }

    /// A key under `root`; `None` if it doesn't exist.
    fn open_in(root: HKEY, subkey: &str, access: u32) -> io::Result<Option<Key>> {
        let mut h: HKEY = std::ptr::null_mut();
        let name = wide(subkey);
        // SAFETY: `name` is NUL-ended; `h` is written on success only.
        let code = unsafe { RegOpenKeyExW(root, name.as_ptr(), 0, access, &mut h) };
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
            if code == ERROR_MORE_DATA {
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

    /// The names of the keys right under it.
    fn subkeys(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        // A key's name is at most 255 characters.
        let mut buf = [0u16; 256];
        loop {
            let mut len = buf.len() as u32;
            // SAFETY: the key is open for listing; `buf` holds `len` characters.
            let code = unsafe {
                RegEnumKeyExW(
                    self.0,
                    names.len() as u32,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if code == ERROR_NO_MORE_ITEMS {
                return Ok(names);
            }
            check(code)?;
            names.push(
                OsString::from_wide(&buf[..len as usize])
                    .to_string_lossy()
                    .into_owned(),
            );
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
    text(Key::open(subkey, KEY_QUERY_VALUE)?, name)
}

/// A text value under `HKEY_LOCAL_MACHINE\\key`, if there is one.
pub fn machine_text(key: &str, name: &str) -> io::Result<Option<String>> {
    text(
        Key::open_in(HKEY_LOCAL_MACHINE, key, KEY_QUERY_VALUE)?,
        name,
    )
}

fn text(key: Option<Key>, name: &str) -> io::Result<Option<String>> {
    let Some(key) = key else {
        return Ok(None);
    };
    let Some((kind, bytes)) = key.get(name)? else {
        return Ok(None);
    };
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return Ok(None);
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
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

/// Writes a binary value, making the key if need be. Only tests write one
/// (Task Manager writes `StartupApproved`).
#[cfg(test)]
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

/// Removes a key and everything under it; whether there was one. For
/// tests' scratch keys.
#[cfg(test)]
pub fn delete_tree(subkey: &str) -> io::Result<bool> {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let name = wide(subkey);
    // SAFETY: `name` is NUL-ended.
    let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    check(code)?;
    Ok(true)
}

/// A number (`REG_DWORD`) under `HKEY_CURRENT_USER\key`, if it's there.
pub fn user_number(key: &str, value: &str) -> io::Result<Option<u32>> {
    number(Key::open(key, KEY_QUERY_VALUE)?, value)
}

/// A number under `HKEY_LOCAL_MACHINE\key` (a policy), if it's there.
pub fn machine_number(key: &str, value: &str) -> io::Result<Option<u32>> {
    number(
        Key::open_in(HKEY_LOCAL_MACHINE, key, KEY_QUERY_VALUE)?,
        value,
    )
}

fn number(key: Option<Key>, value: &str) -> io::Result<Option<u32>> {
    let Some(key) = key else {
        return Ok(None);
    };
    match key.get(value)? {
        None => Ok(None),
        Some((REG_DWORD, bytes)) if bytes.len() == 4 => Ok(Some(u32::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{value} holds something other than a number"),
        )),
    }
}

/// Sets the number `value` under `HKEY_CURRENT_USER\key` to `to`, making the
/// key if need be, or removes the value (`None`; one that isn't there is
/// already as asked). The key itself stays.
pub fn set_user_number(key: &str, value: &str, to: Option<u32>) -> io::Result<()> {
    match to {
        Some(n) => Key::create(key)?.set(value, REG_DWORD, &n.to_le_bytes()),
        None => delete_value(key, value).map(|_| ()),
    }
}

/// The names of the keys right under `HKEY_CURRENT_USER\key` (none when it
/// isn't there).
pub fn user_subkeys(key: &str) -> io::Result<Vec<String>> {
    match Key::open(key, KEY_ENUMERATE_SUB_KEYS)? {
        Some(k) => k.subkeys(),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of the test's own right under `HKEY_CURRENT_USER\Software`,
    /// removed with everything in it when dropped: the tests never touch a
    /// real setting.
    struct Scratch(String);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(&self.0);
        }
    }

    #[test]
    fn a_number_is_set_read_and_removed_and_nothing_else_is_taken_for_one() {
        let s = Scratch(format!(
            r"Software\MyDiskIsFull-test-{}-numbers",
            std::process::id()
        ));
        let key = format!(r"{}\Parameters\Policy", s.0);
        // Nothing there yet, not even the key.
        assert_eq!(user_number(&key, "01").unwrap(), None);
        set_user_number(&key, "01", None).unwrap();
        assert_eq!(user_subkeys(&key).unwrap(), Vec::<String>::new());

        set_user_number(&key, "01", Some(1)).unwrap();
        set_user_number(&key, "256", Some(30)).unwrap();
        assert_eq!(user_number(&key, "01").unwrap(), Some(1));
        assert_eq!(user_number(&key, "256").unwrap(), Some(30));
        set_user_number(&key, "256", Some(u32::MAX)).unwrap();
        assert_eq!(user_number(&key, "256").unwrap(), Some(u32::MAX));

        // Removed, and removing it again is no error.
        set_user_number(&key, "01", None).unwrap();
        assert_eq!(user_number(&key, "01").unwrap(), None);
        set_user_number(&key, "01", None).unwrap();
        assert_eq!(user_number(&key, "256").unwrap(), Some(u32::MAX));

        // Text or bytes where a number was expected are an error, not a number.
        set_string(&key, "text", "1").unwrap();
        assert!(user_number(&key, "text").is_err());
        set_binary(&key, "bytes", &[1, 0, 0, 0]).unwrap();
        assert!(user_number(&key, "bytes").is_err());

        // Its keys, by name.
        set_user_number(&format!(r"{key}\OneDrive!1"), "02", Some(0)).unwrap();
        set_user_number(&format!(r"{key}\OneDrive!2"), "02", Some(1)).unwrap();
        let mut subkeys = user_subkeys(&key).unwrap();
        subkeys.sort();
        assert_eq!(subkeys, ["OneDrive!1", "OneDrive!2"]);

        // A policy that isn't set reads as absent.
        assert_eq!(machine_number(&key, "01").unwrap(), None);
    }
}
