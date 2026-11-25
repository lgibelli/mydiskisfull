//! Updates, as Sparkle gives the Mac app: once a day, unless Settings says not
//! to, the latest release's `latest.json` is read from GitHub (a plain GET:
//! nothing about the PC goes with it), and when it names a newer version the
//! user is told once (a notification) and Settings offers Install and
//! Restart. The installer is signed with the updater's key
//! (docs/RELEASING.md); the plugin checks that signature against the public
//! key in tauri.conf.json before it runs anything, and refuses what fails.
//!
//! Installing ends the app (the plugin's `exit(0)` once the installer is
//! started), so `mdif` runs are stopped first ([`crate::mdif::stop_all`]):
//! a running `mdif.exe` couldn't be replaced, and if one won't stop nothing
//! is installed. The installer's hooks stop and start the watcher. The app
//! comes back with the arguments it had, and with its window, even when it
//! was started at sign-in ([`crate::Settings`]'s `show_after_update`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Error, Update, UpdaterExt};

use crate::{change_settings, load_settings};

/// Where the feed is: tauri.conf.json's `plugins > updater > endpoints`
/// (a test checks they agree), asked again when the plugin can't tell why
/// it got none.
const FEED: &str = "https://github.com/lgibelli/mydiskisfull/releases/latest/download/latest.json";
/// Checked at most this often by itself.
const DAY: i64 = 86_400;
/// How often it looks whether a check is due (a PC that slept, or was off
/// the network, checks within the hour once it's back).
const LOOK: Duration = Duration::from_secs(3600);
/// Not in the way of sign-in: the first look is this long after the start.
const FIRST: Duration = Duration::from_secs(60);
/// A check that hasn't been answered by then has failed (a stalled
/// connection would otherwise hold the daily checks, and Check Now, for good).
const CHECK_WITHIN: Duration = Duration::from_secs(30);
/// The same for the installer's download (a few MB).
#[cfg(windows)]
const DOWNLOAD_WITHIN: Duration = Duration::from_secs(600);
/// What `mdif` is running when an install begins has this long to end by
/// itself before it's stopped.
#[cfg(windows)]
const GRACE: Duration = Duration::from_secs(15);

/// The update found, kept for Install and Restart.
#[derive(Default)]
pub struct State {
    found: Mutex<Option<Update>>,
}

/// One check at a time (Check Now and the daily one): an answer that came
/// late never replaces a newer one.
static CHECKING: tauri::async_runtime::Mutex<()> = tauri::async_runtime::Mutex::const_new(());

/// One install at a time, and never one while what the last one left is
/// being removed ([`after_update`]): the same version's installer, staged
/// again, would be taken for it.
static INSTALL: tauri::async_runtime::Mutex<()> = tauri::async_runtime::Mutex::const_new(());

/// What the window shows of an update: its version (the notes, the
/// CHANGELOG's, are the release page's).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Found {
    version: String,
}

impl From<&Update> for Found {
    fn from(u: &Update) -> Self {
        Found {
            version: u.version.clone(),
        }
    }
}

/// What Settings reads.
#[derive(Serialize)]
pub struct ForWindow {
    /// Checks once a day by itself.
    on: bool,
    /// When it last heard from GitHub, in seconds since 1970.
    checked: Option<i64>,
    found: Option<Found>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Whether a check by itself is due: on, and not in the last day (a clock
/// set back counts as due, or it would wait out the difference).
fn due(on: bool, checked: Option<i64>, now: i64) -> bool {
    on && checked.is_none_or(|c| now - c >= DAY || c > now)
}

/// Whether a version found is news: newer than any told of yet (one found
/// again after a newer was withdrawn isn't; that newer, published again,
/// isn't either). A version that isn't one (not semver) is news unless it's
/// the same.
fn news(found: &str, told: Option<&str>) -> bool {
    let Some(told) = told else {
        return true;
    };
    match (semver::Version::parse(found), semver::Version::parse(told)) {
        (Ok(f), Ok(t)) => f > t,
        _ => found != told,
    }
}

/// Whether the last check offered a version this one isn't: the offer, lost
/// when the app ended, is looked for again at the start, not a day later.
/// (After an update the version offered is this one; a check that finds
/// nothing clears it.)
fn offered(on: bool, offer: Option<&str>, running: &str) -> bool {
    on && offer.is_some_and(|t| t != running)
}

/// What an answer that isn't a feed means: a release without the Windows
/// edition (no latest.json: 404) is nothing new; anything else is a failure
/// to say, not "up to date".
fn missing_feed(status: u16) -> Result<(), String> {
    match status {
        404 => Ok(()),
        200..=299 => Err("The update feed changed while it was read: check again.".into()),
        s => Err(format!(
            "GitHub answered {s} for the update feed: check again later."
        )),
    }
}

/// Reads latest.json. A release without a Windows edition (no latest.json,
/// or none for this kind of PC) is nothing new, not a failure.
async fn look(app: &AppHandle) -> Result<Option<Update>, String> {
    let handle = app.clone();
    let updater = app
        .updater_builder()
        .timeout(CHECK_WITHIN)
        // In place of the plugin's cleanup_before_exit, which also lets go of
        // the single-instance lock: an installer Windows then refuses to start
        // would leave this app running without it, and a second one could
        // open. The tray icon is hidden (Windows would leave it until
        // hovered; hidden on the main thread, which owns its window, where
        // dropping it here wouldn't destroy it) and the window hides; the
        // lock goes with the process, once it has ended.
        .on_before_exit(move || {
            if let Some(tray) = handle.tray_by_id("main") {
                let _ = tray.set_visible(false);
            }
            for w in handle.webview_windows().values() {
                let _ = w.hide();
            }
        })
        .build()
        .map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(u) => Ok(u),
        Err(Error::TargetNotFound(_) | Error::TargetsNotFound(_)) => Ok(None),
        // Said for any answer that isn't a success: asked again for its status.
        Err(Error::ReleaseNotFound) => {
            let status = reqwest::Client::builder()
                .timeout(CHECK_WITHIN)
                .build()
                .map_err(|e| e.to_string())?
                .get(FEED)
                .send()
                .await
                .map_err(|e| format!("The update feed couldn't be read: {e}"))?
                .status();
            missing_feed(status.as_u16()).map(|()| None)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Who asked for a check.
#[derive(Clone, Copy)]
enum By {
    /// Check Now.
    Hand,
    /// The daily check, or the start's look for the offer kept from before
    /// (`restoring`).
    Itself { restoring: bool },
}

/// Whether a check by itself is wanted now: due, or the offer kept from
/// before the app ended is looked for.
fn wanted(s: &crate::Settings, restoring: bool, running: &str, now: i64) -> bool {
    (restoring && offered(s.updates, s.update_offered.as_deref(), running))
        || due(s.updates, s.update_checked, now)
}

/// A check: what's found is kept and given to the window (an event, so an
/// open Settings shows it), and, when it's by itself and new, said once.
/// One by itself that waited for another is asked again whether it's still
/// wanted (switched off meanwhile, or Check Now just checked): if not, it
/// doesn't ask GitHub, and changes nothing.
async fn check(app: &AppHandle, by: By) -> Result<Option<Found>, String> {
    let _one = CHECKING.lock().await;
    let by_itself = matches!(by, By::Itself { .. });
    if let By::Itself { restoring } = by {
        let running = app.package_info().version.to_string();
        if !wanted(&load_settings(app), restoring, &running, now()) {
            return Ok(None);
        }
    }
    let found = look(app).await?;
    let shown = found.as_ref().map(Found::from);
    *app.state::<State>()
        .found
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = found;
    let mut tell = false;
    change_settings(app, |s| {
        s.update_checked = Some(now());
        // Said as a notification only when Notify me is on (as the Mac's
        // reminders), and automatic checks still are (switched off while
        // this one was asking); Settings shows it either way.
        tell = shown.as_ref().is_some_and(|f| {
            by_itself && s.notifications && s.updates && news(&f.version, s.update_told.as_deref())
        });
        s.update_offered = shown.as_ref().map(|f| f.version.clone());
        // Told: by the notification, or in Settings (Check Now). One found
        // while Notify me was off isn't, so it's said once that's on again.
        // The newest told of is kept (a withdrawn one's older found again
        // doesn't lower it).
        if let (true, Some(f)) = (tell || !by_itself, &shown)
            && news(&f.version, s.update_told.as_deref())
        {
            s.update_told = Some(f.version.clone());
        }
    })?;
    let _ = app.emit("update", &shown);
    if let (true, Some(f)) = (tell, &shown) {
        let _ = app
            .notification()
            .builder()
            .title(format!("MyDiskIsFull {} is ready", f.version))
            .body("Install it in Settings, under Updates.")
            .show();
    }
    Ok(shown)
}

/// The app's own folder for what an update stages: the plugin writes the
/// installer through `tempfile`, whose folder is set here just before (in
/// [`install_update`]), so everything in it is the updates' and nothing
/// else's.
fn staging(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|d| d.join("updates"))
}

/// Empties `dir` (the updates' own folder: [`staging`]) of what the updater
/// staged there (`<name>-<version>-updater-*`), nothing followed through a
/// link: not the folder itself if it is one (or a junction), nor anything
/// in it. Whether something couldn't go (an installer still running), or
/// couldn't be looked at: the next start tries again. No folder: nothing.
fn tidy_staged(dir: &Path, name: &str) -> bool {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
        // A link's own metadata: a link or junction here isn't the app's
        // folder, and nothing in it is the update's.
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return false,
        Ok(_) => {}
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    let mut left = false;
    for entry in entries {
        let Ok(entry) = entry else {
            left = true;
            continue;
        };
        if !is_staged(&entry.file_name(), name) {
            continue;
        }
        let path = entry.path();
        // The folder the plugin made (a link, or a file, isn't).
        let gone = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => continue,
            Err(e) => Err(e),
        };
        left |= gone.is_err();
    }
    left
}

/// Whether a name is one the updater stages under: `<name>-<version>-updater-*`.
fn is_staged(file_name: &std::ffi::OsStr, name: &str) -> bool {
    file_name
        .to_str()
        .and_then(|n| n.strip_prefix(&format!("{name}-")))
        .and_then(|rest| rest.split_once("-updater-"))
        .is_some_and(|(version, _)| semver::Version::parse(version).is_ok())
}

/// Whether an installer the updater staged in `dir` runs: a process of this
/// user's named `<name>-<version>-installer` whose program is in `dir`.
fn installer_running(dir: &Path, name: &str) -> bool {
    #[cfg(windows)]
    {
        let dir = dir.to_string_lossy().to_lowercase();
        mdif_windows::proc::own_processes().into_iter().any(|p| {
            p.name.starts_with(&format!("{name}-"))
                && p.name.ends_with("-installer")
                && mdif_windows::proc::path(p.pid)
                    .is_some_and(|exe| exe.to_string_lossy().to_lowercase().starts_with(&dir))
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, name);
        false
    }
}

/// After an update this app started (`version`; kept in the settings until
/// this has run). While its installer runs, no `mdif` runs (`held`: the gate
/// was closed at the start, as the app was opened again meanwhile), so none
/// holds `mdif.exe` against it, and no other install starts: as long as it
/// runs (it may wait for an answer). Once it has ended, what it staged is
/// removed and the watcher it stopped (hooks.nsh) is started as sign-in
/// would start it (nothing if it isn't set up, or is switched off): an
/// installer that didn't complete, even after it had copied the new app,
/// doesn't start it again.
fn after_update(app: &AppHandle, version: Option<String>, dir: Option<PathBuf>, held: bool) {
    let name = app.package_info().name.clone();
    let mut tidied = dir.is_none();
    // All of it, the watcher's step too: an install started meanwhile would
    // have its own flags cleared by this one's.
    let _one = INSTALL.blocking_lock();
    if let Some(dir) = &dir {
        // Often: the installer starts this app a moment before it ends.
        while installer_running(dir, &name) {
            std::thread::sleep(Duration::from_secs(2));
        }
        // It ended without replacing this app (cancelled): the window's
        // flag was kept for the app it would have started (start).
        if held {
            let _ = change_settings(app, |s| s.show_after_update = false);
        }
        // A moment for Windows to let go of the file once it has ended.
        for _ in 0..3 {
            tidied = !tidy_staged(dir, &name);
            if tidied {
                break;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    }
    if held {
        crate::mdif::resume();
        // What was refused meanwhile is read again: the status, and what
        // the window shows.
        if let Some(state) = app.try_state::<crate::monitor::State>() {
            crate::monitor::refresh_now(state);
        }
        let _ = app.emit("shown", ());
    }
    // Once per update (not again while only the clean-up is retried):
    // started as sign-in would, as an installer that stopped it and then
    // didn't complete doesn't. --after-install leaves one the user stopped
    // (before the update, or since) stopped, and asks again at each try.
    // Tried a few times in this run, not at a later start.
    let settings = load_settings(app);
    if settings.update_watcher_due && !settings.show_after_update {
        let start = ["agent".into(), "start".into(), "--after-install".into()];
        for _ in 0..3 {
            if crate::mdif::run(&start, Duration::from_secs(30)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_secs(10));
        }
        let _ = change_settings(app, |s| s.update_watcher_due = false);
        // The watcher's switch, read before this, shows it as it is now.
        let _ = app.emit("agent", ());
    }
    // Kept while what it staged couldn't be removed: the next start tries again.
    let _ = change_settings(app, |s| {
        if tidied && version.is_some() && s.update_staged == version && !s.show_after_update {
            s.update_staged = None;
        }
    });
}

/// Starts the daily checks; the state is the app's from then on. Whether
/// this app was started by an update it began (the window is shown). Before the
/// monitor (which runs `mdif` at once): an installer still running from an
/// update this app started keeps `mdif` from running until it has ended.
pub fn start(app: &AppHandle) -> bool {
    app.manage(State::default());
    // The last run's, read before the window can start an install of its own.
    let settings = load_settings(app);
    let updated = settings.show_after_update;
    let dir = staging(app);
    let name = app.package_info().name.clone();
    // Its installer still runs: nothing runs mdif until it has ended.
    let held = settings.update_staged.is_some()
        && dir.as_deref().is_some_and(|d| installer_running(d, &name));
    if held {
        crate::mdif::hold();
    }
    // Kept while that installer runs: it ends this app to replace it, and the
    // one it starts (with this one's arguments: --background at sign-in)
    // still shows the window the user asked for with Install and Restart.
    // Cleared before after_update reads it.
    if updated && !held {
        let _ = change_settings(app, |s| s.show_after_update = false);
    }
    if settings.update_staged.is_some() {
        let app = app.clone();
        let version = settings.update_staged;
        std::thread::spawn(move || after_update(&app, version, dir, held));
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST);
        let running = app.package_info().version.to_string();
        // The offer kept from before the app ended, until a check gets an
        // answer (started offline, it's looked for again each hour).
        let mut restoring = true;
        loop {
            if wanted(&load_settings(&app), restoring, &running, now()) {
                // Offline: nothing is recorded, so the next look tries again.
                let by = By::Itself { restoring };
                if tauri::async_runtime::block_on(check(&app, by)).is_ok() {
                    restoring = false;
                }
            } else {
                restoring = false;
            }
            std::thread::sleep(LOOK);
        }
    });
    updated
}

#[tauri::command]
pub fn updates(app: AppHandle) -> ForWindow {
    let s = load_settings(&app);
    let found = app.try_state::<State>().and_then(|state| {
        state
            .found
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(Found::from)
    });
    ForWindow {
        on: s.updates,
        checked: s.update_checked,
        found,
    }
}

#[tauri::command]
pub fn set_updates(app: AppHandle, on: bool) -> Result<(), String> {
    change_settings(&app, |s| s.updates = on)
}

/// Check Now, in Settings.
#[tauri::command]
pub async fn check_updates(app: AppHandle) -> Result<Option<Found>, String> {
    check(&app, By::Hand).await
}

/// Install and Restart: the installer is downloaded and its signature
/// checked, `mdif` runs are stopped, and the installer is started, which
/// ends the app; it starts it again when it's done. Only the version found
/// and shown (`version`, which a check meanwhile may have replaced) is
/// installed, once.
#[tauri::command]
pub async fn install_update(app: AppHandle, version: String) -> Result<(), String> {
    let update = app
        .state::<State>()
        .found
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or("There's no update to install now: check again.")?;
    if update.version != version {
        let _ = app.emit("update", Some(Found::from(&update)));
        return Err(format!(
            "MyDiskIsFull {} is the one to install now, not {version}: look again.",
            update.version
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = update;
        Err("Only on Windows.".into())
    }
    #[cfg(windows)]
    {
        let Ok(_one) = INSTALL.try_lock() else {
            return Err("An update is being installed or tidied up: try again in a minute.".into());
        };
        // Asked again: one withdrawn since it was offered (no longer in the
        // feed) isn't installed.
        let mut update = match look(&app).await? {
            Some(fresh) if fresh.version == update.version => fresh,
            other => {
                let shown = other.as_ref().map(Found::from);
                *app.state::<State>()
                    .found
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = other;
                let _ = app.emit("update", &shown);
                return Err(format!(
                    "MyDiskIsFull {} is no longer offered: nothing was installed.",
                    update.version
                ));
            }
        };
        // The check's deadline isn't the download's.
        update.timeout = Some(DOWNLOAD_WITHIN);
        let bytes = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| format!("The update couldn't be downloaded: {e}. Check again."))?;
        // What an earlier update left to remove (it couldn't be): kept if
        // this one stops before it stages anything.
        let mut before = None;
        change_settings(&app, |s| {
            before = s.update_staged.replace(update.version.clone());
            s.show_after_update = true;
            s.update_watcher_due = true;
        })?;
        // The installer it wrote is the plugin's to remove, as it returns.
        let undo = |app: &AppHandle| {
            crate::mdif::resume();
            let _ = change_settings(app, |s| {
                s.show_after_update = false;
                s.update_staged = before.clone();
                s.update_watcher_due = false;
            });
        };
        if let Err(busy) = crate::mdif::stop_all(GRACE) {
            undo(&app);
            return Err(match busy {
                crate::mdif::Busy::Changing => {
                    "A clean-up, a change or a measurement is still under way: nothing was \
                     installed. Install once it's done."
                }
                crate::mdif::Busy::Stuck => {
                    "mdif didn't stop, so it couldn't be replaced: nothing was installed. \
                     Try again in a moment."
                }
            }
            .into());
        }
        // The installer is written to the app's own folder (so what's left
        // there is the update's alone: after_update), not the user's TEMP;
        // without it, nothing is installed (the next start couldn't tell
        // whether it still runs).
        // Not a link or junction: the clean-up after wouldn't follow it.
        let staged_in = staging(&app).filter(|d| {
            std::fs::create_dir_all(d).is_ok()
                && std::fs::symlink_metadata(d)
                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        });
        let staged_ok = staged_in.as_deref().is_some_and(|d| {
            match tempfile::env::override_temp_dir(d) {
                Ok(()) => true,
                // Set already, by an earlier try in this run.
                Err(set) => set == d,
            }
        });
        if !staged_ok {
            undo(&app);
            return Err("There's no folder to put the update in, so nothing was installed.".into());
        }
        // Once the installer is started it doesn't return: the app ends here.
        let result = update.install(bytes);
        undo(&app);
        // What it staged for the installer that didn't start (the folder is
        // left by the plugin): kept for the next start if it can't go.
        let left = staged_in
            .as_deref()
            .is_some_and(|d| tidy_staged(d, &app.package_info().name));
        // The whole folder was tidied: an earlier update's leftovers too.
        let _ = change_settings(&app, |s| {
            s.update_staged = left.then(|| update.version.clone());
        });
        // The tray and the window were hidden before Windows was asked to
        // start the installer: both come back to say so.
        match app.tray_by_id("main") {
            Some(tray) => {
                let _ = tray.set_visible(true);
            }
            None => {
                let _ = crate::tray(&app);
            }
        }
        crate::show(&app);
        Err(match result {
            Ok(()) => "The installer didn't start.".into(),
            Err(e) => format!("The update couldn't be installed: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_by_itself_is_due_once_a_day_and_only_when_on() {
        let now = 1_800_000_000;
        assert!(due(true, None, now), "never checked");
        assert!(!due(false, None, now), "switched off");
        assert!(!due(true, Some(now - DAY + 1), now));
        assert!(due(true, Some(now - DAY), now));
        assert!(due(true, Some(now + 3600), now), "the clock was set back");
        assert!(!due(false, Some(now - 2 * DAY), now));
    }

    #[test]
    fn a_version_is_told_of_once_and_looked_for_again_after_a_restart() {
        assert!(news("0.1.7", None));
        assert!(!news("0.1.7", Some("0.1.7")));
        assert!(news("0.1.8", Some("0.1.7")));
        // Withdrawn: the older found again isn't news, nor the newer again.
        assert!(!news("0.1.7", Some("0.1.8")));
        assert!(
            news("0.1.10", Some("0.1.9")),
            "compared as versions, not text"
        );
        // Offered 0.1.7, still 0.1.6: the offer is looked for at the start.
        assert!(offered(true, Some("0.1.7"), "0.1.6"));
        assert!(!offered(true, Some("0.1.7"), "0.1.7"), "installed");
        assert!(!offered(true, None, "0.1.6"));
        assert!(!offered(false, Some("0.1.7"), "0.1.6"), "switched off");
    }

    #[test]
    fn what_an_update_staged_is_emptied_and_links_are_not_followed() {
        let temp = std::env::temp_dir().join(format!("mdif-staged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let staging = temp.join("updates");
        let staged = staging.join("MyDiskIsFull-0.1.7-updater-AbC123");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("MyDiskIsFull-0.1.7-installer.exe"), b"MZ").unwrap();
        // Not the updater's: left.
        std::fs::write(staging.join("notes.txt"), b"x").unwrap();
        // Something of the user's, outside it, reached through links.
        let theirs = temp.join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("MyDiskIsFull-0.1.6-updater-x"), b"mine").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&theirs, staging.join("MyDiskIsFull-0.1.6-updater-link"))
            .unwrap();
        // Not the updater's either: no version in the name, or a file.
        let backup = staging.join("MyDiskIsFull-backup-updater-notes");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(staging.join("MyDiskIsFull-0.1.7-updater-file"), b"x").unwrap();
        assert!(!tidy_staged(&staging, "MyDiskIsFull"), "all of it went");
        assert!(backup.exists() && staging.join("MyDiskIsFull-0.1.7-updater-file").exists());
        assert!(!staged.exists());
        assert!(staging.join("notes.txt").exists());
        assert!(
            theirs.join("MyDiskIsFull-0.1.6-updater-x").exists(),
            "not followed"
        );
        // The folder itself a link: nothing in what it points to goes.
        #[cfg(unix)]
        {
            let linked = temp.join("linked");
            std::os::unix::fs::symlink(&theirs, &linked).unwrap();
            assert!(!tidy_staged(&linked, "MyDiskIsFull"));
            assert!(theirs.join("MyDiskIsFull-0.1.6-updater-x").exists());
        }
        assert!(
            !tidy_staged(&temp.join("none"), "MyDiskIsFull"),
            "no folder: nothing left"
        );
        std::fs::remove_dir_all(&temp).unwrap();
    }

    /// A check by itself that waited behind Check Now, or for the switch:
    /// asked again, it isn't wanted once one was just made or it's off.
    #[test]
    fn a_waiting_check_by_itself_is_asked_again() {
        let now = 1_800_000_000;
        let mut s = crate::Settings::default();
        assert!(wanted(&s, false, "0.1.6", now), "never checked");
        s.update_checked = Some(now - 5);
        assert!(!wanted(&s, false, "0.1.6", now), "Check Now just checked");
        s.update_offered = Some("0.1.7".into());
        assert!(
            wanted(&s, true, "0.1.6", now),
            "the kept offer, at the start"
        );
        assert!(!wanted(&s, false, "0.1.6", now), "once it was looked for");
        s.updates = false;
        assert!(!wanted(&s, true, "0.1.6", now), "switched off meanwhile");
    }

    #[test]
    fn only_a_missing_feed_is_nothing_new() {
        assert_eq!(missing_feed(404), Ok(()));
        for failed in [403, 429, 500, 502, 503] {
            let why = missing_feed(failed).unwrap_err();
            assert!(why.contains(&failed.to_string()), "{why}");
        }
        assert!(missing_feed(200).is_err(), "a feed now: not \"up to date\"");
    }

    /// The feed is the latest published release's latest.json, checked with
    /// the key in the config, and the installer's own window shows its
    /// progress (passive): what release.yml uploads and docs/RELEASING.md
    /// says.
    #[test]
    fn the_app_asks_github_and_checks_the_key() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let u = &config["plugins"]["updater"];
        assert_eq!(u["endpoints"], serde_json::json!([FEED]));
        assert_eq!(u["windows"]["installMode"], "passive");
        // The version is signed with the installer (the bundler writes it in
        // the signature): a feed can't pair a new number with an older build.
        assert_eq!(u["requireSignedVersion"], true);
        // A minisign public key, in base64: "untrusted comment: …".
        let key = u["pubkey"].as_str().expect("a public key");
        assert!(key.starts_with("dW50cnVzdGVkIGNvbW1lbnQ6"), "{key}");
        // Signed installers are made only where the key is (release.yml):
        // a build without it still builds.
        assert!(config["bundle"].get("createUpdaterArtifacts").is_none());
        let signed: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.updater.json")).unwrap();
        assert_eq!(signed["bundle"]["createUpdaterArtifacts"], true);
    }

    /// release.yml, before it uploads: every installer latest.json names is
    /// beside it (`MDIF_FEED`'s folder) and its signature is the app's key's,
    /// as the installed app will check it. A key that isn't the one in
    /// tauri.conf.json still makes signatures (Tauri only warns): installed
    /// copies would then be offered an update they refuse.
    #[test]
    #[ignore = "reads the release's feed (MDIF_FEED): release.yml runs it"]
    fn the_release_feed_is_signed_with_the_apps_key() {
        use base64::Engine;
        let b64 = |s: &str| {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .expect("base64");
            String::from_utf8(raw).expect("text")
        };
        let feed = std::path::PathBuf::from(std::env::var_os("MDIF_FEED").expect("MDIF_FEED"));
        let dir = feed.parent().unwrap();
        let feed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&feed).unwrap()).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let key = minisign_verify::PublicKey::decode(&b64(config["plugins"]["updater"]["pubkey"]
            .as_str()
            .unwrap()))
        .unwrap();
        let platforms = feed["platforms"].as_object().expect("platforms");
        assert_eq!(platforms.len(), 2, "x64 and ARM64: {platforms:?}");
        for (platform, p) in platforms {
            let url = p["url"].as_str().unwrap();
            assert!(
                url.starts_with("https://github.com/lgibelli/mydiskisfull/releases/download/v"),
                "{url}"
            );
            let name = url.rsplit('/').next().unwrap();
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let sig =
                minisign_verify::Signature::decode(&b64(p["signature"].as_str().unwrap())).unwrap();
            key.verify(&bytes, &sig, true).unwrap_or_else(|e| {
                panic!("{platform}: {name} isn't signed with the app's key: {e}")
            });
            // requireSignedVersion: the version signed is the feed's.
            let signed = sig
                .trusted_comment()
                .split('\t')
                .find_map(|f| f.strip_prefix("version:"));
            assert_eq!(
                signed,
                feed["version"].as_str(),
                "{platform}: {name}'s signature names another version"
            );
            println!("{platform}: {name} signed with the app's key");
        }
    }
}
