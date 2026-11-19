//! `mdif agent` on Windows: the watcher starts at login through the user's
//! `Run` key, as `mdif-watch.exe`, the same program as `mdif watch --quiet
//! --log` with no console window. The key is per user and needs no
//! administrator; Task Manager's Startup apps can switch it off, and `status`
//! says so when that is what happened.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use mdif_core::os::startup;
use mdif_core::store::Store;
use mdif_core::units::bytes;

use crate::watch_cmd;

/// The `Run` value's name: what Task Manager's Startup apps lists.
pub const NAME: &str = "MyDiskIsFull Watcher";
/// What the running watcher holds ([`startup::claim`]): a file beside its
/// history, so two watchers of one history can't run, in any session.
pub fn lock_path(db: &std::path::Path) -> PathBuf {
    beside(db, ".lock")
}

/// `<db file name><suffix>` in the history's folder: the whole name, so
/// `work.db` and `work.sqlite3` don't share one, and never the history itself.
fn beside(db: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
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

/// The file whose appearance asks the watcher of `db` to finish: it looks at
/// it each second between samples, and goes when it is there. No process is
/// ended from outside, so nothing but the watcher itself ever has to be
/// identified.
pub fn stop_path(db: &std::path::Path) -> PathBuf {
    beside(db, ".stop")
}

/// Whether a stop has been asked for.
pub fn stop_asked(db: &std::path::Path) -> bool {
    stop_path(db).exists()
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
        let began = mdif_core::watch::unix_now() - 1;
        let mut child = Command::new(&exe)
            .args(["--interval", &interval.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        // Started is when its first sample is in the history; one that gave up
        // before that has said why in its log.
        let key = mdif_core::os::fs::statfs(&mdif_core::map::boot_volume())
            .map(|s| s.history_key())
            .context("looking at the disk")?;
        let up = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                bail!(
                    "the watcher stopped at once ({status}); see {}",
                    watch_cmd::log_path().display()
                );
            }
            let sampled = db.exists()
                && Store::open(&db)
                    .and_then(|s| s.space_since(&key, began))
                    .is_ok_and(|pts| !pts.is_empty());
            if sampled {
                break;
            }
            if up.elapsed() > std::time::Duration::from_secs(30) {
                bail!(
                    "the watcher took no sample in 30 s; see {}",
                    watch_cmd::log_path().display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
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
    let db = watch_cmd::default_db();
    let lock = lock_path(&db);
    let had = startup::remove_login_item(NAME).context("removing the Run entry")?;
    let mut stopped = false;
    if startup::is_claimed(&lock) {
        // Ask it to finish, and wait until it has let go of its history.
        std::fs::write(stop_path(&db), b"stop").with_context(|| {
            format!("asking the watcher to stop ({})", stop_path(&db).display())
        })?;
        let asked = std::time::Instant::now();
        while startup::is_claimed(&lock) && asked.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        stopped = !startup::is_claimed(&lock);
        // A request left behind would stop the next watcher at once.
        let _ = std::fs::remove_file(stop_path(&db));
    }
    println!(
        "{} {} The history stays in {}.",
        if had {
            "No longer starts at sign-in."
        } else {
            "It wasn't set to start at sign-in."
        },
        match (stopped, startup::is_claimed(&lock)) {
            (false, false) => "No watcher was running.",
            (true, _) => "Watcher stopped.",
            (false, true) => "A watcher is still running and didn't stop in 20 s.",
        },
        db.display()
    );
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
