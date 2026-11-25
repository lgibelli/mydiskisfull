//! What starts at login: the per-user `Run` key, and Task Manager's switch
//! for it (`StartupApproved`). Windows only; there is no macOS twin (a
//! LaunchAgent is another thing).

use std::io;

use crate::registry::{delete_value, get_binary, get_string, set_string};

/// The programs that start when this user logs in.
pub const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Task Manager's Startup apps: a value named like the `RUN` one, whose first
/// byte says whether the user turned it off.
pub const RUN_APPROVED: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

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
pub struct Instance(#[allow(dead_code)] std::fs::File);

/// `ERROR_SHARING_VIOLATION`: another process has the file open to itself.
const SHARING_VIOLATION: i32 = 32;

/// Takes `lock` for this process: a file nobody else may open while it's held.
/// `None` if another process has it. Windows lets go of it when the process
/// ends, however it ends, so a crash leaves nothing stale; and being a file,
/// it holds across every sign-in of the same user, not just one session.
pub fn claim(lock: &std::path::Path) -> io::Result<Option<Instance>> {
    use std::os::windows::fs::OpenOptionsExt;
    // [`is_claimed`] opens the file for a moment, which a claim at that moment
    // can't tell from a holder: asked again over a tenth of a second.
    for tries_left in (0..5).rev() {
        match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(lock)
        {
            Ok(file) => return Ok(Some(Instance(file))),
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) && tries_left > 0 => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Keeps this process's standard handles (a console, or the pipes its caller
/// reads its output from) out of the programs it starts from now on. Windows
/// gives a new process every handle that may be inherited, whatever its own
/// standard handles are, so a program left running, the watcher, would hold
/// its caller's pipe open and the caller would wait for it to end.
pub fn keep_std_handles_to_self() {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle takes one of the three constants; the handle it
        // gives is this process's own, and is only changed when it is one.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Whether some process holds `lock` ([`claim`]).
pub fn is_claimed(lock: &std::path::Path) -> bool {
    match std::fs::File::open(lock) {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(SHARING_VIOLATION),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{delete_tree, set_binary};

    /// A scratch key, removed (with what's under it) when dropped.
    struct Scratch(String);

    impl Scratch {
        fn new() -> Scratch {
            // Right under Software, so removing it leaves nothing behind.
            let id = format!(
                "Software\\MyDiskIsFullTest-{}-{:?}",
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
    fn a_lock_is_held_by_one_process_until_it_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("watcher.lock");
        assert!(!is_claimed(&lock));
        let first = claim(&lock).unwrap().expect("the first takes it");
        assert!(is_claimed(&lock));
        assert!(claim(&lock).unwrap().is_none(), "the second doesn't");
        // Still the first's after the second gave up.
        assert!(is_claimed(&lock));
        drop(first);
        assert!(!is_claimed(&lock));
        assert!(claim(&lock).unwrap().is_some());
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
