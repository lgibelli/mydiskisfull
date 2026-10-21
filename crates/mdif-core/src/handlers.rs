//! What apps keep, folder by folder: where an app's bulk is (Signal's
//! attachments, the old Outlook's mailbox, iPhone backups), what it is, and
//! where you make it smaller, in the app or in macOS. `mdif space` puts the
//! biggest one in an item with the item; the app has a button to open the
//! place. Only what's known to work is said: a way that isn't sure isn't
//! given.

use std::path::Path;

use serde::Serialize;

/// Where a handler's way is: an app, or a page of System Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Open {
    /// "Open Signal".
    pub label: &'static str,
    /// The app's bundle id.
    pub app: Option<&'static str>,
    /// A URL to open instead (System Settings).
    pub url: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handler {
    /// From the home folder, `/`-separated; `*` stands for any one name.
    pub path: &'static str,
    /// "Signal's attachments".
    pub title: &'static str,
    /// What's in it.
    pub what: &'static str,
    /// How to make it smaller.
    pub how: &'static str,
    /// Folders in it (`*` for any one name) that only cache what downloads
    /// again, which `mdif clean` empties while the app (`open.app`) is closed.
    pub empty: &'static [&'static str],
    /// Whether the folder itself can go to the Trash once you've checked
    /// it (a cloud folder nothing syncs any more): `mdif clean` moves it.
    pub trash: bool,
    pub open: Option<Open>,
}

const fn app(label: &'static str, id: &'static str) -> Option<Open> {
    Some(Open {
        label,
        app: Some(id),
        url: None,
    })
}

const ICLOUD: Option<Open> = Some(Open {
    label: "Open iCloud Settings…",
    app: None,
    url: Some("x-apple.systempreferences:com.apple.systempreferences.AppleIDSettings"),
});

const STORAGE: Option<Open> = Some(Open {
    label: "Open Storage Settings…",
    app: None,
    url: Some("x-apple.systempreferences:com.apple.settings.Storage"),
});

pub const HANDLERS: &[Handler] = &[
    // Cloud storage: what each keeps on this Mac besides the cloud, and how
    // to keep less of it. A dated folder is first: it's no account's now.
    Handler {
        path: "Library/CloudStorage/*-* (*)",
        title: "A cloud folder left behind",
        what: "A copy macOS kept when this cloud account's folder was removed or reset (the \
               date in its name). Nothing syncs it any more.",
        how: "Check that what you need in it is in the cloud, or in the account's current \
              folder; then it can go to the Trash.",
        empty: &[],
        trash: true,
        open: None,
    },
    Handler {
        path: "Library/Mobile Documents",
        title: "iCloud Drive's downloads",
        what: "Files from iCloud Drive kept on this Mac as well as in iCloud.",
        how: "With Optimize Mac Storage on (System Settings → your name → iCloud → iCloud \
              Drive), macOS removes the downloads of files you don't open when space runs low. \
              In Finder, Remove Download frees a file or folder now; it stays in iCloud.",
        empty: &[],
        trash: false,
        open: ICLOUD,
    },
    Handler {
        path: "Library/CloudStorage/OneDrive-*",
        title: "OneDrive's downloads",
        what: "Files from OneDrive kept on this Mac as well as in the cloud.",
        how: "In Finder, Free Up Space on a file or folder keeps it in OneDrive only, and Always \
              Keep on This Device does the opposite. A file you open downloads again.",
        empty: &[],
        trash: false,
        open: app("Open OneDrive", "com.microsoft.OneDrive"),
    },
    Handler {
        path: "Library/CloudStorage/GoogleDrive-*",
        title: "Google Drive's downloads",
        what: "Files from Google Drive kept on this Mac as well as in the cloud.",
        how: "In Google Drive → Settings → Google Drive, Stream Files keeps files in the cloud \
              and downloads them when you open them; Mirror Files keeps all of them here. In \
              Finder, a folder made available offline can be made online-only again.",
        empty: &[],
        trash: false,
        open: app("Open Google Drive", "com.google.drivefs"),
    },
    Handler {
        path: "Library/CloudStorage/Dropbox*",
        title: "Dropbox's downloads",
        what: "Files from Dropbox kept on this Mac as well as in the cloud.",
        how: "In Finder, Make Online-Only on a file or folder keeps it in Dropbox only. Dropbox's \
              settings can make new files online-only too.",
        empty: &[],
        trash: false,
        open: app("Open Dropbox", "com.getdropbox.dropbox"),
    },
    Handler {
        path: "Library/CloudStorage/Box-Box*",
        title: "Box's offline files",
        what: "Files from Box kept on this Mac: Box Drive keeps the rest online until opened.",
        how: "Folders made available offline stay here; in Finder, turning that off for a folder \
              frees it.",
        empty: &[],
        trash: false,
        open: app("Open Box", "com.box.desktop"),
    },
    Handler {
        path: "Library/Application Support/Signal/attachments.noindex",
        title: "Signal's attachments",
        what: "Every photo, video and file from your Signal chats, kept on this Mac.",
        how: "In Signal, delete the chats or the media you don't need (a chat's details list \
              its media), or clear them all in Settings → Chats. Disappearing messages keep \
              new chats from piling up.",
        empty: &[],
        trash: false,
        open: app("Open Signal", "org.whispersystems.signal-desktop"),
    },
    Handler {
        path: "Library/Messages/Attachments",
        title: "Messages' attachments",
        what: "Photos, videos and files from your conversations.",
        how: "System Settings → General → Storage → Messages lists the biggest to delete. In \
              Messages → Settings → General, Keep Messages for 1 Year drops older ones.",
        empty: &[],
        trash: false,
        open: STORAGE,
    },
    Handler {
        path: "Library/Mail",
        title: "Mail's mailboxes",
        what: "Mail's copy of your mailboxes. Mail kept on a server downloads again when you \
               open it.",
        how: "An account you read elsewhere can go from Mail → Settings → Accounts: its mail \
              stays on the server.",
        empty: &[],
        trash: false,
        open: app("Open Mail", "com.apple.mail"),
    },
    Handler {
        path: "Library/Application Support/MobileSync/Backup",
        title: "iPhone and iPad backups",
        what: "Backups made by connecting a device to this Mac.",
        how: "System Settings → General → Storage → iOS Files lists them, with their dates, to \
              delete the ones you don't need. Backups in iCloud aren't touched.",
        empty: &[],
        trash: false,
        open: STORAGE,
    },
    Handler {
        path: "Library/Group Containers/UBF8T346G9.Office/Outlook/Outlook 15 Profiles",
        title: "The old Outlook's mailbox",
        what: "The old Outlook's copy of your mail. The new Outlook keeps its own, so this is \
               only needed to go back to the old one, or for mail that is nowhere else.",
        how: "Check that your accounts' old mail is in Outlook, then move the profile's Data \
              folder to the Trash; empty the Trash once Outlook still has everything.",
        empty: &[],
        trash: false,
        open: app("Open Outlook", "com.microsoft.Outlook"),
    },
    Handler {
        path: "Library/Containers/com.microsoft.teams2/Data/Library/Application Support/Microsoft/MSTeams/EBWebView",
        title: "Teams' web cache",
        what: "What Teams' built-in browser keeps of chats and meetings.",
        how: "Empty Caches empties the parts that download again, while Teams is closed; its \
              messages and sign-in stay. Microsoft's way to clear everything is to delete \
              `~/Library/Containers/com.microsoft.teams2`, which signs you out.",
        empty: &[
            "*/Cache",
            "*/Code Cache",
            "*/GPUCache",
            "*/Service Worker/CacheStorage",
            "*/Service Worker/ScriptCache",
        ],
        trash: false,
        open: app("Open Teams", "com.microsoft.teams2"),
    },
    Handler {
        path: "Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram",
        title: "Telegram's media",
        what: "Photos, videos and files Telegram downloaded: they stay in Telegram's cloud.",
        how: "Telegram → Settings → Data and Storage → Storage Usage clears them, and can limit \
              how long they're kept.",
        empty: &[],
        trash: false,
        open: app("Open Telegram", "ru.keepcoder.Telegram"),
    },
    Handler {
        path: "Library/Application Support/Spotify/PersistentCache",
        title: "Spotify's cache",
        what: "Music Spotify keeps to play it again without downloading it.",
        how: "Spotify → Settings → Storage → Clear cache. Songs you downloaded stay.",
        empty: &[],
        trash: false,
        open: app("Open Spotify", "com.spotify.client"),
    },
    Handler {
        path: "Library/Application Support/Slack",
        title: "Slack's data",
        what: "Slack's caches and what it keeps of your workspaces.",
        how: "Slack → Help → Troubleshooting → Clear Cache and Restart.",
        empty: &[
            "Cache",
            "Code Cache",
            "GPUCache",
            "Service Worker/CacheStorage",
        ],
        trash: false,
        open: app("Open Slack", "com.tinyspeck.slackmacgap"),
    },
    Handler {
        path: "Library/Metadata/CoreSpotlight/DocumentProcessing",
        title: "Spotlight's processing",
        what: "Spotlight working through what it indexed (key phrases, suggestions). It grows \
               after a migration or a big import, and shrinks as Spotlight catches up.",
        how: "macOS's own: deleting it makes Spotlight start over. If it keeps growing for \
              weeks, rebuilding the Spotlight index is the fix.",
        empty: &[],
        trash: false,
        open: None,
    },
    Handler {
        path: ".android/avd",
        title: "Android emulators",
        what: "The Android emulators' disks, one per virtual device.",
        how: "Android Studio → Device Manager deletes the devices you don't use.",
        empty: &[],
        trash: false,
        open: app("Open Android Studio", "com.google.android.studio"),
    },
    Handler {
        path: "Pictures/*.photoslibrary",
        title: "Photos library",
        what: "Your photos and videos, full size.",
        how: "With iCloud Photos, Photos → Settings → iCloud → Optimize Mac Storage keeps the \
              full-size originals in iCloud only and smaller copies here.",
        empty: &[],
        trash: false,
        open: app("Open Photos", "com.apple.Photos"),
    },
];

/// Whose cache `rel` (from the home folder) is, when it's one of a
/// handler's folders to empty: its app's bundle id.
pub fn cache_owner(rel: &Path) -> Option<&'static str> {
    HANDLERS.iter().find_map(|h| {
        let app = h.open?.app?;
        h.empty
            .iter()
            .any(|e| matches(&format!("{}/{e}", h.path), rel))
            .then_some(app)
    })
}

/// Whether `rel` (from the home folder) is a folder a handler says can go
/// to the Trash.
pub fn trashable(rel: &Path) -> bool {
    find(rel).is_some_and(|h| h.trash)
}

/// The handler for `rel` (from the home folder), if one knows it.
pub fn find(rel: &Path) -> Option<&'static Handler> {
    HANDLERS.iter().find(|h| matches(h.path, rel))
}

fn matches(pattern: &str, rel: &Path) -> bool {
    let mut names = rel.iter();
    for p in pattern.split('/') {
        let Some(name) = names.next().and_then(|n| n.to_str()) else {
            return false;
        };
        if !glob(p, name) {
            return false;
        }
    }
    names.next().is_none()
}

/// One name against a pattern where each `*` stands for any run of
/// characters.
fn glob(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for m in middle {
        match rest.find(m) {
            Some(i) => rest = &rest[i + m.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_known_by_its_place() {
        let signal = Path::new("Library/Application Support/Signal/attachments.noindex");
        assert_eq!(find(signal).unwrap().title, "Signal's attachments");
        assert_eq!(
            find(Path::new("Pictures/Photos Library.photoslibrary"))
                .unwrap()
                .title,
            "Photos library"
        );
        // A cloud folder, and one left behind by an account removed since.
        let cloud = |n: &str| {
            find(&Path::new("Library/CloudStorage").join(n))
                .unwrap()
                .title
        };
        assert_eq!(cloud("OneDrive-Personal"), "OneDrive's downloads");
        assert_eq!(
            cloud("GoogleDrive-me@example.com"),
            "Google Drive's downloads"
        );
        assert_eq!(cloud("Dropbox"), "Dropbox's downloads");
        assert_eq!(
            cloud("GoogleDrive-me@example.com (11-10-24 10:55)"),
            "A cloud folder left behind"
        );
        assert_eq!(
            cloud("iCloudDrive-iCloudDrive (16-03-26 15:16)"),
            "A cloud folder left behind"
        );
        assert!(glob("a*b*c", "aXbYc") && glob("a*", "a") && !glob("a*b", "ab c"));
        // A handler's caches belong to its app.
        let teams = "Library/Containers/com.microsoft.teams2/Data/Library/Application Support/\
                     Microsoft/MSTeams/EBWebView";
        assert_eq!(
            cache_owner(&Path::new(teams).join("WV2Profile_tfw/Service Worker/CacheStorage")),
            Some("com.microsoft.teams2")
        );
        assert_eq!(
            cache_owner(&Path::new(teams).join("WV2Profile_tfw/IndexedDB")),
            None
        );
        // Not what's in it, nor what's around it.
        assert!(find(&signal.join("ab")).is_none());
        assert!(find(Path::new("Library/Application Support/Signal")).is_none());
        assert!(find(Path::new("Pictures/Photos Library")).is_none());
        for h in HANDLERS {
            assert!(
                !h.path.starts_with('/') && !h.path.ends_with('/'),
                "{}",
                h.path
            );
            assert!(h.what.ends_with('.') && h.how.ends_with('.'), "{}", h.path);
            if let Some(o) = h.open {
                assert!(o.app.is_some() != o.url.is_some(), "{}", h.path);
            }
        }
    }
}
