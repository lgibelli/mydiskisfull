//! Opening the app at sign-in: a value in the user's `Run` key, as the
//! watcher's (`mdif agent install`), written with the command line's own code
//! (mdif-windows' `startup`): per user, the program's path quoted, Task
//! Manager's switch read. Turned off there, it says so; turned on here again,
//! that switch is cleared, since the user asked for it.

/// "yes", "off" (switched off in Task Manager's Startup apps), "no", or
/// "unavailable" (not on Windows: while working on the app on a Mac).
pub type State = &'static str;

#[cfg(windows)]
mod imp {
    use mdif_windows::startup;

    /// The `Run` value's name: what Task Manager's Startup apps lists.
    const NAME: &str = "MyDiskIsFull";
    /// What Windows takes of a `Run` command line.
    const RUN_COMMAND_MAX: usize = 259;

    fn command() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // The `Run` key names it as text: a name that isn't would name another file.
        let exe = exe
            .to_str()
            .ok_or("the app is in a folder whose name can't be written as text")?;
        let command = format!("\"{exe}\" --background");
        if command.encode_utf16().count() > RUN_COMMAND_MAX {
            return Err(format!(
                "Windows starts a sign-in program from a command line of at most \
                 {RUN_COMMAND_MAX} characters, and this one is longer: {command}"
            ));
        }
        Ok(command)
    }

    pub fn state() -> Result<super::State, String> {
        Ok(
            match startup::login_item(NAME).map_err(|e| e.to_string())? {
                Some(item) if item.enabled => "yes",
                Some(_) => "off",
                None => "no",
            },
        )
    }

    pub fn set(on: bool) -> Result<super::State, String> {
        if on {
            startup::add_login_item(NAME, &command()?).map_err(|e| e.to_string())?;
        } else {
            startup::remove_login_item(NAME).map_err(|e| e.to_string())?;
        }
        state()
    }

    /// Moved (installed again elsewhere): the item starts this copy. Not one
    /// switched off in Task Manager, which stays off.
    pub fn follow() {
        if let (Ok(Some(item)), Ok(command)) = (startup::login_item(NAME), command())
            && item.enabled
            && item.command != command
        {
            let _ = startup::add_login_item(NAME, &command);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn state() -> Result<super::State, String> {
        Ok("unavailable")
    }

    pub fn set(_on: bool) -> Result<super::State, String> {
        Err("Only on Windows.".into())
    }

    pub fn follow() {}
}

pub use imp::{follow, set, state};

#[tauri::command]
pub fn login_item() -> Result<State, String> {
    state()
}

#[tauri::command]
pub fn set_login_item(on: bool) -> Result<State, String> {
    set(on)
}
