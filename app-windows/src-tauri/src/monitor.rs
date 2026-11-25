//! The Mac app's `Monitor`, for Windows: every ten minutes (and when the window
//! asks) `mdif status --json`, the tray's tooltip from it, and notifications
//! as on the Mac: each runaway filler once, each alert level once per episode
//! (up a level is said at once, down one only after six hours there, so a
//! forecast hovering at a boundary doesn't repeat itself). The window gets the
//! status as an event, so it never waits for a refresh of its own.
//!
//! It also does what launchd does for the Mac's watcher: one that ended though
//! it's set to start at sign-in is started again ([`Supervisor`]).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::format;

/// How often the status is read when nothing asks sooner.
const EVERY: Duration = Duration::from_secs(600);
/// A lower alert level is taken as the level once it has lasted this long.
const LOWER_FOR: i64 = 6 * 3600;
/// A watcher seen ended is started again when it's still ended this much
/// later: an update stops it a moment before it quits the app.
const GRACE: i64 = 120;
/// And at most this often, so one that can't run isn't started over and over.
const RETRY: i64 = 3600;

/// Whether to start the watcher again, from what `mdif agent status --json`
/// says: when it's set to start at sign-in (and not switched off in Task
/// Manager), isn't running, and wasn't stopped on purpose (`mdif agent stop`,
/// until it next starts), seen so twice [`GRACE`] apart; at most once every
/// [`RETRY`].
#[derive(Debug, Default)]
struct Supervisor {
    /// When it was first seen ended, this time.
    ended_since: Option<i64>,
    /// When it was last started again.
    tried: Option<i64>,
}

impl Supervisor {
    fn start_now(&mut self, agent: &Value, now: i64) -> bool {
        let wanted =
            agent["at_sign_in"] == "yes" && agent["running"] == false && agent["stopped"] != true;
        if !wanted {
            self.ended_since = None;
            return false;
        }
        let since = *self.ended_since.get_or_insert(now);
        if now - since < GRACE || self.tried_lately(now) {
            return false;
        }
        self.tried = Some(now);
        true
    }

    fn tried_lately(&self, now: i64) -> bool {
        self.tried.is_some_and(|t| now - t < RETRY)
    }

    /// How long until the next look: [`GRACE`] while it's been seen ended
    /// and may be started at the next, the usual otherwise.
    fn wait(&self, now: i64) -> Duration {
        if self.ended_since.is_some() && !self.tried_lately(now + GRACE) {
            Duration::from_secs(GRACE as u64)
        } else {
            EVERY
        }
    }
}

/// What the monitor knows: the last status, as `mdif` printed it, and how to
/// wake it.
pub struct State {
    latest: Mutex<Option<String>>,
    wake: Mutex<Sender<()>>,
    /// Off in Settings: nothing is announced, nothing counts as announced.
    pub quiet: Mutex<bool>,
}

/// What was announced, kept between runs (the Mac app keeps it in its
/// defaults).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Announced {
    incidents: BTreeSet<String>,
    level: u8,
    lower_since: Option<i64>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn announced_file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("announced.json"))
}

fn load(app: &AppHandle) -> Announced {
    announced_file(app)
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, a: &Announced) {
    let Some(file) = announced_file(app) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_vec(a) {
        let _ = std::fs::write(file, text);
    }
}

/// From least to most urgent, as `mdif` names them.
fn rank(alert: &str) -> u8 {
    match alert {
        "14_days" => 1,
        "3_days" => 2,
        "1_day" => 3,
        "low" => 4,
        _ => 0,
    }
}

/// One per episode, stable across refreshes: as the Mac app's `Episode.id`.
fn episode_id(e: &Value) -> String {
    format!(
        "{}:{}:{}",
        e["incident"]["kind"].as_str().unwrap_or_default(),
        e["incident"]["subject"].as_str().unwrap_or_default(),
        e["opened"].as_i64().unwrap_or_default()
    )
}

/// "target keeps growing", "Safari keeps rewriting the disk".
fn episode_title(e: &Value) -> String {
    let subject = e["incident"]["subject"].as_str().unwrap_or_default();
    if e["incident"]["kind"] == "churn" {
        let who = e["app"].as_str().unwrap_or(subject);
        return format!("{who} keeps rewriting the disk");
    }
    let name = subject
        .rsplit(['\\', '/'])
        .find(|n| !n.is_empty())
        .unwrap_or(subject);
    format!("{name} keeps growing")
}

/// What the tray says when the pointer rests on it (Windows shows 127
/// characters at most).
pub fn tooltip(s: &Value) -> String {
    let avail = s["avail"].as_i64().unwrap_or_default();
    let mut t = format!("MyDiskIsFull: {} free", format::bytes(avail));
    match s["forecast"]["full_in"]["days"].as_f64() {
        Some(days) if s["alert"] != "none" => {
            t += &format!(", full in about {}", format::days(days));
        }
        _ => {}
    }
    let open = s["incidents"].as_array().map_or(0, Vec::len);
    if open > 0 {
        t += &format!(
            "; {open} folder{} filling it",
            if open == 1 { "" } else { "s" }
        );
    }
    t.chars().take(127).collect()
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// What's new in `s` to say, as the Mac app's `speakUp` works it out: each
/// runaway filler once (if that kind is wanted), and the alert level when it
/// goes up (almost full always; the forecast if wanted). A lower level is
/// taken only once it has lasted `LOWER_FOR`, so a forecast hovering at a
/// boundary doesn't repeat itself. `a` is what was said before, updated.
fn news(s: &Value, a: &mut Announced, now: i64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let wants = |key: &str| s["alerts"][key].as_bool().unwrap_or(true);
    let episodes = s["incidents"].as_array().cloned().unwrap_or_default();
    let open: BTreeSet<String> = episodes.iter().map(episode_id).collect();
    for e in &episodes {
        let wanted = if e["incident"]["kind"] == "growth" {
            wants("notify_growth")
        } else {
            wants("notify_churn")
        };
        if wanted && !a.incidents.contains(&episode_id(e)) {
            let sentence = e["sentence"].as_str().unwrap_or_default();
            let body = match e["fix_line"].as_str() {
                Some(fix) if !fix.is_empty() => format!("{sentence}\n{fix}"),
                _ => sentence.to_string(),
            };
            out.push((episode_title(e), body));
        }
    }
    // Closed ones are forgotten: one that opens again is new.
    a.incidents = open;

    let level = rank(s["alert"].as_str().unwrap_or("none"));
    if level > a.level {
        if level == 4 || wants("notify_forecast") {
            let title = if level == 4 {
                "The disk is almost full".to_string()
            } else if let Some(days) = s["forecast"]["full_in"]["days"].as_f64() {
                format!("The disk is full in about {}", format::days(days))
            } else {
                "The disk is filling up".to_string()
            };
            if let Some(sentence) = s["alert_sentence"].as_str() {
                out.push((title, sentence.to_string()));
            }
        }
        a.level = level;
        a.lower_since = None;
    } else if level < a.level {
        match a.lower_since {
            None => a.lower_since = Some(now),
            Some(since) if now - since > LOWER_FOR => {
                a.level = level;
                a.lower_since = None;
            }
            Some(_) => {}
        }
    } else {
        a.lower_since = None;
    }
    out
}

/// Says what's new in `s`.
fn speak_up(app: &AppHandle, s: &Value, a: &mut Announced) {
    for (title, body) in news(s, a, now()) {
        notify(app, &title, &body);
    }
}

/// One refresh: the status read, the tray and the window told, what's new said.
fn refresh(app: &AppHandle) {
    let state = app.state::<State>();
    match crate::mdif::run(&["status".into(), "--json".into()], Duration::from_secs(60)) {
        Ok(text) => {
            let Ok(s) = serde_json::from_str::<Value>(&text) else {
                return;
            };
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_tooltip(Some(tooltip(&s)));
            }
            if !*state.quiet.lock().unwrap_or_else(|e| e.into_inner()) {
                let mut a = load(app);
                speak_up(app, &s, &mut a);
                save(app, &a);
            }
            *state.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
            let _ = app.emit("status", text);
        }
        Err(why) => {
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_tooltip(Some("MyDiskIsFull: the disk couldn't be read"));
            }
            let _ = app.emit("status-error", why);
        }
    }
}

/// Whether the watcher runs, and if it should and doesn't, `mdif agent start`
/// (which checks again). The window is told, to show it running.
fn supervise(app: &AppHandle, s: &mut Supervisor) {
    if !cfg!(windows) {
        return;
    }
    let args = |a: &[&str]| a.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let Ok(text) = crate::mdif::run(
        &args(&["agent", "status", "--json"]),
        Duration::from_secs(30),
    ) else {
        return;
    };
    let Ok(agent) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if s.start_now(&agent, now())
        && crate::mdif::run(&args(&["agent", "start"]), Duration::from_secs(60)).is_ok()
    {
        s.ended_since = None;
        refresh(app);
        let _ = app.emit("agent", ());
    }
}

fn run(app: AppHandle, wake: Receiver<()>) {
    let mut supervisor = Supervisor::default();
    loop {
        refresh(&app);
        supervise(&app, &mut supervisor);
        match wake.recv_timeout(supervisor.wait(now())) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {
                // Several asks at once are one refresh.
                while wake.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Starts the monitor; its state is the app's from then on.
pub fn start(app: &AppHandle, quiet: bool) {
    let (tx, rx) = channel();
    app.manage(State {
        latest: Mutex::new(None),
        wake: Mutex::new(tx),
        quiet: Mutex::new(quiet),
    });
    let handle = app.clone();
    std::thread::spawn(move || run(handle, rx));
}

/// The last status read, if one was: what the window shows while it waits.
#[tauri::command]
pub fn latest(state: tauri::State<'_, State>) -> Option<String> {
    state
        .latest
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Reads the status again now (after a clean-up, or when the window opens).
#[tauri::command]
pub fn refresh_now(state: tauri::State<'_, State>) {
    let _ = state
        .wake
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_tooltip_says_what_is_free_and_what_fills_it() {
        let s = json!({
            "avail": 42_300_000_000_i64, "alert": "3_days",
            "forecast": {"full_in": {"days": 2.6}},
            "incidents": [{"incident": {"kind": "growth", "subject": "C:\\x"}}],
        });
        assert_eq!(
            tooltip(&s),
            "MyDiskIsFull: 42.3 GB free, full in about 3 days; 1 folder filling it"
        );
        let calm = json!({"avail": 900_000_000_000_i64, "alert": "none", "incidents": []});
        assert_eq!(tooltip(&calm), "MyDiskIsFull: 900.0 GB free");
    }

    #[test]
    fn episodes_are_named_as_on_the_mac() {
        let e = json!({"opened": 5, "incident": {"kind": "growth", "subject": "C:\\Users\\me\\devel\\app\\target"}});
        assert_eq!(episode_title(&e), "target keeps growing");
        assert_eq!(episode_id(&e), "growth:C:\\Users\\me\\devel\\app\\target:5");
        let c = json!({"opened": 5, "app": "Edge", "incident": {"kind": "churn", "subject": "msedge.exe"}});
        assert_eq!(episode_title(&c), "Edge keeps rewriting the disk");
    }

    fn status(alert: &str, incidents: Value) -> Value {
        json!({
            "alert": alert,
            "alert_sentence": "Only 9.8 GB is left.",
            "forecast": {"full_in": {"days": 2.6}},
            "incidents": incidents,
            "alerts": {"notify_forecast": true, "notify_growth": true, "notify_churn": false},
        })
    }

    fn episode(kind: &str, subject: &str, opened: i64) -> Value {
        json!({
            "opened": opened, "sentence": format!("{subject} grew 3.0 GB in 6 hours."),
            "fix_line": "Steps: Lower its cache size.",
            "incident": {"kind": kind, "subject": subject},
        })
    }

    #[test]
    fn each_runaway_is_said_once_and_again_only_when_it_opens_again() {
        let mut a = Announced::default();
        let target = episode("growth", r"C:\Users\me\devel\app\target", 100);
        let said = news(&status("none", json!([target])), &mut a, 0);
        assert_eq!(
            said,
            [(
                "target keeps growing".to_string(),
                "C:\\Users\\me\\devel\\app\\target grew 3.0 GB in 6 hours.\nSteps: Lower its cache size."
                    .to_string()
            )]
        );
        assert!(news(&status("none", json!([target])), &mut a, 600).is_empty());
        // Closed, then open again: a new episode, said again.
        assert!(news(&status("none", json!([])), &mut a, 1200).is_empty());
        let again = episode("growth", r"C:\Users\me\devel\app\target", 2000);
        assert_eq!(news(&status("none", json!([again])), &mut a, 2400).len(), 1);
        // A kind that isn't wanted is never said.
        let churn = episode("churn", "msedge.exe", 3000);
        assert!(news(&status("none", json!([again, churn])), &mut a, 3600).is_empty());
    }

    #[test]
    fn an_alert_is_said_going_up_and_taken_lower_only_after_six_hours() {
        let mut a = Announced::default();
        let said = news(&status("3_days", json!([])), &mut a, 0);
        assert_eq!(said[0].0, "The disk is full in about 3 days");
        assert!(news(&status("3_days", json!([])), &mut a, 600).is_empty());
        // Down a level for a while, and back up within six hours: nothing
        // more to say.
        assert!(news(&status("14_days", json!([])), &mut a, 1200).is_empty());
        assert!(news(&status("14_days", json!([])), &mut a, 1800).is_empty());
        assert_eq!(a.level, 2);
        assert!(news(&status("3_days", json!([])), &mut a, 2000).is_empty());
        // Down for longer: taken, so going up again is said again.
        news(&status("14_days", json!([])), &mut a, 2400);
        news(&status("14_days", json!([])), &mut a, 2400 + LOWER_FOR + 1);
        assert_eq!(a.level, 1);
        assert_eq!(
            news(&status("3_days", json!([])), &mut a, 2400 + LOWER_FOR + 600).len(),
            1
        );
        // Almost full is said even when the forecast isn't wanted.
        let mut quiet = status("low", json!([]));
        quiet["alerts"]["notify_forecast"] = json!(false);
        assert_eq!(news(&quiet, &mut a, 99_999)[0].0, "The disk is almost full");
        let mut b = Announced::default();
        let mut forecast_off = status("1_day", json!([]));
        forecast_off["alerts"]["notify_forecast"] = json!(false);
        assert!(news(&forecast_off, &mut b, 0).is_empty());
    }

    #[test]
    fn a_watcher_that_ended_is_started_again_seen_twice_and_not_often() {
        let ended = json!({"at_sign_in": "yes", "running": false, "stopped": false});
        let mut s = Supervisor::default();
        assert!(!s.start_now(&ended, 1000), "seen once");
        assert_eq!(s.wait(1000), Duration::from_secs(GRACE as u64));
        assert!(!s.start_now(&ended, 1000 + GRACE - 1));
        assert!(s.start_now(&ended, 1000 + GRACE));
        // Ended again at once: not again within the hour, nor looked at sooner.
        let then = 1000 + GRACE;
        assert!(!s.start_now(&ended, then + 600));
        assert_eq!(s.wait(then + 600), EVERY);
        assert_eq!(
            s.wait(then + RETRY - GRACE),
            Duration::from_secs(GRACE as u64)
        );
        assert!(s.start_now(&ended, then + RETRY));
        // Seen running in between: the count starts afresh.
        let mut s = Supervisor::default();
        assert!(!s.start_now(&ended, 0));
        assert!(!s.start_now(&json!({"at_sign_in": "yes", "running": true}), 60));
        assert!(!s.start_now(&ended, GRACE));
        assert!(s.start_now(&ended, 2 * GRACE));
        // Running, stopped on purpose, switched off, not set up, or not said:
        // left as it is.
        for agent in [
            json!({"at_sign_in": "yes", "running": true, "stopped": false}),
            json!({"at_sign_in": "yes", "running": false, "stopped": true}),
            json!({"at_sign_in": "off", "running": false, "stopped": false}),
            json!({"at_sign_in": "no", "running": false, "stopped": false}),
            json!({}),
        ] {
            let mut s = Supervisor::default();
            assert!(!s.start_now(&agent, 0), "{agent}");
            assert!(!s.start_now(&agent, 10 * RETRY), "{agent}");
            assert_eq!(s.wait(10 * RETRY), EVERY, "{agent}");
        }
    }

    #[test]
    fn alert_levels_rank_as_mdif_names_them() {
        let order: Vec<u8> = ["none", "14_days", "3_days", "1_day", "low"]
            .into_iter()
            .map(rank)
            .collect();
        assert_eq!(order, [0, 1, 2, 3, 4]);
    }
}
