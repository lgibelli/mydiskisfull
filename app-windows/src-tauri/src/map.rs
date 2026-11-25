//! The disk map (Storage → Folders), kept as the Mac app's `MapModel` keeps
//! it: `mdif map --json`, measured while someone is looking at it, and kept
//! in the app's cache folder so the last one shows at once next time. The
//! file's modification time is when it was measured. Regenerable, and never
//! leaves the computer.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// What the map measures, as on the Mac: six folders deep, folders of
/// 200 MB and more on their own.
const ARGS: &[&str] = &["map", "--depth", "6", "--min-mb", "200", "--json"];

/// A map as `mdif map --json` printed it, and when (seconds since 1970).
#[derive(Debug, Serialize)]
pub struct Kept {
    json: String,
    measured: i64,
}

fn file(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_cache_dir()
        .ok()
        .map(|d| d.join("disk-map.json"))
}

/// Whether `text` is a map this version reads: a root folder with its path
/// and private size.
fn readable(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| {
        let root = &v["scan"]["root"];
        root["path"].is_string() && root["totals"]["private"].is_u64()
    })
}

fn seconds(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The kept map at `path`, if there's one this version reads; one written by
/// another version is deleted, to be measured afresh.
fn read_kept(path: &Path) -> Option<Kept> {
    let json = std::fs::read_to_string(path).ok()?;
    if !readable(&json) {
        let _ = std::fs::remove_file(path);
        return None;
    }
    let measured = seconds(std::fs::metadata(path).ok()?.modified().ok()?);
    Some(Kept { json, measured })
}

/// `json` kept at `path`, whole or not at all (written beside it, then
/// moved over it).
fn keep(path: &Path, json: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("json.part");
    std::fs::write(&part, json)?;
    std::fs::rename(&part, path)
}

/// The map kept from last time, if any.
#[tauri::command]
pub fn kept_map(app: AppHandle) -> Option<Kept> {
    read_kept(&file(&app)?)
}

/// One measurement at a time: a window reloaded while one runs asks again,
/// and two would each take twice as long and write the same file.
static MEASURING: Mutex<()> = Mutex::new(());

/// The map `run` measures, kept at `path`; or, when one was kept there since
/// `asked` (by the measurement this one waited for), that one.
fn measure_once(
    path: Option<&Path>,
    asked: SystemTime,
    run: impl FnOnce() -> Result<String, String>,
) -> Result<Kept, String> {
    let _one = MEASURING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(kept) = path
        .and_then(read_kept)
        .filter(|k| k.measured >= seconds(asked))
    {
        return Ok(kept);
    }
    let json = run()?;
    if !readable(&json) {
        return Err(
            "The bundled mdif printed something this version of the app can't read.".into(),
        );
    }
    if let Some(path) = path {
        // Not kept, it's still shown: only the next start measures again.
        let _ = keep(path, &json);
    }
    Ok(Kept {
        json,
        measured: seconds(SystemTime::now()),
    })
}

/// Measures the disk folder by folder and keeps the map. `automatic`: nobody
/// pressed a button, so it runs at idle priority and may take longer.
#[tauri::command]
pub async fn measure_map(app: AppHandle, automatic: bool) -> Result<Kept, String> {
    let limit = Duration::from_secs(if automatic { 3600 } else { 900 });
    let args: Vec<String> = ARGS.iter().map(|a| a.to_string()).collect();
    let path = file(&app);
    let asked = SystemTime::now();
    tauri::async_runtime::spawn_blocking(move || {
        measure_once(path.as_deref(), asked, || {
            crate::mdif::run_at(&args, limit, automatic)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str =
        r#"{"root":"C:\\","scan":{"root":{"path":"C:\\","totals":{"private":5},"children":[]}}}"#;

    #[test]
    fn a_kept_map_is_read_back_and_one_it_cant_read_goes() {
        let dir = std::env::temp_dir().join(format!("mdif-map-{}", std::process::id()));
        let path = dir.join("cache").join("disk-map.json");
        assert!(read_kept(&path).is_none());
        keep(&path, MAP).unwrap();
        let kept = read_kept(&path).unwrap();
        assert_eq!(kept.json, MAP);
        assert!((kept.measured - seconds(SystemTime::now())).abs() < 60);
        assert!(!path.with_extension("json.part").exists());
        // Another version's: deleted, to be measured afresh.
        for other in [r#"{"scan":{"root":{"path":"C:\\"}}}"#, "not json", ""] {
            std::fs::write(&path, other).unwrap();
            assert!(read_kept(&path).is_none(), "{other}");
            assert!(!path.exists(), "{other}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_measurement_at_a_time_and_one_that_waited_gets_its_map() {
        let dir = std::env::temp_dir().join(format!("mdif-map-once-{}", std::process::id()));
        let path = dir.join("disk-map.json");
        let (running, ran) = std::sync::mpsc::channel();
        let first = {
            let path = path.clone();
            std::thread::spawn(move || {
                measure_once(Some(&path), SystemTime::now(), || {
                    running.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(300));
                    Ok(MAP.to_string())
                })
            })
        };
        ran.recv().unwrap();
        // Asked while the first one runs: it waits, and gets that map.
        let waited = measure_once(Some(&path), SystemTime::now(), || {
            panic!("measured a second time")
        });
        assert_eq!(waited.unwrap().json, MAP);
        assert_eq!(first.join().unwrap().unwrap().json, MAP);
        // Asked after it: measured again.
        let later = MAP.replace("\"private\":5", "\"private\":6");
        let again = measure_once(
            Some(&path),
            SystemTime::now() + Duration::from_secs(2),
            || Ok(later.clone()),
        );
        assert_eq!(again.unwrap().json, later);
        assert_eq!(read_kept(&path).unwrap().json, later);
        // What it can't read isn't kept.
        let bad = measure_once(
            Some(&path),
            SystemTime::now() + Duration::from_secs(4),
            || Ok("[]".into()),
        );
        assert!(bad.is_err());
        assert_eq!(read_kept(&path).unwrap().json, later);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_map_with_its_root_and_sizes_is_readable() {
        assert!(readable(MAP));
        assert!(!readable(
            r#"{"scan":{"root":{"path":"C:\\","totals":{"private":-1}}}}"#
        ));
        assert!(!readable(r#"{"scan":{}}"#));
        assert!(!readable("[]"));
    }
}
