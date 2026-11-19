//! `mdif agent` on Windows: the watcher starts at login through the user's
//! `Run` key, as `mdif-watch.exe`, the same program as `mdif watch --quiet
//! --log` with no console window. The key is per user and needs no
//! administrator; Task Manager's Startup apps can switch it off, and `status`
//! says so when that is what happened.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use mdif_core::os::{proc, startup};
use mdif_core::store::Store;
use mdif_core::units::bytes;

use crate::watch_cmd;

/// The `Run` value's name: what Task Manager's Startup apps lists.
pub const NAME: &str = "MyDiskIsFull Watcher";
/// What the running watcher holds ([`startup::claim`]): a file beside its
/// history, so two watchers of one history can't run, in any session.
pub fn lock_path(db: &std::path::Path) -> PathBuf {
    db.with_extension("lock")
}

/// What Windows takes of a `Run` command line.
const RUN_COMMAND_MAX: usize = 259;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `mdif-watch.exe`, next to this program.
fn watcher_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?
        .parent()
        .context("the program has no folder")?
        .join("mdif-watch.exe");
    if !exe.is_file() {
        bail!("{} is missing: it comes with mdif.exe", exe.display());
    }
    Ok(exe)
}

/// The command line the `Run` key holds. The path is quoted whole, so a folder
/// with spaces is one program and not a program and arguments.
fn command_line(exe: &std::path::Path, interval: u64) -> String {
    format!("\"{}\" --interval {interval}", exe.display())
}

/// The program a `Run` command line starts: the quoted path, or its first word.
fn program_of(command: &str) -> Option<PathBuf> {
    let command = command.trim_start();
    let path = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split_whitespace().next()?,
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Ends the `mdif-watch` processes of this user that run `exe`: how many, and
/// what went wrong with the others.
fn stop(exe: &std::path::Path) -> (usize, Vec<String>) {
    let (mut stopped, mut failed) = (0, Vec::new());
    for p in proc::own_processes()
        .into_iter()
        .filter(|p| p.name.eq_ignore_ascii_case("mdif-watch"))
    {
        match proc::terminate_program(p.pid, exe) {
            Ok(true) => stopped += 1,
            Ok(false) => {}
            Err(e) => failed.push(format!("process {}: {e}", p.pid)),
        }
    }
    (stopped, failed)
}

pub fn install(interval: u64) -> Result<()> {
    let exe = watcher_exe()?;
    let db = watch_cmd::default_db();
    let lock = lock_path(&db);
    let command = command_line(&exe, interval);
    if command.encode_utf16().count() > RUN_COMMAND_MAX {
        bail!(
            "Windows starts a sign-in program from a command line of at most {RUN_COMMAND_MAX} \
             characters, and this one is longer: {command}\nMove mdif to a shorter folder path and \
             run `mdif agent install` from there."
        );
    }
    std::fs::create_dir_all(watch_cmd::log_path().parent().unwrap())?;
    startup::add_login_item(NAME, &command).context("writing the Run key")?;
    if startup::is_claimed(&lock) {
        println!(
            "The watcher is already running; the new settings apply from the next sign-in \
             (`mdif agent uninstall` stops it now)."
        );
    } else {
        let mut child = Command::new(&exe)
            .args(["--interval", &interval.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        // Started is when it holds its lock; one that gave up before that has
        // said why in its log.
        let up = std::time::Instant::now();
        while !startup::is_claimed(&lock) {
            if let Some(status) = child.try_wait()? {
                bail!(
                    "the watcher stopped at once ({status}); see {}",
                    watch_cmd::log_path().display()
                );
            }
            if up.elapsed() > std::time::Duration::from_secs(15) {
                bail!(
                    "the watcher didn't come up in 15 s; see {}",
                    watch_cmd::log_path().display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!(
            "Watcher installed and started: every {interval} s, history in {}.",
            db.display()
        );
    }
    println!(
        "It starts when you sign in, as {}; if you move or delete that program, run `mdif agent install` again.",
        exe.display()
    );
    println!("Check on it with `mdif agent status`, and see the forecast with `mdif forecast`.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    // The watcher that was installed, wherever this copy of mdif is.
    let installed = startup::login_item(NAME)?.and_then(|i| program_of(&i.command));
    let had = startup::remove_login_item(NAME).context("removing the Run entry")?;
    let (stopped, failed) = match installed.map(Ok).unwrap_or_else(watcher_exe) {
        // Only that program, found by where it lives, not by its name.
        Ok(exe) => stop(&exe),
        Err(_) => (0, Vec::new()),
    };
    let db = watch_cmd::default_db();
    println!(
        "{} {} The history stays in {}.",
        if had {
            "No longer starts at sign-in."
        } else {
            "It wasn't set to start at sign-in."
        },
        match (stopped, startup::is_claimed(&lock_path(&db))) {
            (0, false) => "No watcher was running.",
            (_, false) => "Watcher stopped.",
            (_, true) => "A watcher is still running.",
        },
        db.display()
    );
    if !failed.is_empty() {
        bail!("couldn't stop the watcher ({})", failed.join("; "));
    }
    Ok(())
}

pub fn status() -> Result<()> {
    match startup::login_item(NAME)? {
        Some(item) if item.enabled => println!("Starts at sign-in: yes ({})", item.command),
        Some(item) => println!(
            "Starts at sign-in: switched off in Task Manager's Startup apps ({})",
            item.command
        ),
        None => println!("Starts at sign-in: no (`mdif agent install`)"),
    }
    if startup::is_claimed(&lock_path(&watch_cmd::default_db())) {
        println!("Watcher: running");
    } else {
        println!("Watcher: not running (`mdif agent install`, or `mdif watch`)");
    }
    let db = watch_cmd::default_db();
    if db.exists() {
        let size = watch_cmd::history_bytes(&db);
        let store = Store::open(&db)?;
        let rows: i64 = store.row_counts()?.iter().map(|(_, _, n)| n).sum();
        println!("History: {} ({rows} rows) in {}", bytes(size), db.display());
    } else {
        println!("History: none yet");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_what_the_command_line_starts() {
        assert_eq!(
            program_of(r#""C:\Program Files\x\mdif-watch.exe" --interval 300"#),
            Some(PathBuf::from(r"C:\Program Files\x\mdif-watch.exe"))
        );
        assert_eq!(
            program_of(r"C:\x\mdif-watch.exe --interval 300"),
            Some(PathBuf::from(r"C:\x\mdif-watch.exe"))
        );
        assert_eq!(program_of(""), None);
        assert_eq!(program_of("\"\""), None);
    }

    #[test]
    fn the_command_line_quotes_the_program_whole() {
        let line = command_line(
            std::path::Path::new(r"C:\Program Files\x\mdif-watch.exe"),
            300,
        );
        assert_eq!(
            line,
            r#""C:\Program Files\x\mdif-watch.exe" --interval 300"#
        );
    }
}
