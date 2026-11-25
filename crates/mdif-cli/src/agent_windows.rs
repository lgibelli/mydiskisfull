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
    // The `Run` key names it as text: a name that isn't would name another file.
    if exe.to_str().is_none() {
        bail!(
            "{} has a name that can't be written as text: move MyDiskIsFull to a folder with a plain name",
            exe.display()
        );
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

/// What a stop request says. A file of that name that says anything else isn't
/// ours: a watcher neither obeys it nor removes it.
const STOP_REQUEST: &str = "mdif: please stop watching";

fn is_stop_request(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(256).read_to_string(&mut text))
        .is_ok()
        && text == STOP_REQUEST
}

/// Whether a stop has been asked for.
pub fn stop_asked(db: &std::path::Path) -> bool {
    is_stop_request(&stop_path(db))
}

/// A stop asked for before this watcher began was for another: removed, if it
/// is a request of ours. One made since it started (`agent uninstall` sees the
/// lock as soon as it's taken) is for this one, and stays for it to find.
pub fn clear_stale_stop(db: &std::path::Path, started: i64) {
    let path = stop_path(db);
    let made = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    if is_stop_request(&path) && made.is_none_or(|t| t < started) {
        let _ = std::fs::remove_file(path);
    }
}

/// A stop asked for and not yet carried out, called off: `agent stop` or
/// `uninstall` gave up waiting, and the watcher is wanted after all.
fn call_off_stop(db: &std::path::Path) -> bool {
    let asked = stop_asked(db);
    if asked {
        let _ = std::fs::remove_file(stop_path(db));
    }
    asked
}

/// The file that says the watcher was stopped on purpose (`agent stop`): the
/// app, which starts again a watcher that has ended though it's set to start
/// at sign-in, leaves it stopped while it's there. A watcher that starts
/// (at sign-in, or launched) takes it away, and so do `agent start` and
/// `agent install` when one is running already: it's wanted again.
pub fn stopped_path(db: &std::path::Path) -> PathBuf {
    beside(db, ".stopped")
}

/// A watcher is running, or is wanted again: it's no longer stopped on purpose.
pub fn clear_stopped(db: &std::path::Path) {
    let _ = std::fs::remove_file(stopped_path(db));
}

/// What `agent install` gives the watcher it starts, to be told that this very
/// watcher took its first sample (in the environment, so it needs no argument).
const NONCE_VAR: &str = "MDIF_WATCH_NONCE";

fn started_path(db: &std::path::Path) -> PathBuf {
    beside(db, ".started")
}

/// Called by a watcher after its first sample: says so to whoever started it
/// (and gave it a nonce), by writing the nonce where they look.
pub fn acknowledge(db: &std::path::Path) {
    if let Ok(nonce) = std::env::var(NONCE_VAR) {
        let _ = std::fs::write(started_path(db), nonce);
    }
}

/// Starts the watcher `exe` now, as sign-in does, and waits for its first
/// sample.
fn launch(exe: &std::path::Path, interval: u64, db: &std::path::Path) -> Result<()> {
    // Started is when this very watcher has taken its first sample: it says
    // so with the nonce given it. One that gave up before that has said why
    // in its log.
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let _ = std::fs::remove_file(started_path(db));
    clear_stopped(db);
    // It outlives this program: it must hold neither the pipe this program's
    // output goes to (whoever reads it would wait for the watcher to end) nor
    // the folder this was run from (which couldn't be deleted or ejected).
    startup::keep_std_handles_to_self();
    let mut child = Command::new(exe)
        .args(["--interval", &interval.to_string()])
        .current_dir(exe.parent().context("the program has no folder")?)
        .env(NONCE_VAR, &nonce)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let up = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "the watcher stopped at once ({status}); see {}",
                watch_cmd::log_path().display()
            );
        }
        if std::fs::read_to_string(started_path(db)).is_ok_and(|t| t == nonce) {
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
    Ok(())
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
    keep_running(&db);
    if startup::is_claimed(&lock) {
        println!(
            "The watcher is already running; the new settings apply from the next sign-in \
             (`mdif agent uninstall` stops it now)."
        );
    } else {
        launch(&exe, interval, &db)?;
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

/// The watcher is wanted from now on: not stopped on purpose any more, and a
/// stop asked for and not yet carried out (an `agent stop` or `uninstall` that
/// gave up waiting: the watcher would go when it next looks) is called off. One
/// already leaving lets go of its history within a second or two.
fn keep_running(db: &std::path::Path) {
    clear_stopped(db);
    if call_off_stop(db) {
        let lock = lock_path(db);
        let asked = std::time::Instant::now();
        while startup::is_claimed(&lock) && asked.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Asks the running watcher of `db`, if there is one, to finish, and waits
/// until it has let go of its history; whether one was running.
fn stop_running(db: &std::path::Path) -> Result<bool> {
    let lock = lock_path(db);
    if !startup::is_claimed(&lock) {
        return Ok(false);
    }
    std::fs::write(stop_path(db), STOP_REQUEST)
        .with_context(|| format!("asking the watcher to stop ({})", stop_path(db).display()))?;
    let asked = std::time::Instant::now();
    while startup::is_claimed(&lock) && asked.elapsed() < std::time::Duration::from_secs(20) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if startup::is_claimed(&lock) {
        // The request stays: a watcher busy with a long sample will find it.
        bail!(
            "the watcher didn't stop within 20 s (it may be in a long sample); the request \
             stays, and it will stop when it next looks"
        );
    }
    // Gone with it; one left behind would stop the next watcher at once.
    call_off_stop(db);
    Ok(true)
}

/// The program and interval a `Run` command line of [`command_line`]'s
/// starts: `"<exe>" --interval <n>` (the interval 300 if it names none).
fn parse_run_command(command: &str) -> Option<(PathBuf, u64)> {
    let command = command.trim();
    let (exe, rest) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?,
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    if exe.is_empty() {
        return None;
    }
    let mut words = rest.split_whitespace();
    let mut interval = 300;
    while let Some(word) = words.next() {
        if word == "--interval" {
            interval = words.next()?.parse().ok()?;
        }
    }
    Some((PathBuf::from(exe), interval))
}

/// `mdif agent stop`: the watcher stops now and starts again at the next
/// sign-in (or with `agent start`). The installer stops it this way before it
/// replaces `mdif-watch.exe`, which Windows doesn't let a running program's
/// file be.
pub fn stop() -> Result<()> {
    let db = watch_cmd::default_db();
    // Said first, so the app never sees it ended and not stopped on purpose;
    // until the next start it leaves it stopped. (No folder: no watcher ever
    // ran here.)
    if db.parent().is_some_and(std::path::Path::is_dir) {
        std::fs::write(stopped_path(&db), "stopped with `mdif agent stop`")
            .with_context(|| format!("writing {}", stopped_path(&db).display()))?;
    }
    if stop_running(&db)? {
        println!("Watcher stopped. It starts again at sign-in, or now with `mdif agent start`.");
    } else {
        println!("No watcher was running.");
    }
    Ok(())
}

/// `mdif agent start`: the watcher starts now, as sign-in would start it: the
/// program and interval its `Run` value names, and not if it isn't set to
/// start then (Task Manager's switch included).
pub fn start() -> Result<()> {
    let db = watch_cmd::default_db();
    let Some(item) = startup::login_item(NAME).context("reading the Run key")? else {
        bail!(
            "the watcher isn't set to start at sign-in: `mdif agent install` sets it up and starts it"
        );
    };
    if !item.enabled {
        bail!(
            "the watcher is switched off in Task Manager's Startup apps: switch it on there, or \
             run `mdif agent install`"
        );
    }
    // Left stopped on purpose, it would stay down once the one running ends.
    keep_running(&db);
    if startup::is_claimed(&lock_path(&db)) {
        println!("The watcher is already running.");
        return Ok(());
    }
    let Some((exe, interval)) = parse_run_command(&item.command) else {
        bail!(
            "the watcher's Run value isn't one `mdif agent install` wrote ({}): run it again",
            item.command
        );
    };
    if !exe.is_file() {
        bail!(
            "{} is missing: run `mdif agent install` from where mdif is now",
            exe.display()
        );
    }
    launch(&exe, interval, &db)?;
    println!(
        "Watcher started: every {interval} s, history in {}.",
        db.display()
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let db = watch_cmd::default_db();
    let lock = lock_path(&db);
    let had = startup::remove_login_item(NAME).context("removing the Run entry")?;
    let stopped = stop_running(&db)?;
    // Nothing starts it again now: there's nothing to say about why it's stopped.
    clear_stopped(&db);
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

pub fn status(json_out: bool) -> Result<()> {
    if json_out {
        let item = startup::login_item(NAME)?;
        let out = serde_json::json!({
            // "yes", "off" (switched off in Task Manager's Startup apps) or "no".
            "at_sign_in": match &item {
                Some(i) if i.enabled => "yes",
                Some(_) => "off",
                None => "no",
            },
            "command": item.as_ref().map(|i| i.command.clone()),
            "running": startup::is_claimed(&lock_path(&watch_cmd::default_db())),
            // Stopped on purpose (`agent stop`) until it next starts: the app
            // doesn't start it again meanwhile.
            "stopped": stopped_path(&watch_cmd::default_db()).exists(),
            "history": watch_cmd::history_json(&watch_cmd::default_db())?,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
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
    #[test]
    fn a_run_command_names_its_program_and_interval() {
        let exe =
            std::path::Path::new(r"C:\Users\Ann Lee\AppData\Local\MyDiskIsFull\mdif-watch.exe");
        assert_eq!(
            parse_run_command(&command_line(exe, 120)),
            Some((exe.to_path_buf(), 120))
        );
        assert_eq!(
            parse_run_command(r"C:\Tools\mdif-watch.exe"),
            Some((PathBuf::from(r"C:\Tools\mdif-watch.exe"), 300))
        );
        assert_eq!(
            parse_run_command(r"C:\Tools\mdif-watch.exe --interval 60"),
            Some((PathBuf::from(r"C:\Tools\mdif-watch.exe"), 60))
        );
        // Not one of ours: refused rather than guessed.
        assert_eq!(
            parse_run_command(r#""C:\Tools\mdif-watch.exe --interval 60"#),
            None
        );
        assert_eq!(parse_run_command(r#""C:\x.exe" --interval soon"#), None);
        assert_eq!(parse_run_command(r#""" --interval 60"#), None);
        assert_eq!(parse_run_command(""), None);
    }
}
