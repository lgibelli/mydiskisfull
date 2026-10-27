//! Folders macOS guards with a prompt.
//!
//! Without Full Disk Access, opening Desktop, Documents, Downloads, iCloud
//! Drive, a cloud provider's folder or another app's data shows a dialog, and
//! the `open` waits until someone answers it. A background process can't
//! wait on that: the watcher once stopped sampling for as long as the dialog
//! stayed unanswered. So the watcher, and `mdif status` (which the app runs
//! every few minutes), leave those folders alone until they have Full Disk
//! Access. Other protected places (Mail, Safari, the Trash) fail quietly
//! instead, and need no care.

use std::ffi::OsStr;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::map::DATA_ROOT;

static AVOID: AtomicBool = AtomicBool::new(false);

/// Held by tests that switch [`avoid_prompts`] on, and by those that need it
/// off: it is process-wide, and tests run in parallel.
#[cfg(test)]
pub(crate) static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// From now on (`true`), skip what would show a prompt; set by background
/// work that lacks Full Disk Access, and cleared once it has it.
pub fn avoid_prompts(avoid: bool) {
    AVOID.store(avoid, Ordering::Relaxed);
}

/// Whether prompts are being avoided: then nothing may be looked up where
/// it might be in a guarded folder, as a moved folder found by number.
pub fn avoiding() -> bool {
    AVOID.load(Ordering::Relaxed)
}

/// Whether `path` may be opened: always, unless prompts are being avoided
/// and it would show one.
pub fn may_open(path: &Path) -> bool {
    !AVOID.load(Ordering::Relaxed) || !prompts(path)
}

/// Whether opening `path` without Full Disk Access shows a privacy prompt:
/// inside a user's Desktop, Documents or Downloads, iCloud Drive, cloud
/// storage, another app's container, or on another volume (removable and
/// network volumes prompt; an internal one under /Volumes can't be told
/// apart by its path).
pub fn prompts(path: &Path) -> bool {
    let path = path.strip_prefix(DATA_ROOT).map_or(path, |rest| rest);
    if path.starts_with("/Volumes") && path.components().count() > 2 {
        return true;
    }
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect();
    // /Users/<name>/...
    let [users, _, rest @ ..] = names.as_slice() else {
        return false;
    };
    if *users != "Users" {
        return false;
    }
    match rest {
        [top, ..]
            if ["Desktop", "Documents", "Downloads"]
                .map(OsStr::new)
                .contains(top) =>
        {
            true
        }
        [lib, place, ..] if *lib == "Library" => {
            let inside = rest.len() > 2;
            match place.to_str() {
                Some("Mobile Documents" | "CloudStorage") => true,
                // Listing the containers is fine; looking inside one isn't.
                Some("Containers" | "Group Containers") => inside,
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folders_that_prompt() {
        for p in [
            "/Users/me/Documents",
            "/Users/me/Desktop/a/b",
            "/Users/me/Downloads/x.dmg",
            "/Users/me/Library/Mobile Documents/com~apple~CloudDocs",
            "/Users/me/Library/CloudStorage/OneDrive-Personal",
            "/Users/me/Library/Containers/net.whatsapp.WhatsApp/Data",
            "/Users/me/Library/Group Containers/group.x/Library",
            "/System/Volumes/Data/Users/me/Documents/a",
            "/Volumes/Backup/Users",
        ] {
            assert!(prompts(Path::new(p)), "{p}");
        }
        for p in [
            "/Users/me",
            "/Users/me/devel/project",
            "/Users/me/Library/Caches/Google",
            "/Users/me/Library/Containers",
            "/Users/me/Library/Group Containers",
            "/Users/me/Documentsx",
            "/private/var/folders/x/T/Documents",
            "/Applications",
            "/Volumes",
        ] {
            assert!(!prompts(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn avoided_only_when_asked() {
        let _off_after = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let docs = Path::new("/Users/me/Documents/x");
        avoid_prompts(true);
        assert!(!may_open(docs));
        assert!(may_open(Path::new("/Users/me/devel")));
        avoid_prompts(false);
        assert!(may_open(docs));
    }
}
