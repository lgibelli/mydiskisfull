//! Android's emulators and the system images they start from, cleaned when
//! asked (`mdif clean`, the app's Clean Up):
//!
//! - A system image no emulator uses
//!   (`~/Library/Android/sdk/system-images/android-34/google_apis/arm64-v8a`),
//!   deleted: Android Studio downloads it again when an emulator needs it.
//! - An emulator (`~/.android/avd/<id>.avd`, listed by the `<id>.ini` beside
//!   it), deleted as Device Manager deletes one: the apps installed in it,
//!   their data and its settings go. Offered once nothing has started it for
//!   a month; never while it runs.
//!
//! Read from the emulators' own files (`config.ini`: which image, what it's
//! called) and the times of the files in their folders, not from Android
//! Studio, which may not be installed. Where they are: Android's own
//! variables when they're set (`ANDROID_AVD_HOME`, `ANDROID_HOME`), else
//! Android Studio's defaults.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::clean::Step;
use crate::space::run_within;
use crate::text::printable;

/// The emulators, from the home folder, unless `ANDROID_AVD_HOME` says.
pub const AVDS: &str = ".android/avd";
/// The SDK Android Studio installs, from the home folder, unless
/// `ANDROID_HOME` says.
pub const SDK: &str = "Library/Android/sdk";
/// In the SDK: `android-<api>/<tag>/<abi>`, one folder each.
const IMAGES: &str = "system-images";
/// An emulator nothing started for this long is offered for deletion.
pub const UNUSED: i64 = 30 * 86_400;

/// An emulator (an Android Virtual Device).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Avd {
    /// What `emulator -avd` takes: `Pixel_8_API_34`.
    pub id: String,
    /// As Device Manager shows it.
    pub name: String,
    pub dir: PathBuf,
    /// The `<id>.ini` that lists it, beside its folder.
    pub ini: Option<PathBuf>,
    /// The system image it starts from.
    pub image: Option<PathBuf>,
    /// When a file in its folder last changed: when it last ran.
    pub last_used: Option<i64>,
}

/// Where the emulators are.
pub fn avd_home(home: &Path) -> PathBuf {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("ANDROID_AVD_HOME")
        .or_else(|| var("ANDROID_USER_HOME").map(|d| d.join("avd")))
        .or_else(|| var("ANDROID_EMULATOR_HOME").map(|d| d.join("avd")))
        .unwrap_or_else(|| home.join(AVDS))
}

/// Where the SDK is.
pub fn sdk(home: &Path) -> PathBuf {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("ANDROID_HOME")
        .or_else(|| var("ANDROID_SDK_ROOT"))
        .unwrap_or_else(|| home.join(SDK))
}

/// The emulators, as they are now. An error when one's settings can't be
/// read: which images are in use isn't known then.
pub fn avds(home: &Path) -> Result<Vec<Avd>, String> {
    avds_in(&avd_home(home), &sdk(home))
}

/// The emulators in `dir`, their images in `sdk`: each `<id>.ini` and the
/// folder it names, and folders no `.ini` lists (Device Manager doesn't
/// show those, but they take their space).
pub fn avds_in(dir: &Path, sdk: &Path) -> Result<Vec<Avd>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut inis = Vec::new();
    let mut folders = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        match path.extension().and_then(|x| x.to_str()) {
            Some("ini") if e.file_type().is_ok_and(|t| t.is_file()) => inis.push(path),
            Some("avd") if e.file_type().is_ok_and(|t| t.is_dir()) => folders.push(path),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for ini in inis {
        let Some(id) = ini.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        let listed =
            std::fs::read_to_string(&ini).map_err(|e| format!("{}: {e}", ini.display()))?;
        let dir = value(&listed, "path")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| dir.join(format!("{id}.avd")));
        if !dir.is_dir() {
            // Listed, with nothing left of it.
            continue;
        }
        folders.retain(|f| f != &dir);
        out.push(avd(id, dir, Some(ini), sdk)?);
    }
    for dir in folders {
        let id = dir
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(avd(id, dir, None, sdk)?);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn avd(id: String, dir: PathBuf, ini: Option<PathBuf>, sdk: &Path) -> Result<Avd, String> {
    let config = dir.join("config.ini");
    let settings =
        std::fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let image = value(&settings, "image.sysdir.1").map(|rel| sdk.join(rel.trim_end_matches('/')));
    let name = value(&settings, "avd.ini.displayname")
        .map(str::to_string)
        .unwrap_or_else(|| id.replace('_', " "));
    Ok(Avd {
        last_used: newest(&dir),
        id,
        name,
        dir,
        ini,
        image,
    })
}

/// `key=value` (spaces around `=` allowed), the first line with `key`.
fn value<'a>(ini: &'a str, key: &str) -> Option<&'a str> {
    ini.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key)
            .then(|| v.trim())
            .filter(|v| !v.is_empty())
    })
}

/// When the newest file directly in `dir` was changed: an emulator writes
/// its disk and settings there each time it runs. Metadata only.
fn newest(dir: &Path) -> Option<i64> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
}

/// Whether `path` is a system image's folder in `sdk`:
/// `system-images/android-34/google_apis/arm64-v8a`.
pub fn is_image(path: &Path, sdk: &Path) -> bool {
    path.strip_prefix(sdk.join(IMAGES))
        .is_ok_and(|rel| rel.components().count() == 3 && rel.iter().all(|c| c != ".."))
}

/// "Android 14 system image (Google APIs, arm64-v8a)", from its folder's
/// names: nothing in it is read.
pub fn image_name(path: &Path) -> String {
    let c: Vec<String> = path
        .iter()
        .rev()
        .take(3)
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let [abi, tag, api] = c.as_slice() else {
        return "Android system image".into();
    };
    let level = api.strip_prefix("android-").unwrap_or(api);
    let version = match level {
        "21" => "Android 5.0".to_string(),
        "22" => "Android 5.1".into(),
        "23" => "Android 6".into(),
        "24" => "Android 7.0".into(),
        "25" => "Android 7.1".into(),
        "26" => "Android 8.0".into(),
        "27" => "Android 8.1".into(),
        "28" => "Android 9".into(),
        "29" => "Android 10".into(),
        "30" => "Android 11".into(),
        "31" => "Android 12".into(),
        "32" => "Android 12L".into(),
        "33" => "Android 13".into(),
        "34" => "Android 14".into(),
        "35" => "Android 15".into(),
        "36" => "Android 16".into(),
        l => format!("Android API {l}"),
    };
    let tag = match tag.as_str() {
        "default" => None,
        "google_apis" => Some("Google APIs".to_string()),
        "google_apis_playstore" => Some("Google Play".into()),
        "google_atd" | "aosp_atd" => Some("ATD".into()),
        "android-tv" | "google-tv" => Some("TV".into()),
        "android-wear" | "wear" => Some("Wear OS".into()),
        "android-automotive" | "android-automotive-playstore" => Some("Automotive".into()),
        t => Some(t.replace('_', " ")),
    };
    let what = [tag, Some(printable(abi).into_owned())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} system image ({what})", printable(&version))
}

/// The emulators that use the image at `path`.
pub fn users<'a>(path: &Path, avds: &'a [Avd]) -> Vec<&'a Avd> {
    avds.iter()
        .filter(|a| a.image.as_deref() == Some(path))
        .collect()
}

/// What cleaning `path` means, if it's Android's: a system image, or an
/// emulator's folder.
pub fn plan(path: &Path, home: &Path) -> Option<Result<Step, String>> {
    let (dir, sdk) = (avd_home(home), sdk(home));
    if is_image(path, &sdk) {
        return Some(image_step(path, home, &dir, &sdk));
    }
    if path.parent() == Some(dir.as_path()) && path.extension().is_some_and(|x| x == "avd") {
        return Some(avd_step(path, home, &dir, &sdk));
    }
    None
}

fn image_step(path: &Path, home: &Path, dir: &Path, sdk: &Path) -> Result<Step, String> {
    crate::clean::deletable(path, home)?;
    let name = image_name(path);
    unused_image(path, &name, dir, sdk)?;
    Ok(Step::DeleteImage {
        dir: path.to_path_buf(),
        name,
    })
}

fn unused_image(path: &Path, name: &str, dir: &Path, sdk: &Path) -> Result<(), String> {
    let avds = avds_in(dir, sdk)
        .map_err(|e| format!("Which emulators use the {name} can't be read ({e}), so it stays."))?;
    match users(path, &avds).first() {
        Some(a) => Err(format!(
            "The emulator {} uses the {name}: delete it first, or keep both.",
            printable(&a.name)
        )),
        None => Ok(()),
    }
}

fn avd_step(path: &Path, home: &Path, dir: &Path, sdk: &Path) -> Result<Step, String> {
    crate::clean::deletable(path, home)?;
    let a = avds_in(dir, sdk)?
        .into_iter()
        .find(|a| a.dir == path)
        .ok_or_else(|| format!("{} isn't an Android emulator now.", path.display()))?;
    not_running(&a)?;
    Ok(Step::DeleteAvd {
        dir: a.dir,
        ini: a.ini,
        name: a.name,
    })
}

fn not_running(a: &Avd) -> Result<(), String> {
    let name = printable(&a.name);
    let ids = running()
        .ok_or_else(|| format!("Whether {name} is running can't be told, so it stays."))?;
    if ids.iter().any(|id| id == &a.id || id == "*") {
        return Err(format!("{name} is running: close it first."));
    }
    Ok(())
}

/// The emulators running now, by id (`*` for one whose id can't be told);
/// `None` when the processes can't be listed.
fn running() -> Option<Vec<String>> {
    let mut c = Command::new("/bin/ps");
    c.args(["-x", "-ww", "-o", "command="]);
    let out = run_within(c, Duration::from_secs(10)).filter(|o| o.status.success())?;
    Some(running_in(&String::from_utf8_lossy(&out.stdout)))
}

/// From `ps -o command=`: each emulator's `-avd <id>` or `@<id>`.
fn running_in(ps: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in ps.lines() {
        let mut words = line.split_whitespace();
        let Some(program) = words.next() else {
            continue;
        };
        let program = Path::new(program)
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if !(program.starts_with("qemu-system") || program == "emulator") {
            continue;
        }
        let words: Vec<&str> = words.collect();
        let id = words
            .iter()
            .position(|w| *w == "-avd")
            .and_then(|i| words.get(i + 1))
            .map(|w| w.to_string())
            .or_else(|| {
                words
                    .iter()
                    .find_map(|w| w.strip_prefix('@').map(str::to_string))
            });
        out.push(id.unwrap_or_else(|| "*".into()));
    }
    out
}

/// Deletes the system image `dir` once no emulator uses it, and the
/// folders it leaves empty (`android-34/google_apis`).
pub fn delete_image(dir: &Path, name: &str, home: &Path) -> Result<(), String> {
    crate::clean::deletable(dir, home)?;
    let sdk = sdk(home);
    if !is_image(dir, &sdk) {
        return Err(format!("{} isn't a system image now.", dir.display()));
    }
    unused_image(dir, name, &avd_home(home), &sdk)?;
    std::fs::remove_dir_all(dir).map_err(|e| format!("The {name}: {e}"))?;
    for up in dir.ancestors().skip(1).take(2) {
        // Only while empty.
        if std::fs::remove_dir(up).is_err() {
            break;
        }
    }
    Ok(())
}

/// Deletes the emulator in `dir` and the `.ini` that lists it, unless it's
/// running now.
pub fn delete_avd(dir: &Path, ini: Option<&Path>, name: &str, home: &Path) -> Result<(), String> {
    crate::clean::deletable(dir, home)?;
    let a = avds_in(&avd_home(home), &sdk(home))?
        .into_iter()
        .find(|a| a.dir == dir)
        .ok_or_else(|| format!("{} isn't an Android emulator now.", printable(name)))?;
    not_running(&a)?;
    std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", printable(name)))?;
    if let Some(ini) = ini.filter(|i| a.ini.as_deref() == Some(*i)) {
        std::fs::remove_file(ini).map_err(|e| format!("{}: {e}", ini.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn emulators_are_read_from_their_own_settings() {
        let t = tempfile::tempdir().unwrap();
        let (dir, sdk) = (t.path().join("avd"), t.path().join("sdk"));
        let pixel = dir.join("Pixel_8.avd");
        write(
            &dir.join("Pixel_8.ini"),
            &format!(
                "avd.ini.encoding=UTF-8\npath={}\ntarget=android-34\n",
                pixel.display()
            ),
        );
        write(
            &pixel.join("config.ini"),
            "avd.ini.displayname=Pixel 8\nimage.sysdir.1 = system-images/android-34/google_apis/arm64-v8a/\n",
        );
        // A folder no .ini lists, and an .ini whose folder is gone.
        write(
            &dir.join("old.avd/config.ini"),
            "image.sysdir.1=system-images/android-30/default/x86_64/\n",
        );
        write(
            &dir.join("gone.ini"),
            &format!("path={}\n", dir.join("gone.avd").display()),
        );
        let avds = avds_in(&dir, &sdk).unwrap();
        assert_eq!(avds.len(), 2);
        let p = avds.iter().find(|a| a.id == "Pixel_8").unwrap();
        assert_eq!(p.name, "Pixel 8");
        assert_eq!(p.dir, pixel);
        assert_eq!(p.ini, Some(dir.join("Pixel_8.ini")));
        assert_eq!(
            p.image,
            Some(sdk.join("system-images/android-34/google_apis/arm64-v8a"))
        );
        assert!(p.last_used.is_some());
        let old = avds.iter().find(|a| a.id == "old").unwrap();
        assert_eq!(old.name, "old");
        assert_eq!(old.ini, None);
        assert!(avds_in(&t.path().join("none"), &sdk).unwrap().is_empty());
        // Settings that can't be read: which images are used isn't known.
        std::fs::remove_file(pixel.join("config.ini")).unwrap();
        assert!(avds_in(&dir, &sdk).is_err());
    }

    #[test]
    fn a_system_image_is_named_by_its_folder() {
        let sdk = Path::new("/Users/me/Library/Android/sdk");
        let img = sdk.join("system-images/android-34/google_apis/arm64-v8a");
        assert!(is_image(&img, sdk));
        assert!(!is_image(&sdk.join("system-images/android-34"), sdk));
        assert!(!is_image(&img.join("data"), sdk));
        assert_eq!(
            image_name(&img),
            "Android 14 system image (Google APIs, arm64-v8a)"
        );
        assert_eq!(
            image_name(&sdk.join("system-images/android-30/default/x86_64")),
            "Android 11 system image (x86_64)"
        );
        assert_eq!(
            image_name(&sdk.join("system-images/android-Baklava/google_apis_playstore/arm64-v8a")),
            "Android API Baklava system image (Google Play, arm64-v8a)"
        );
    }

    #[test]
    fn running_emulators_are_told_by_their_command_line() {
        let ps = "/Applications/Safari.app/Contents/MacOS/Safari\n\
                  /Users/me/Library/Android/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -netdelay none -avd Pixel_8 -qt-hide-window\n\
                  /Users/me/Library/Android/sdk/emulator/emulator @Tablet -no-snapshot\n\
                  /opt/qemu/bin/qemu-system-x86_64 -m 2048\n\
                  /bin/zsh -c echo -avd nothing\n";
        assert_eq!(running_in(ps), vec!["Pixel_8", "Tablet", "*"]);
        assert!(running_in("").is_empty());
    }

    #[test]
    fn an_image_an_emulator_uses_stays() {
        let t = tempfile::tempdir().unwrap();
        let (dir, sdk) = (t.path().join("avd"), t.path().join("sdk"));
        let used = sdk.join("system-images/android-34/google_apis/arm64-v8a");
        let unused = sdk.join("system-images/android-33/google_apis/arm64-v8a");
        write(&used.join("system.img"), "");
        write(&unused.join("system.img"), "");
        write(&dir.join("Pixel_8.ini"), "path=/nowhere\n");
        write(
            &dir.join("Pixel_8.avd/config.ini"),
            "avd.ini.displayname=Pixel 8\nimage.sysdir.1=system-images/android-34/google_apis/arm64-v8a/\n",
        );
        let err = unused_image(&used, "Android 14 system image", &dir, &sdk).unwrap_err();
        assert!(err.contains("Pixel 8 uses"), "{err}");
        unused_image(&unused, "Android 13 system image", &dir, &sdk).unwrap();
        let avds = avds_in(&dir, &sdk).unwrap();
        assert_eq!(users(&used, &avds).len(), 1);
        assert!(users(&unused, &avds).is_empty());
    }
}
