//! MyDiskIsFull for Windows: a tray icon (what's free, in its tooltip), the
//! window with the pages, and notifications. Like the Mac app it never
//! measures anything itself: it runs `mdif` (see `mdif.rs`), and the watcher
//! is `mdif-watch.exe`, started at sign-in by `mdif agent install`.
//!
//! Closing the window keeps the app in the tray; Quit (in the tray's menu)
//! ends it. Started again, it brings its window forward instead. Started with
//! `--background` (at sign-in), it opens no window.

// No console window behind the app's own.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod format;
mod login;
mod map;
mod mdif;
mod monitor;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

/// The app's own settings (the history's are `mdif`'s).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Notifications on: a runaway filler, the forecast getting close.
    notifications: bool,
    /// Opening at sign-in was set up once, at the welcome's Get Started;
    /// from then on it's the user's (Settings, Task Manager).
    login_set: bool,
    /// The first window's welcome is done (Get Started).
    welcomed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            notifications: true,
            login_set: false,
            welcomed: false,
        }
    }
}

/// What the window reads of the settings.
#[derive(Serialize)]
struct ForWindow {
    notifications: bool,
    /// False until Get Started: the window shows the welcome first.
    welcomed: bool,
}

fn settings_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_file(app)
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let file = settings_file(app).ok_or("no folder for the app's settings")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, serde_json::to_vec(s).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn settings(app: AppHandle) -> ForWindow {
    let s = load_settings(&app);
    ForWindow {
        notifications: s.notifications,
        welcomed: s.welcomed,
    }
}

/// Get Started, in the welcome: from then on the app opens at sign-in, so the
/// tray says what's free and notifications reach the user, as the Mac app
/// does when its first window is done (the welcome says so; Settings turns it
/// off). Asked once: what the user does with it later stays theirs.
#[tauri::command]
fn finish_welcome(app: AppHandle) -> Result<(), String> {
    let mut s = load_settings(&app);
    s.welcomed = true;
    if !s.login_set && login::set(true) == Ok("yes") {
        s.login_set = true;
    }
    save_settings(&app, &s)
}

#[tauri::command]
fn set_notifications(app: AppHandle, on: bool) -> Result<(), String> {
    let mut s = load_settings(&app);
    s.notifications = on;
    save_settings(&app, &s)?;
    *app.state::<monitor::State>()
        .quiet
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = !on;
    Ok(())
}

/// A copy installed elsewhere takes the sign-in item over (once it's set up).
fn follow_login(app: &AppHandle) {
    if load_settings(app).login_set {
        login::follow();
    }
}

/// Runs Explorer on `target` (a folder, or a Windows place such as
/// `shell:RecycleBinFolder`); nothing else is opened or run.
#[cfg(windows)]
fn explorer(target: &str) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The Recycle Bin, in Explorer: emptying it is done there.
#[tauri::command]
fn open_recycle_bin() -> Result<(), String> {
    #[cfg(windows)]
    return explorer("shell:RecycleBinFolder");
    #[cfg(not(windows))]
    Err("Only on Windows.".into())
}

/// Task Manager's list of what starts at sign-in, as Settings shows it.
#[tauri::command]
fn open_startup_apps() -> Result<(), String> {
    #[cfg(windows)]
    return explorer("ms-settings:startupapps");
    #[cfg(not(windows))]
    Err("Only on Windows.".into())
}

/// Settings → System → Storage, where Windows deletes its temporary files and
/// what programs left behind (Storage Sense): what mdif says to use for those.
#[tauri::command]
fn open_storage_settings() -> Result<(), String> {
    #[cfg(windows)]
    return explorer("ms-settings:storagesense");
    #[cfg(not(windows))]
    Err("Only on Windows.".into())
}

/// ChatGPT's usage settings, in the browser: where what apps use of the plan
/// is seen and limited, as OpenAI asks apps to link it. Only this address.
#[tauri::command]
fn open_chatgpt_usage() -> Result<(), String> {
    #[cfg(windows)]
    return explorer("https://chatgpt.com/settings/usage");
    #[cfg(not(windows))]
    Err("Only on Windows.".into())
}

/// Shows `path` selected in Explorer (Finder, while working on the app on a
/// Mac). Only what is there: nothing is opened or run.
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.is_absolute() || std::fs::symlink_metadata(p).is_err() {
        return Err(format!("{path} isn't there now."));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer reads its own command line: the path in quotes after the
        // comma (a Windows path can't hold a quote).
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{path}\""))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Whether the window is on screen (shown, not minimised): a hidden one
/// doesn't have the disk map measured again, as each time is the whole disk.
#[tauri::command]
fn window_shown(app: AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(true))
}

/// The window, brought forward (made first if it was never shown).
fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    if let Some(state) = app.try_state::<monitor::State>() {
        monitor::refresh_now(state);
    }
    // What's old in the window is read again.
    let _ = app.emit("shown", ());
}

fn main() {
    tauri::Builder::default()
        // Started again (from the Start menu, at sign-in): the one running
        // comes forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show(app)
        }))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            finish_welcome,
            login::login_item,
            login::set_login_item,
            map::kept_map,
            map::measure_map,
            mdif::mdif,
            monitor::latest,
            monitor::refresh_now,
            open_chatgpt_usage,
            open_recycle_bin,
            open_startup_apps,
            open_storage_settings,
            reveal,
            settings,
            set_notifications,
            window_shown,
        ])
        .setup(|app| {
            follow_login(app.handle());
            let open = MenuItem::with_id(app, "open", "Open MyDiskIsFull", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit MyDiskIsFull", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("MyDiskIsFull")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = e
                    {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            let quiet = !load_settings(app.handle()).notifications;
            monitor::start(app.handle(), quiet);
            if !std::env::args().any(|a| a == "--background") {
                show(app.handle());
            }
            Ok(())
        })
        // Closing the window keeps the app in the tray.
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("MyDiskIsFull couldn't start");
}

#[cfg(test)]
mod tests {
    /// The app is the command line's release: one version for both (the
    /// installer and the window take it from this crate's Cargo.toml, as
    /// `tauri.conf.json` names none).
    #[test]
    fn the_app_has_the_command_lines_version() {
        let root = include_str!("../../../Cargo.toml");
        let version = root
            .lines()
            .skip_while(|l| l.trim() != "[workspace.package]")
            .find_map(|l| l.trim().strip_prefix("version = "))
            .map(|v| v.trim_matches('"'))
            .expect("the workspace names its version");
        assert_eq!(env!("CARGO_PKG_VERSION"), version);
        let config = include_str!("../tauri.conf.json");
        let config: serde_json::Value = serde_json::from_str(config).unwrap();
        assert!(
            config.get("version").is_none(),
            "tauri.conf.json would name another"
        );
    }
}
