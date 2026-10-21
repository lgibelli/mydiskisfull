//! Clean-ups MyDiskIsFull does itself, when asked (`mdif clean`, a button
//! in the app): only what a suggestion offers, each checked again right
//! before, since the disk may have changed since it was suggested.
//!
//! - A finished worktree ([`worktrees`]) is removed with `git worktree
//!   remove`, which refuses one with changes; its branch stays. Not moved to
//!   the Trash: git would go on listing a worktree whose folder is gone.
//! - Chrome's on-device AI model, once its policy keeps Chrome from
//!   downloading it again, goes to the Trash while Chrome isn't running.
//! - A big file of yours that nothing has opened or changed for months
//!   ([`unused_file`]) goes to the Trash, where Put Back returns it.
//! - A project's build output ([`build_output`]) is deleted: the next build
//!   makes it again, and in the Trash it would still fill the disk. So are a
//!   Rust build's incremental caches alone ([`incremental_caches`]), which
//!   only make the next build of the project's own crates slower.
//! - A project's `node_modules` ([`dependencies`]), when its lockfile says
//!   what `npm install` puts back: deleted, after you're told it downloads
//!   what npm hasn't cached.
//! - An app's or a tool's cache folder ([`cache_of`]) is emptied, the folder
//!   itself kept: the app makes what it needs again. Not while the app is
//!   open, nor macOS's own caches, nor one a group of apps shares.
//! - An app's helper process that keeps rewriting the disk (Safari's
//!   `com.apple.WebKit.Networking`) is ended, and the app starts a fresh one
//!   when it needs it: the app itself, and its windows, stay.
//!
//! - The Trash is emptied, after you confirm: what's in it is erased.
//! - The folders macOS updates leave in `/Users/Shared` ([`relocated`]) go
//!   to the Trash through Finder, which asks for an administrator's
//!   password: they belong to the system.
//!
//! What you keep ([`Keep`]) is never touched, and anything else is refused.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::keep::Keep;
use crate::suggest::display;
use crate::tweaks::{self, CHROME_MODEL, Status};
use crate::worktrees::{self, Finished};

/// What a button does, offered with a suggestion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    /// `worktrees`, `trash`, `delete`, `empty` or `restart`.
    pub kind: &'static str,
    /// "Move to Trash", "Remove 8 Worktrees".
    pub label: String,
    /// What to pass to `mdif clean`.
    pub paths: Vec<String>,
    /// Asked first, when it can't be undone.
    pub confirm: Option<String>,
    /// When MyDiskIsFull did it, while it waits to see whether it worked
    /// (a restart): the app shows that instead of the button.
    pub done: Option<i64>,
}

/// One clean-up, checked.
#[derive(Debug, Clone)]
pub enum Step {
    RemoveWorktree(Finished),
    /// To the Trash once `quit` (a process name) isn't running. Without one,
    /// a big file nobody uses ([`unused_file`]), checked again right before.
    Trash {
        path: PathBuf,
        quit: Option<&'static str>,
    },
    /// Build output, or a Rust build's incremental caches, deleted.
    Delete(PathBuf),
    /// A cache folder emptied while `owner` (a bundle id or a name) isn't
    /// open.
    Empty {
        path: PathBuf,
        owner: String,
    },
    /// The Trash, emptied: what's in it is erased.
    EmptyTrash(PathBuf),
    /// To the Trash through Finder, which asks for a password: what macOS
    /// updates left in `/Users/Shared`.
    FinderTrash(PathBuf),
    /// `helper`, a process of the app at `app` that keeps rewriting the
    /// disk, ended.
    Restart {
        app: PathBuf,
        helper: String,
        pids: Vec<i32>,
    },
}

impl Step {
    pub fn path(&self) -> &Path {
        match self {
            Step::RemoveWorktree(f) => &f.path,
            Step::Trash { path, .. }
            | Step::Delete(path)
            | Step::Empty { path, .. }
            | Step::EmptyTrash(path)
            | Step::FinderTrash(path) => path,
            Step::Restart { app, .. } => app,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Done {
    pub path: PathBuf,
    /// `removed`, `trashed`, `deleted`, `emptied`, `erased` (the Trash) or
    /// `restarted`.
    pub how: &'static str,
    /// Where it went in the Trash.
    pub to: Option<PathBuf>,
}

/// What cleaning `path` means, or why it isn't done. `finished` are the
/// finished worktrees as they are now; `churning`, the processes that keep
/// rewriting the disk now (open churn incidents).
pub fn plan(
    path: &Path,
    home: &Path,
    keep: &Keep,
    finished: &[Finished],
    churning: &[String],
) -> Result<Step, String> {
    let shown = display(path, home);
    if path.extension().is_some_and(|e| e == "app") {
        // The helper that writes the most is this app's: another app's
        // helper of the same name isn't the one suggested.
        return churning
            .iter()
            .filter(|name| helper(name).is_some_and(|a| a.bundle == path))
            .find_map(|name| {
                let pids = helpers_of(name, path);
                (!pids.is_empty()).then(|| Step::Restart {
                    app: path.to_path_buf(),
                    helper: name.clone(),
                    pids,
                })
            })
            .ok_or_else(|| format!("{shown}: none of its helpers keeps rewriting the disk now."));
    }
    if path == home.join(".Trash") {
        return deletable(path, home).map(|()| Step::EmptyTrash(path.to_path_buf()));
    }
    if let Some(entry) = keep.covering(path).or_else(|| keep.inside(path)) {
        return Err(format!(
            "{shown}: you keep {}, so it isn't touched.",
            crate::text::printable(entry)
        ));
    }
    if relocated(path, Path::new(SHARED)) {
        return Ok(Step::FinderTrash(path.to_path_buf()));
    }
    // Minecraft's game folder, saved worlds and all, after you confirm.
    if minecraft(path) {
        return deletable(path, home).map(|()| Step::Trash {
            path: path.to_path_buf(),
            quit: None,
        });
    }
    // A folder a handler says can go (a cloud folder nothing syncs any more).
    if path
        .strip_prefix(home)
        .is_ok_and(crate::handlers::trashable)
    {
        return deletable(path, home).map(|()| Step::Trash {
            path: path.to_path_buf(),
            quit: None,
        });
    }
    if let Some(f) = finished.iter().find(|f| f.path == path) {
        return Ok(Step::RemoveWorktree(f.clone()));
    }
    if path == home.join(CHROME_MODEL) {
        let applied = tweaks::find("chrome-ai-model")
            .is_some_and(|t| matches!(t.status(home), Ok(Status::Applied)));
        if !applied {
            return Err(format!(
                "{shown}: Chrome would download it again. `mdif tweaks apply chrome-ai-model` \
                 stops that first."
            ));
        }
        return Ok(Step::Trash {
            path: path.to_path_buf(),
            quit: Some("Google Chrome"),
        });
    }
    if path.join(".git").is_file() {
        return Err(format!(
            "{shown} isn't a finished worktree now: it has changes, isn't merged, or was used \
             lately."
        ));
    }
    if build_output(path) || incremental_caches(path) || dependencies(path) {
        return deletable(path, home).map(|()| Step::Delete(path.to_path_buf()));
    }
    if let Some(owner) = cache_of(path, home) {
        return deletable(path, home).map(|()| Step::Empty {
            path: path.to_path_buf(),
            owner,
        });
    }
    // A file: one of yours nobody uses. (A link isn't followed.)
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
        return unused_file(path, home, BIG_FILE, crate::watch::unix_now()).map(|()| Step::Trash {
            path: path.to_path_buf(),
            quit: None,
        });
    }
    Err(format!(
        "{shown}: MyDiskIsFull only cleans what it suggests (`mdif suggest`)."
    ))
}

/// Does it.
pub fn run(step: &Step, home: &Path) -> Result<Done, String> {
    match step {
        Step::RemoveWorktree(f) => worktrees::remove(f).map(|()| Done {
            path: f.path.clone(),
            how: "removed",
            to: None,
        }),
        Step::Trash { path, quit } => {
            match quit {
                Some(quit) if running(quit) => {
                    return Err(format!(
                        "Quit {quit} first: it uses {}.",
                        display(path, home)
                    ));
                }
                Some(_) => {}
                // Still one of yours that nobody uses, and not a link put
                // in its place.
                None => unused_file(path, home, BIG_FILE, crate::watch::unix_now())?,
            }
            mdif_macos::fs::trash(path)
                .map(|to| Done {
                    path: path.clone(),
                    how: "trashed",
                    to: Some(to),
                })
                .map_err(|e| format!("{}: {e}", display(path, home)))
        }
        Step::Restart { app, helper, pids } => {
            use mdif_macos::proc::{alive, force_quit, terminate};
            // Only what is still that app's helper: a pid can be reused.
            let now = helpers_of(helper, app);
            let pids: Vec<i32> = pids.iter().copied().filter(|p| now.contains(p)).collect();
            if pids.is_empty() {
                return Err(format!(
                    "{}: {helper} isn't running now.",
                    display(app, home)
                ));
            }
            for &pid in &pids {
                let _ = terminate(pid);
            }
            // WebKit's helpers don't end when asked while they have work
            // under way: they are forced then, as Force Quit does, and the
            // app starts a fresh one as it would after a crash.
            if !ended(&pids, std::time::Duration::from_secs(3)) {
                for &pid in pids.iter().filter(|&&p| alive(p)) {
                    let _ = force_quit(pid);
                }
            }
            if !ended(&pids, std::time::Duration::from_secs(2)) {
                return Err(format!("{}: {helper} didn't end.", display(app, home)));
            }
            Ok(Done {
                path: app.clone(),
                how: "restarted",
                to: None,
            })
        }
        Step::Delete(path) => {
            // As checked: a symlink put in its place since isn't followed.
            deletable(path, home)?;
            if path.file_name() == Some(INCREMENTAL.as_ref()) && !incremental_caches(path) {
                return Err(format!(
                    "{} isn't a Rust build's incremental caches now.",
                    display(path, home)
                ));
            }
            if path.file_name() == Some("node_modules".as_ref()) && !dependencies(path) {
                return Err(format!(
                    "{} isn't a project's packages with a lockfile now.",
                    display(path, home)
                ));
            }
            std::fs::remove_dir_all(path)
                .map(|()| Done {
                    path: path.clone(),
                    how: "deleted",
                    to: None,
                })
                .map_err(|e| format!("{}: {e}", display(path, home)))
        }
        Step::EmptyTrash(path) => {
            deletable(path, home)?;
            empty(path)
                .map(|()| Done {
                    path: path.clone(),
                    how: "erased",
                    to: None,
                })
                .map_err(|e| format!("The Trash: {e}"))
        }
        Step::FinderTrash(path) => finder_trash(std::slice::from_ref(path))
            .map(|()| Done {
                path: path.clone(),
                how: "trashed",
                to: None,
            })
            .map_err(|e| format!("{}: {e}", path.display())),
        Step::Empty { path, owner } => {
            deletable(path, home)?;
            let shown = display(path, home);
            if cache_of(path, home).as_ref() != Some(owner) {
                return Err(format!("{shown} isn't an app's cache folder now."));
            }
            if let Some(app) = open_app(owner) {
                return Err(format!("Quit {app} first: it's using {shown}."));
            }
            if let Some(name) = running_from(path) {
                return Err(format!("Quit {name} first: it runs from {shown}."));
            }
            empty(path)
                .map(|()| Done {
                    path: path.clone(),
                    how: "emptied",
                    to: None,
                })
                .map_err(|e| format!("{shown}: {e}"))
        }
    }
}

/// Where macOS updates leave what they moved aside.
pub const SHARED: &str = "/Users/Shared";

/// A file of yours this big (allocated bytes) ...
pub const BIG_FILE: u64 = 50_000_000;

/// ... that nothing has opened or changed for this long is offered for the
/// Trash ([`unused_file`]).
pub const UNUSED_FOR: i64 = 90 * 86_400;

/// When a file was last used, as far as can be told: the later of when it
/// was last read (`atime`) and last changed (`mtime`). The access time
/// alone misses uses: macOS volumes aren't mounted `strictatime`, so the
/// file system skips some of its updates for speed, and a file can be
/// changed without being read.
pub fn last_used(atime: i64, mtime: i64) -> i64 {
    atime.max(mtime)
}

/// Folders macOS shows as one file (packages), by extension, lowercase:
/// what's in one belongs to the app that made it (a Photos library, a
/// virtual machine, a Final Cut library, a disk image's bands).
const PACKAGES: &[&str] = &[
    "app",
    "appex",
    "aplibrary",
    "band",
    "bundle",
    "component",
    "docarchive",
    "download",
    "dsym",
    "fcpbundle",
    "framework",
    "imovielibrary",
    "kext",
    "key",
    "logicx",
    "lrdata",
    "mdimporter",
    "mpkg",
    "musiclibrary",
    "numbers",
    "pages",
    "photolibrary",
    "photoslibrary",
    "pkg",
    "playground",
    "plugin",
    "prefpane",
    "pvm",
    "qlgenerator",
    "rtfd",
    "saver",
    "scptd",
    "sparsebundle",
    "tvlibrary",
    "utm",
    "vmwarevm",
    "workflow",
    "xcarchive",
    "xcodeproj",
    "xcworkspace",
    "xpc",
];

/// Packages known by name, without an extension.
const PACKAGE_NAMES: &[&str] = &["Photo Booth Library"];

/// Folders of installed packages, besides build output: Python's.
const INSTALLED: &[&str] = &["site-packages", "dist-packages"];

/// Parts of programs, by extension: libraries, Java archives, compiled
/// code. Without one, what it belongs to stops working.
const PROGRAM_PARTS: &[&str] = &[
    "a", "class", "dll", "dylib", "jar", "node", "o", "pyd", "rlib", "snapshot", "so",
];

/// Whether `path` (as you know it: `/Users/me/...`) is among your own
/// files: in the home folder `home` or in [`SHARED`], but not in
/// `~/Library` (apps' data), `~/Applications` (what's installed there) or a
/// developer tool's folder (`~/fvm`, `~/go`), in a hidden folder
/// (`~/.cache`, a project's `.git`) or hidden itself, in a package
/// ([`PACKAGES`]: what's in one is its app's), in a project's build output
/// or dependencies ([`ARTIFACTS`](crate::owners::ARTIFACTS), Python's
/// `site-packages` in a virtual environment of any name), nor a part of a
/// program ([`PROGRAM_PARTS`]), nor in what macOS updates moved aside in
/// [`SHARED`]. Names only: nothing is read.
pub fn yours(path: &Path, home: &Path) -> bool {
    let (rel, in_home) = match (path.strip_prefix(home), path.strip_prefix(SHARED)) {
        (Ok(rel), _) => (rel, true),
        (_, Ok(rel)) => (rel, false),
        _ => return false,
    };
    // Lowercase: names compare without case, as on the Mac's own volume.
    let mut names = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => names.push(n.to_string_lossy().to_lowercase()),
            _ => return false,
        }
    }
    let Some((file, folders)) = names.split_last() else {
        return false;
    };
    let first = folders.first().map(String::as_str);
    let tool = |n: &str| {
        crate::space::DEV_HOME
            .iter()
            .any(|t| t.eq_ignore_ascii_case(n))
    };
    if in_home && first.is_some_and(|n| n == "library" || n == "applications" || tool(n)) {
        return false;
    }
    // What macOS updates moved aside (`Previously Relocated Items 3`).
    if !in_home
        && first.is_some_and(|n| {
            n.starts_with("relocated items") || n.starts_with("previously relocated items")
        })
    {
        return false;
    }
    let package = |n: &str| {
        PACKAGE_NAMES.iter().any(|p| p.eq_ignore_ascii_case(n))
            || Path::new(n)
                .extension()
                .is_some_and(|e| PACKAGES.contains(&&*e.to_string_lossy()))
    };
    let artifact = |n: &str| {
        crate::owners::ARTIFACTS
            .iter()
            .any(|a| a.eq_ignore_ascii_case(n))
            || INSTALLED.contains(&n)
    };
    let program = Path::new(file)
        .extension()
        .is_some_and(|e| PROGRAM_PARTS.contains(&&*e.to_string_lossy()));
    if program {
        return false;
    }
    !names.iter().any(|n| n.starts_with('.')) && !folders.iter().any(|n| artifact(n) || package(n))
}

/// Whether moving `file` to the Trash takes only a file of yours that
/// nothing uses: a regular file (not a link) with one name, of at least
/// `min` allocated bytes, among your own files ([`yours`]) in the home
/// folder and on its volume, neither opened nor changed ([`last_used`]) in
/// the [`UNUSED_FOR`] before `now`. Why not, when it isn't.
pub fn unused_file(file: &Path, home: &Path, min: u64, now: i64) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let shown = display(file, home);
    if file
        .components()
        .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!("{shown}: give the file's full path."));
    }
    let meta = std::fs::symlink_metadata(file).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(format!(
            "{shown} isn't a file (a link to one isn't followed)."
        ));
    }
    if let Some(link) = link_on_the_way(file, home) {
        return Err(format!(
            "{shown} is reached through a link ({}), which isn't followed.",
            display(link, home)
        ));
    }
    if !file.starts_with(home) || !yours(file, home) {
        return Err(format!(
            "{shown}: MyDiskIsFull only moves your own files to the Trash, in your home \
             folder: not apps' data or installed software, nor what's in a package, a hidden \
             folder or build output."
        ));
    }
    if meta.nlink() > 1 {
        return Err(format!(
            "{shown} has other names (hard links): moving one to the Trash frees nothing."
        ));
    }
    if meta.blocks() * 512 < min {
        return Err(format!(
            "{shown} is smaller than {}.",
            crate::units::bytes(min)
        ));
    }
    if now - last_used(meta.atime(), meta.mtime()) < UNUSED_FOR {
        return Err(format!(
            "{shown} was opened or changed in the last {} days.",
            UNUSED_FOR / 86_400
        ));
    }
    let same_volume = mdif_macos::fs::dev_of(file).ok() == mdif_macos::fs::dev_of(home).ok();
    if !same_volume {
        return Err(format!("{shown} is on another volume."));
    }
    Ok(())
}

/// Whether `dir` is a folder a macOS update left in `shared`: `Relocated
/// Items` or `Previously Relocated Items` (with a number), a real folder the
/// system owns. Each update moves aside the files it didn't recognise (old
/// copies of system settings), and they stay there.
pub fn relocated(dir: &Path, shared: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let numbered = |n: &str| {
        n.is_empty()
            || n.strip_prefix(' ')
                .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
    };
    let named = name == "Relocated Items"
        || name
            .strip_prefix("Previously Relocated Items")
            .is_some_and(numbered);
    // The system's own; in tests, the temp folder's owner's.
    let owner = if cfg!(test) {
        unsafe { libc::getuid() }
    } else {
        0
    };
    named
        && dir.parent() == Some(shared)
        && std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && m.uid() == owner)
}

/// Those folders in `shared`, oldest first.
pub fn relocated_in(shared: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(shared) else {
        return Vec::new();
    };
    let mut out: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| relocated(p, shared))
        .map(|p| {
            let t = std::fs::symlink_metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// Moves `paths` to the Trash with Finder, all at once, so Finder asks for
/// a password once. Each is passed as an argument, never in the script.
pub fn finder_trash(paths: &[PathBuf]) -> Result<(), String> {
    const SCRIPT: &str = "on run argv
set found to {}
repeat with p in argv
set end of found to (POSIX file (p as text)) as alias
end repeat
tell application \"Finder\" to delete found
end run";
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(SCRIPT)
        .args(paths)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(if err.contains("-128") {
        "cancelled.".into()
    } else if err.contains("-1743") {
        "MyDiskIsFull may not ask Finder: allow it in System Settings → Privacy & Security → \
         Automation."
            .into()
    } else {
        err.trim().to_string()
    })
}

/// Apple's apps whose caches are theirs alone. macOS's other caches are left
/// to macOS: a service may spend hours making one again (`mediaanalysisd`
/// analyses the whole photo library anew).
const APPLE_APPS: &[&str] = &[
    "com.apple.Safari",
    "com.apple.dt.Xcode",
    "com.apple.Music",
    "com.apple.TV",
    "com.apple.podcasts",
];

/// macOS's caches in `~/Library/Caches` that aren't named after a bundle id.
const MACOS_CACHES: &[&str] = &[
    "CloudKit",
    "GeoServices",
    "FamilyCircle",
    "familycircled",
    "Metadata",
    "PassKit",
    "SiriTTS",
];

/// Whose cache `dir` is (a bundle id or a name), when it is an app's or a
/// tool's cache folder in `home` that it fills again: `~/Library/Caches/<x>`,
/// an app's container's `Library/Caches`, `~/Library/Application
/// Support/<app>/Cache` (or `Caches`, `Code Cache`, `GPUCache`) and
/// `~/.cache/<tool>`. Not macOS's own, nor a group container's, which several
/// apps share.
pub fn cache_of(dir: &Path, home: &Path) -> Option<String> {
    let rel = dir.strip_prefix(home).ok()?;
    // An app's web caches its handler names (Teams', Slack's).
    if let Some(app) = crate::handlers::cache_owner(rel) {
        return Some(app.to_string());
    }
    let c: Vec<&str> = rel.iter().map(|n| n.to_str()).collect::<Option<_>>()?;
    let owner = match c.as_slice() {
        ["Library", "Caches", x] | [".cache", x] => *x,
        ["Library", "Containers", id, "Data", "Library", "Caches"] => *id,
        [
            "Library",
            "Application Support",
            app,
            "Cache" | "Caches" | "Code Cache" | "GPUCache",
        ] => *app,
        _ => return None,
    };
    let apple = (owner.starts_with("com.apple.") && !APPLE_APPS.contains(&owner))
        || MACOS_CACHES.contains(&owner);
    (!apple && !owner.starts_with('.')).then(|| owner.to_string())
}

/// The name of the open app `owner` names (a bundle id, or an app's name),
/// as Launch Services knows it.
fn open_app(owner: &str) -> Option<String> {
    let bundle_id = owner.contains('.') && owner.starts_with(|c: char| c.is_ascii_lowercase());
    let key = if bundle_id { "bundleid" } else { "name" };
    let out = std::process::Command::new("/usr/bin/lsappinfo")
        .args(["find", &format!("{key}={owner}")])
        .output()
        .ok()?;
    // `ASN:0x0-0x11011-"Safari":`, or nothing.
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with("ASN:"))?;
    Some(line.split('"').nth(1).unwrap_or(owner).to_string())
}

/// A process of yours whose program is in `dir` (a tool's runtime kept in
/// its cache).
fn running_from(dir: &Path) -> Option<String> {
    mdif_macos::proc::own_processes()
        .into_iter()
        .find(|p| mdif_macos::proc::path(p.pid).is_some_and(|x| x.starts_with(dir)))
        .map(|p| p.name)
}

/// Deletes what's in `dir`, not `dir`: apps expect their cache folder to be
/// there. Links are removed, not followed. What can't be deleted (a file
/// only its app may touch) stays; an error only if nothing could be.
fn empty(dir: &Path) -> std::io::Result<()> {
    let mut first_error = None;
    let mut any = false;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let gone = if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match gone {
            Ok(()) => any = true,
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match first_error {
        Some(e) if !any => Err(e),
        _ => Ok(()),
    }
}

/// Whether `dir` is Minecraft's game folder: a real folder with its
/// launcher's profiles, and saved worlds or the versions it downloaded.
pub fn minecraft(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
        && dir.join("launcher_profiles.json").is_file()
        && (dir.join("saves").is_dir() || dir.join("versions").is_dir())
}

/// The saved worlds in a Minecraft game folder, by name.
pub fn minecraft_worlds(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir.join("saves"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Whether `dir` is a project's `node_modules` that its package manager puts
/// back as it was: next to `package.json` and a lockfile, with nothing in it
/// that git tracks. Unlike build output it comes back by downloading what
/// isn't in the package manager's cache, which the button says.
pub fn dependencies(dir: &Path) -> bool {
    const LOCKS: &[&str] = &[
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "bun.lock",
    ];
    let Some(project) = dir.parent() else {
        return false;
    };
    dir.file_name() == Some("node_modules".as_ref())
        && crate::privacy::may_open(project)
        && project.join("package.json").is_file()
        && LOCKS.iter().any(|l| project.join(l).is_file())
        && dir.is_dir()
        && worktrees::tracks_files_in(dir) != Some(true)
}

/// Whether `dir` is a project's build output, which its next build makes
/// again: a folder named in [`ARTIFACTS`](crate::owners::ARTIFACTS) next to
/// the file its toolchain builds from, with nothing in it that git tracks.
/// Not `node_modules`, a virtualenv or `Pods`: those come back only by
/// downloading them again.
pub fn build_output(dir: &Path) -> bool {
    const GRADLE: &[&str] = &[
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ];
    let (Some(name), Some(project)) = (dir.file_name().and_then(|n| n.to_str()), dir.parent())
    else {
        return false;
    };
    let markers: &[&str] = match name {
        "target" => &["Cargo.toml"],
        ".dart_tool" => &["pubspec.yaml"],
        "build" => &[
            "pubspec.yaml",
            "package.json",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        ".gradle" => GRADLE,
        ".next" | ".turbo" | "dist" => &["package.json"],
        ".build" => &["Package.swift"],
        _ => return false,
    };
    debug_assert!(crate::owners::ARTIFACTS.contains(&name));
    crate::privacy::may_open(project)
        && markers.iter().any(|m| project.join(m).is_file())
        && dir.is_dir()
        && worktrees::tracks_files_in(dir) != Some(true)
}

const INCREMENTAL: &str = "incremental";

/// Whether `dir` is a Rust build's incremental caches:
/// `target/<profile>/incremental`, or `target/<triple>/<profile>/incremental`
/// when cross-compiling, in a project's build output ([`build_output`]).
/// Deleting them only makes the next build of the project's own crates
/// slower: what it builds and its dependencies stay. Every folder from
/// `target` down is a real folder, not a link to one.
pub fn incremental_caches(dir: &Path) -> bool {
    if dir.file_name() != Some(INCREMENTAL.as_ref()) {
        return false;
    }
    let Some(target) = dir
        .ancestors()
        .skip(2)
        .take(2)
        .find(|a| a.file_name().is_some_and(|n| n == "target"))
    else {
        return false;
    };
    let real = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    dir.ancestors()
        .take_while(|a| *a != target)
        .chain([target])
        .all(real)
        && build_output(target)
}

/// Whether deleting `dir` deletes only what it seems to: a real folder (not
/// a symlink to one), in the home folder, on its volume.
fn deletable(dir: &Path, home: &Path) -> Result<(), String> {
    let shown = display(dir, home);
    // `..` would make "in the home folder" and what you keep mean nothing.
    if dir.components().any(|c| {
        !matches!(
            c,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        )
    }) {
        return Err(format!("{shown}: give the folder's full path."));
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!(
            "{shown} isn't a folder (a link to one isn't followed)."
        ));
    }
    if dir == home || !dir.starts_with(home) {
        return Err(format!(
            "{shown}: MyDiskIsFull only deletes in your home folder."
        ));
    }
    // Nor through a link on the way: `~/devel/link/target` with `link`
    // pointing elsewhere is in the home folder by name only.
    if let Some(link) = link_on_the_way(dir, home) {
        return Err(format!(
            "{shown} is reached through a link ({}), which isn't followed.",
            display(link, home)
        ));
    }
    let same_volume = mdif_macos::fs::dev_of(dir).ok() == mdif_macos::fs::dev_of(home).ok();
    if !same_volume {
        return Err(format!("{shown} is on another volume."));
    }
    Ok(())
}

/// The first folder between `home` and `path` that is a link, if one is
/// (none for a path outside `home`, which is refused for that).
fn link_on_the_way<'p>(path: &'p Path, home: &Path) -> Option<&'p Path> {
    if !path.starts_with(home) {
        return None;
    }
    path.ancestors()
        .skip(1)
        .take_while(|a| *a != home)
        .find(|a| std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()))
}

/// The app a process named `name` works for when it is an app's helper,
/// not the app: of those running, the one that wrote the most.
pub fn helper(name: &str) -> Option<mdif_macos::proc::App> {
    let top = mdif_macos::proc::own_processes()
        .into_iter()
        .filter(|p| p.name == name)
        .max_by_key(|p| p.bytes_written)?;
    mdif_macos::proc::responsible_app(top.pid).filter(|a| a.pid != top.pid && a.name != name)
}

/// Whether all of `pids` ended within `wait`.
fn ended(pids: &[i32], wait: std::time::Duration) -> bool {
    let until = std::time::Instant::now() + wait;
    loop {
        if !pids.iter().any(|&p| mdif_macos::proc::alive(p)) {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The processes named `name` that work for the app at `app`.
fn helpers_of(name: &str, app: &Path) -> Vec<i32> {
    mdif_macos::proc::own_processes()
        .into_iter()
        .filter(|p| p.name == name)
        .filter(|p| {
            mdif_macos::proc::responsible_app(p.pid)
                .is_some_and(|a| a.pid != p.pid && a.bundle == app)
        })
        .map(|p| p.pid)
        .collect()
}

/// The button for a helper that keeps rewriting the disk: "Restart
/// Safari's Networking" (the last part of a dotted name).
pub fn restart_action(helper: &str, app: &mdif_macos::proc::App) -> Action {
    let part = helper.rsplit('.').next().unwrap_or(helper);
    Action {
        kind: "restart",
        done: None,
        label: format!("Restart {}'s {part}", app.name),
        paths: vec![app.bundle.to_string_lossy().into_owned()],
        confirm: Some(format!(
            "This ends {0}'s {helper} process, forcing it if it doesn't end by itself; {0} \
             starts a fresh one when it needs it. Its windows and tabs stay; a page still \
             loading loads again, and a download under way stops.",
            app.name
        )),
    }
}

fn running(name: &str) -> bool {
    mdif_macos::proc::own_processes()
        .iter()
        .any(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_deleted_through_a_link_on_the_way() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let elsewhere = tempfile::tempdir().unwrap();
        let project = elsewhere.path().join("project");
        std::fs::create_dir_all(project.join("target/debug")).unwrap();
        std::fs::write(project.join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(h.join("devel")).unwrap();
        std::os::unix::fs::symlink(&project, h.join("devel/link")).unwrap();
        let through = h.join("devel/link/target");
        let why = plan(&through, h, &Keep::default(), &[], &[]).unwrap_err();
        assert!(why.contains("through a link"), "{why}");
        assert!(deletable(&through, h).is_err());
        // The same folder for real, in the home folder, is fine.
        std::fs::create_dir_all(h.join("devel/real/target/debug")).unwrap();
        std::fs::write(h.join("devel/real/Cargo.toml"), "").unwrap();
        assert!(deletable(&h.join("devel/real/target"), h).is_ok());
    }

    #[test]
    fn minecrafts_game_folder_goes_to_the_trash_with_its_worlds_named() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let game = h.join("Library/Application");
        std::fs::create_dir_all(game.join("saves/New World")).unwrap();
        std::fs::create_dir_all(game.join("saves/Island")).unwrap();
        std::fs::write(game.join("launcher_profiles.json"), "{}").unwrap();
        let step = plan(&game, h, &Keep::default(), &[], &[]).unwrap();
        assert!(matches!(step, Step::Trash { quit: None, .. }));
        assert_eq!(minecraft_worlds(&game), ["Island", "New World"]);
        // Kept, or not Minecraft's: refused.
        let keep = Keep::parse("~/Library/Application\n", h);
        assert!(plan(&game, h, &keep, &[], &[]).is_err());
        std::fs::remove_file(game.join("launcher_profiles.json")).unwrap();
        assert!(plan(&game, h, &Keep::default(), &[], &[]).is_err());
    }

    #[test]
    fn a_cloud_folder_left_behind_goes_to_the_trash_and_a_live_one_stays() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let cloud = h.join("Library/CloudStorage");
        let old = cloud.join("iCloudDrive-iCloudDrive (16-03-26 15:16)");
        let live = cloud.join("OneDrive-Example");
        for d in [&old, &live] {
            std::fs::create_dir_all(d.join("Documents")).unwrap();
        }
        let step = plan(&old, h, &Keep::default(), &[], &[]).unwrap();
        assert!(matches!(step, Step::Trash { quit: None, .. }));
        assert!(plan(&live, h, &Keep::default(), &[], &[]).is_err());
        assert!(plan(&old.join("Documents"), h, &Keep::default(), &[], &[]).is_err());
    }

    #[test]
    fn what_macos_updates_left_is_found_by_name_and_the_trash_is_emptied() {
        let shared = tempfile::tempdir().unwrap();
        let s = shared.path();
        for n in [
            "Relocated Items",
            "Previously Relocated Items",
            "Previously Relocated Items 12",
            "Previously Relocated Items x",
            "Relocated Items copy",
        ] {
            std::fs::create_dir_all(s.join(n)).unwrap();
        }
        std::os::unix::fs::symlink(
            s.join("Relocated Items"),
            s.join("Previously Relocated Items 3"),
        )
        .unwrap();
        let mut found: Vec<String> = relocated_in(s)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                "Previously Relocated Items",
                "Previously Relocated Items 12",
                "Relocated Items"
            ]
        );
        assert!(!relocated(&s.join("Relocated Items/Relocated Items"), s));

        // The Trash: emptied, kept as a folder.
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let trash = h.join(".Trash");
        std::fs::create_dir_all(trash.join("old folder")).unwrap();
        std::fs::write(trash.join("a.zip"), "x").unwrap();
        let step = plan(&trash, h, &Keep::default(), &[], &[]).unwrap();
        assert!(matches!(step, Step::EmptyTrash(_)));
        assert_eq!(run(&step, h).unwrap().how, "erased");
        assert!(trash.is_dir() && std::fs::read_dir(&trash).unwrap().count() == 0);
    }

    #[test]
    fn your_own_files_are_told_by_their_place() {
        let me = Path::new("/Users/me");
        for p in [
            "/Users/me/Movies/trip.mov",
            "/Users/me/Desktop/old.dmg",
            "/Users/me/Documents/2019/backup.zip",
            "/Users/me/Downloads/installer.dmg",
            "/Users/me/devel/app/data/model.bin",
            "/Users/me/devel/app/logs/driver.log",
            "/Users/me/Movies/Trip.fcpbundle",
            "/Users/me/Downloads/setup.exe",
            "/Users/Shared/old.pkg",
        ] {
            assert!(yours(Path::new(p), me), "{p}");
        }
        for p in [
            "/Users/me",
            "/Users/me/Library/Containers/x/Data/big.db",
            "/Users/me/library/Caches/x.bin",
            "/Users/me/Applications/Gateway 10.45/data/big.bin",
            "/Users/me/fvm/versions/stable/bin/cache/flutter_tester",
            "/Users/me/go/pkg/mod/cache/x.zip",
            "/Users/me/devel/ds/venv-jupyter/lib/python3.12/site-packages/x/data.bin",
            "/Users/me/tools/jadx/lib/jadx-all.jar",
            "/Users/me/Downloads/libbig.DYLIB",
            "/Users/me/.cache/huggingface/model.bin",
            "/Users/me/.Trash/old.dmg",
            "/Users/me/Movies/.hidden.mov",
            "/Users/me/devel/app/.git/objects/pack/p.pack",
            "/Users/me/devel/app/target/debug/app",
            "/Users/me/devel/web/Node_Modules/x/big.node",
            "/Users/me/devel/app/build/out.apk",
            "/Users/me/Pictures/Photos Library.photoslibrary/originals/a.mov",
            "/Users/me/Movies/Trip.FCPBundle/Media/a.mov",
            "/Users/me/Virtual Machines/Win.vmwarevm/disk.vmdk",
            "/Users/me/Backup.sparsebundle/bands/1f",
            "/Users/me/Pictures/Photo Booth Library/Pictures/a.mov",
            "/Users/me/Movies/../Library/x.bin",
            "/Users/Shared/Previously Relocated Items 2/x.bin",
            "/Users/other/Movies/theirs.mov",
            "/Applications/Big.app/Contents/x",
        ] {
            assert!(!yours(Path::new(p), me), "{p}");
        }
    }

    #[test]
    fn a_big_file_nobody_uses_goes_to_the_trash_and_nothing_like_it() {
        use std::time::{Duration, UNIX_EPOCH};
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let none = Keep::default();
        const DAY: i64 = 86_400;
        let now = crate::watch::unix_now();
        let age = |p: &Path, read: i64, changed: i64| {
            let at = |days: i64| UNIX_EPOCH + Duration::from_secs((now - days * DAY) as u64);
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_accessed(at(read))
                        .set_modified(at(changed)),
                )
                .unwrap();
        };
        // One real file as big as it takes (written, so not sparse), moved
        // from place to place.
        let movie = h.join("Movies/old.mov");
        std::fs::create_dir_all(movie.parent().unwrap()).unwrap();
        std::fs::write(&movie, vec![7u8; BIG_FILE as usize]).unwrap();
        std::fs::File::open(&movie).unwrap().sync_all().unwrap();
        age(&movie, 200, 900);
        let accepted = |p: &Path, keep: &Keep| {
            matches!(
                plan(p, h, keep, &[], &[]),
                Ok(Step::Trash { quit: None, ref path }) if path == p
            )
        };
        let refused = |p: &Path, keep: &Keep| plan(p, h, keep, &[], &[]).unwrap_err();
        assert!(accepted(&movie, &none));

        // Opened, or changed, lately.
        age(&movie, 10, 900);
        assert!(refused(&movie, &none).contains("in the last 90 days"));
        age(&movie, 400, 30);
        assert!(refused(&movie, &none).contains("in the last 90 days"));
        age(&movie, 91, 91);
        assert!(accepted(&movie, &none));

        // Kept, itself or by its folder.
        let keep = Keep::parse("~/Movies/old.mov\n", h);
        assert!(refused(&movie, &keep).contains("you keep"));
        let keep = Keep::parse("~/Movies\n", h);
        assert!(refused(&movie, &keep).contains("you keep"));

        // Not yours: an app's data, a package's, build output, packages,
        // a hidden folder.
        let mut at = movie.clone();
        for rel in [
            "Library/Application Support/Player/old.mov",
            "Movies/Trip.fcpbundle/Media/old.mov",
            "devel/web/node_modules/big/old.mov",
            "devel/web/.git/lfs/old.mov",
            ".ollama/models/old.mov",
        ] {
            let to = h.join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::rename(&at, &to).unwrap();
            at = to;
            assert!(
                refused(&at, &none).contains("only moves your own files"),
                "{rel}"
            );
        }
        std::fs::rename(&at, &movie).unwrap();
        assert!(accepted(&movie, &none));

        // A folder, a link to the file, a second name for it, a small file,
        // a file outside the home folder.
        assert!(refused(&h.join("Movies"), &none).contains("only cleans what it suggests"));
        let link = h.join("Movies/link.mov");
        std::os::unix::fs::symlink(&movie, &link).unwrap();
        assert!(refused(&link, &none).contains("only cleans what it suggests"));
        let again = h.join("Movies/again.mov");
        std::fs::hard_link(&movie, &again).unwrap();
        assert!(refused(&movie, &none).contains("hard links"));
        std::fs::remove_file(&again).unwrap();
        let small = h.join("Documents/small.bin");
        std::fs::create_dir_all(small.parent().unwrap()).unwrap();
        std::fs::write(&small, vec![7u8; 1_000_000]).unwrap();
        age(&small, 900, 900);
        assert!(refused(&small, &none).contains("smaller than 50 MB"));
        let away = tempfile::tempdir().unwrap();
        let outside = away.path().join("old.mov");
        std::fs::rename(&movie, &outside).unwrap();
        assert!(refused(&outside, &none).contains("only moves your own files"));
        std::fs::rename(&outside, &movie).unwrap();
        assert!(refused(&h.join("Movies/../Movies/old.mov"), &none).contains("full path"));

        // Checked again when it's done: used since, or a link put in its
        // place, and it stays where it is.
        let step = plan(&movie, h, &none, &[], &[]).unwrap();
        age(&movie, 0, 900);
        assert!(run(&step, h).unwrap_err().contains("in the last 90 days"));
        assert!(movie.is_file());
        age(&movie, 200, 900);
        let step = plan(&movie, h, &none, &[], &[]).unwrap();
        let aside = h.join("Movies/aside.mov");
        std::fs::rename(&movie, &aside).unwrap();
        std::os::unix::fs::symlink(&aside, &movie).unwrap();
        assert!(run(&step, h).unwrap_err().contains("isn't a file"));
        assert!(aside.is_file() && movie.is_symlink());
    }

    #[test]
    fn node_modules_goes_only_with_a_lockfile() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let none = Keep::default();
        let app = h.join("devel/web");
        let modules = app.join("node_modules");
        std::fs::create_dir_all(modules.join("left-pad")).unwrap();
        std::fs::write(app.join("package.json"), "{}").unwrap();
        // Without a lockfile, `npm install` might put back other versions.
        assert!(!dependencies(&modules));
        assert!(plan(&modules, h, &none, &[], &[]).is_err());
        std::fs::write(app.join("package-lock.json"), "{}").unwrap();
        assert!(dependencies(&modules));
        let step = plan(&modules, h, &none, &[], &[]).unwrap();
        assert!(matches!(step, Step::Delete(_)));
        assert_eq!(run(&step, h).unwrap().how, "deleted");
        assert!(!modules.exists() && app.join("package.json").exists());
        // Not one elsewhere, nor one you keep.
        let stray = h.join("devel/node_modules");
        std::fs::create_dir_all(&stray).unwrap();
        assert!(!dependencies(&stray));
        std::fs::create_dir_all(&modules).unwrap();
        let keep = Keep::parse("~/devel/web\n", h);
        assert!(plan(&modules, h, &keep, &[], &[]).is_err());
    }

    #[test]
    fn an_apps_cache_is_emptied_and_macos_own_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let none = Keep::default();
        let at = |rel: &str| h.join(rel);
        assert_eq!(
            cache_of(&at("Library/Caches/com.example.chat"), h).as_deref(),
            Some("com.example.chat")
        );
        assert_eq!(
            cache_of(
                &at("Library/Containers/com.apple.Safari/Data/Library/Caches"),
                h
            )
            .as_deref(),
            Some("com.apple.Safari")
        );
        assert_eq!(
            cache_of(&at("Library/Application Support/Chat/Code Cache"), h).as_deref(),
            Some("Chat")
        );
        assert_eq!(cache_of(&at(".cache/tool"), h).as_deref(), Some("tool"));
        for theirs in [
            "Library/Caches/com.apple.mediaanalysisd",
            "Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches",
            "Library/Caches/CloudKit",
            "Library/Group Containers/ABCDE.shared/Library/Caches",
            "Library/Caches",
            "Library/Application Support/Chat",
        ] {
            assert_eq!(cache_of(&at(theirs), h), None, "{theirs}");
        }

        // Emptied, the folder itself kept; a link in it removed, not followed.
        let cache = at("Library/Caches/com.example.mdif-test-not-an-app");
        std::fs::create_dir_all(cache.join("sub/deeper")).unwrap();
        std::fs::write(cache.join("a"), "x").unwrap();
        std::fs::write(cache.join("sub/deeper/b"), "x").unwrap();
        let outside = at("Documents/kept.txt");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, "mine").unwrap();
        std::os::unix::fs::symlink(&outside, cache.join("link")).unwrap();
        let step = plan(&cache, h, &none, &[], &[]).unwrap();
        assert!(
            matches!(&step, Step::Empty { owner, .. } if owner == "com.example.mdif-test-not-an-app")
        );
        assert_eq!(run(&step, h).unwrap().how, "emptied");
        assert!(cache.is_dir());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "mine");

        // Not macOS's own, not one you keep, not a link to a folder.
        let apple = at("Library/Caches/com.apple.mediaanalysisd");
        std::fs::create_dir_all(&apple).unwrap();
        assert!(plan(&apple, h, &none, &[], &[]).is_err());
        let keep = Keep::parse("~/Library/Caches/com.example.mdif-test-not-an-app\n", h);
        assert!(plan(&cache, h, &keep, &[], &[]).is_err());
        let linked = at("Library/Caches/com.example.linked");
        std::os::unix::fs::symlink(at("Documents"), &linked).unwrap();
        let err = plan(&linked, h, &none, &[], &[]).unwrap_err();
        assert!(err.contains("isn't a folder"), "{err}");
        assert!(outside.exists());
    }

    #[test]
    fn only_what_is_suggested_is_cleaned_and_nothing_kept() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let model = h.join(CHROME_MODEL);
        std::fs::create_dir_all(&model).unwrap();
        let worktree = Finished {
            path: h.join("devel/app/.claude/worktrees/a"),
            repo: h.join("devel/app"),
            into: "main".into(),
            agents: true,
            touched: 0,
        };
        let finished = [worktree.clone()];
        let none = Keep::default();

        assert!(matches!(
            plan(&worktree.path, h, &none, &finished, &[]),
            Ok(Step::RemoveWorktree(_))
        ));
        // The model goes only once Chrome can't download it again (its
        // profile is installed for the whole account, not in this home).
        let applied = matches!(
            tweaks::find("chrome-ai-model").unwrap().status(h),
            Ok(Status::Applied)
        );
        match plan(&model, h, &none, &finished, &[]) {
            Ok(Step::Trash { quit, .. }) if applied => assert_eq!(quit, Some("Google Chrome")),
            Err(e) if !applied => assert!(e.contains("download it again"), "{e}"),
            other => panic!("applied {applied}: {other:?}"),
        }
        // Anything else is refused, worktree or not.
        std::fs::create_dir_all(h.join("devel/app/.claude/worktrees/b")).unwrap();
        std::fs::write(h.join("devel/app/.claude/worktrees/b/.git"), "gitdir: x").unwrap();
        let err = plan(
            &h.join("devel/app/.claude/worktrees/b"),
            h,
            &none,
            &finished,
            &[],
        )
        .unwrap_err();
        assert!(err.contains("isn't a finished worktree"), "{err}");
        let err = plan(&h.join("Documents"), h, &none, &finished, &[]).unwrap_err();
        assert!(err.contains("only cleans what it suggests"), "{err}");
        // An app is restarted only through a helper that keeps rewriting the
        // disk now, and only that app's.
        let app = Path::new("/Applications/Safari.app");
        let err = plan(app, h, &none, &finished, &[]).unwrap_err();
        assert!(err.contains("none of its helpers"), "{err}");
        let err = plan(app, h, &none, &finished, &["no-such-helper".into()]).unwrap_err();
        assert!(err.contains("none of its helpers"), "{err}");
        // What you keep, or a folder with something kept in it, isn't touched.
        let keep = Keep::parse("~/devel/app/.claude/worktrees/a/target\n", h);
        let err = plan(&worktree.path, h, &keep, &finished, &[]).unwrap_err();
        assert!(
            err.contains("you keep ~/devel/app/.claude/worktrees/a/target"),
            "{err}"
        );
    }

    #[test]
    fn build_output_is_deleted_and_nothing_that_only_looks_like_it() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let project = |rel: &str, marker: &str, out: &str| {
            let p = h.join(rel);
            std::fs::create_dir_all(p.join(out).join("debug")).unwrap();
            std::fs::write(p.join(out).join("debug/app"), vec![1u8; 10_000]).unwrap();
            if !marker.is_empty() {
                std::fs::write(p.join(marker), "").unwrap();
            }
            p.join(out)
        };
        let target = project("devel/app", "Cargo.toml", "target");
        let none = Keep::default();
        assert!(matches!(
            plan(&target, h, &none, &[], &[]),
            Ok(Step::Delete(_))
        ));
        let built = project("devel/flutter", "pubspec.yaml", "build");
        assert!(matches!(
            plan(&built, h, &none, &[], &[]),
            Ok(Step::Delete(_))
        ));

        // Without the file its toolchain builds from, it's just a folder;
        // node_modules comes back only by downloading it.
        let refused = |p: &Path, keep: &Keep| plan(p, h, keep, &[], &[]).unwrap_err();
        let plain = project("devel/notes", "", "target");
        assert!(refused(&plain, &none).contains("only cleans what it suggests"));
        let modules = project("devel/web", "package.json", "node_modules");
        assert!(refused(&modules, &none).contains("only cleans what it suggests"));
        assert!(refused(&h.join("devel/app"), &none).contains("only cleans what it suggests"));

        // What you keep, in it or around it.
        let keep = Keep::parse("~/devel/app/target/debug\n", h);
        assert!(refused(&target, &keep).contains("you keep ~/devel/app/target/debug"));
        let keep = Keep::parse("~/devel/app\n", h);
        assert!(refused(&target, &keep).contains("you keep ~/devel/app"));

        // A link to build output isn't followed, `..` doesn't leave the
        // home folder, and build output elsewhere isn't touched.
        std::fs::create_dir_all(h.join("devel/link")).unwrap();
        std::fs::write(h.join("devel/link/Cargo.toml"), "").unwrap();
        std::os::unix::fs::symlink(&target, h.join("devel/link/target")).unwrap();
        assert!(refused(&h.join("devel/link/target"), &none).contains("isn't a folder"));
        let sneaky = h.join("devel/app/../app/target");
        assert!(refused(&sneaky, &none).contains("full path"));
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(elsewhere.path().join("target")).unwrap();
        std::fs::write(elsewhere.path().join("Cargo.toml"), "").unwrap();
        assert!(refused(&elsewhere.path().join("target"), &none).contains("in your home folder"));

        // Deleting it.
        let step = plan(&target, h, &none, &[], &[]).unwrap();
        let done = run(&step, h).unwrap();
        assert_eq!(done.how, "deleted");
        assert!(!target.exists() && h.join("devel/app/Cargo.toml").exists());
        assert!(h.join("devel/link/target").symlink_metadata().is_ok());
    }

    #[test]
    fn a_rust_builds_incremental_caches_are_deleted_alone() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let make = |rel: &str| {
            let p = h.join(rel);
            std::fs::create_dir_all(p.join("app-1x2y3z")).unwrap();
            std::fs::write(p.join("app-1x2y3z/query-cache.bin"), vec![1u8; 10_000]).unwrap();
            p
        };
        std::fs::create_dir_all(h.join("devel/app/src-tauri")).unwrap();
        std::fs::write(h.join("devel/app/src-tauri/Cargo.toml"), "").unwrap();
        let debug = make("devel/app/src-tauri/target/debug/incremental");
        let cross = make("devel/app/src-tauri/target/x86_64-pc-windows-msvc/release/incremental");
        let none = Keep::default();
        for p in [&debug, &cross] {
            assert!(incremental_caches(p), "{}", p.display());
            assert!(
                matches!(plan(p, h, &none, &[], &[]), Ok(Step::Delete(_))),
                "{}",
                p.display()
            );
        }
        let refused = |p: &Path, keep: &Keep| plan(p, h, keep, &[], &[]).unwrap_err();

        // Not in a Rust project's target, or not where a profile keeps them.
        let loose = make("devel/notes/target/debug/incremental");
        assert!(refused(&loose, &none).contains("only cleans what it suggests"));
        std::fs::write(h.join("devel/app/src-tauri/cache.toml"), "").unwrap();
        let elsewhere = make("devel/app/src-tauri/cache/debug/incremental");
        assert!(refused(&elsewhere, &none).contains("only cleans what it suggests"));
        let shallow = make("devel/app/src-tauri/target/incremental");
        assert!(refused(&shallow, &none).contains("only cleans what it suggests"));
        let deep = make("devel/app/src-tauri/target/a/b/c/incremental");
        assert!(refused(&deep, &none).contains("only cleans what it suggests"));

        // What you keep, in it or around it.
        let keep = Keep::parse("~/devel/app/src-tauri/target/debug\n", h);
        assert!(refused(&debug, &keep).contains("you keep"));
        let keep = Keep::parse("~/devel/app/src-tauri/target/debug/incremental/app-*\n", h);
        assert!(refused(&debug, &keep).contains("you keep"));

        // A link anywhere from target down isn't followed.
        let outside = tempfile::tempdir().unwrap();
        let real = outside.path().join("debug");
        std::fs::create_dir_all(real.join("incremental/x")).unwrap();
        std::fs::create_dir_all(h.join("devel/app/src-tauri/target/linked")).unwrap();
        std::os::unix::fs::symlink(&real, h.join("devel/app/src-tauri/target/linked/debug"))
            .unwrap();
        let via_link = h.join("devel/app/src-tauri/target/linked/debug/incremental");
        assert!(!incremental_caches(&via_link));
        std::fs::create_dir_all(h.join("devel/app/src-tauri/target/beta")).unwrap();
        std::os::unix::fs::symlink(
            real.join("incremental"),
            h.join("devel/app/src-tauri/target/beta/incremental"),
        )
        .unwrap();
        let link = h.join("devel/app/src-tauri/target/beta/incremental");
        assert!(refused(&link, &none).contains("only cleans what it suggests"));
        // Outside the home folder.
        let away = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(away.path().join("target/debug/incremental")).unwrap();
        std::fs::write(away.path().join("Cargo.toml"), "").unwrap();
        let away = away.path().join("target/debug/incremental");
        assert!(incremental_caches(&away));
        assert!(refused(&away, &none).contains("in your home folder"));

        // Deleting them leaves the rest of the build.
        let step = plan(&debug, h, &none, &[], &[]).unwrap();
        assert_eq!(run(&step, h).unwrap().how, "deleted");
        assert!(!debug.exists() && h.join("devel/app/src-tauri/target/debug").is_dir());
        assert!(real.join("incremental/x").is_dir());
    }
}
