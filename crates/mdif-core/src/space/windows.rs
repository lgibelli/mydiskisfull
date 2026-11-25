//! Where the space goes on Windows: how the folders of the Windows drive
//! and of a profile are sorted into kinds (the rules [`super::classify`]
//! follows there), what Windows' own big files and folders are called, and
//! what each is.
//!
//! Compiled on every system, so the rules are tested anywhere; used on
//! Windows. Names are compared as Windows does: case left out.

use super::{Class, DEV_HOME, Kind, What, group, item};
use crate::owners::{ARTIFACTS, CODE_ROOTS, DOTNET_OUTPUT};

/// Windows' own files in the drive's root folder: the name, what it's
/// called, and what it is. Each is an item of its own in Windows' kind.
pub(super) const SYSTEM_FILES: &[(&str, &str, &str)] = &[
    (
        "pagefile.sys",
        "Page file",
        "Windows' virtual memory. Its size is set in Settings → System → About → Advanced \
         system settings → Performance → Virtual memory",
    ),
    (
        "hiberfil.sys",
        "Hibernation file",
        "What hibernation and fast startup start from. `powercfg /hibernate off`, run as \
         administrator, turns both off and deletes it",
    ),
    (
        "swapfile.sys",
        "Swap file",
        "Where Windows puts apps from the Microsoft Store out of memory",
    ),
];

/// Folders apps keep their caches in, by name.
const CACHE_NAMES: &[&str] = &[
    "cache",
    "caches",
    "code cache",
    "gpucache",
    "cacheddata",
    "cachedextensionvsixs",
    "shadercache",
    "dawncache",
    "grshadercache",
    "cache2",
];

/// Developer tools' folders in the profile besides the ones they have on a
/// Mac ([`DEV_HOME`]).
const DEV_PROFILE: &[&str] = &[".nuget", ".vscode", ".docker", ".dotnet", "scoop"];

/// Folders whose projects' build output is sorted as such: the Mac's, and
/// Visual Studio's own (`source\repos`).
fn code_root(name: &str) -> bool {
    one_of(name, CODE_ROOTS) || name.eq_ignore_ascii_case("source")
}

fn one_of(name: &str, list: &[&str]) -> bool {
    list.iter().any(|l| l.eq_ignore_ascii_case(name))
}

fn lower(c: &[String]) -> Vec<String> {
    c.iter().map(|n| n.to_lowercase()).collect()
}

/// A WSL distribution, as the Microsoft Store installs it (the package's
/// folder in `AppData\Local\Packages`).
fn wsl_package(p: &str) -> bool {
    p.starts_with("canonicalgroup")
        || p.starts_with("thedebianproject")
        || p.starts_with("kalilinux")
        || [
            "ubuntu",
            "opensuse",
            "suse",
            "almalinux",
            "oraclelinux",
            "fedora",
        ]
        .iter()
        .any(|d| p.contains(d))
}

/// Extensions of files never yours to move on Windows, wherever they are: a
/// virtual machine's disks and saved state, Outlook's mail stores, drivers
/// (an `.exe` too, outside Downloads, where installers land).
const NOT_YOURS: &[&str] = &[
    "vhd", "vhdx", "avhd", "avhdx", "vmdk", "vdi", "qcow2", "vmem", "vmsn", "vmss", "nvram",
    "vmcx", "vmrs", "vmgs", "pst", "ost", "sys",
];

/// Whether a file of the profile is among your own files as Windows has
/// them: in no folder sorted as anything but your files or media (apps'
/// data, apps, cloud folders, which delete online what goes here, virtual
/// machines, developer tools, caches, mail, build output), and not a
/// virtual machine's disk, a mail store or a program. `names` is its path
/// below the profile, lowercase, the file last. What
/// [`crate::clean::yours`] adds on Windows to its own rules (hidden names,
/// packages, build output and programs' parts by name).
pub(super) fn own_file(names: &[String]) -> bool {
    let Some((file, folders)) = names.split_last() else {
        return false;
    };
    let ext = std::path::Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase());
    if let Some(ext) = ext.as_deref() {
        let downloaded = folders.first().is_some_and(|f| f == "downloads");
        if one_of(ext, NOT_YOURS) || (ext == "exe" && !downloaded) {
            return false;
        }
    }
    (1..=folders.len())
        .all(|n| in_home(&folders[..n]).is_none_or(|c| matches!(c.kind, Kind::Files | Kind::Media)))
}

/// Whether a folder of the profile, by its name (lowercase), is a sync
/// service's: what's deleted there is deleted online. OneDrive's
/// (`OneDrive - Contoso`), Dropbox's (`Dropbox (Personal)`), iCloud's
/// (`iCloudDrive`, `iCloud Photos`) and Creative Cloud's by how their names
/// begin; and those of the services that keep plain files in sync, which
/// nothing else tells: Google Drive's (`My Drive`, its mirror), Box's,
/// Nextcloud's, ownCloud's, MEGA's, Proton Drive's, Synology Drive's,
/// Seafile's, Tresorit's, and Syncthing's and Sync.com's `Sync`.
fn cloud_folder(name: &str) -> bool {
    const BEGIN: &[&str] = &["onedrive", "dropbox", "icloud", "creative cloud files"];
    const NAMES: &[&str] = &[
        "google drive",
        "my drive",
        "box",
        "box sync",
        "nextcloud",
        "owncloud",
        "mega",
        "megasync",
        "proton drive",
        "synologydrive",
        "seafile",
        "tresors",
        "sync",
    ];
    BEGIN.iter().any(|b| name.starts_with(b)) || NAMES.contains(&name)
}

/// How a folder in the profile is sorted: `c` is its path below the profile.
pub(super) fn in_home(c: &[String]) -> Option<Class> {
    use Kind::*;
    let l = lower(c);
    let c: Vec<&str> = l.iter().map(String::as_str).collect();
    match c.as_slice() {
        [] => group(Files),
        ["appdata"] | ["appdata", "local" | "roaming" | "locallow"] => group(Appdata),
        ["appdata", "local", "temp"] => item(Caches, What::Plain),
        ["appdata", "local", "programs"] => group(Apps),
        ["appdata", "local", "programs", _] => item(Apps, What::Plain),
        ["appdata", "local", "packages"] => group(Appdata),
        ["appdata", "local", "packages", p] if wsl_package(p) => item(Vms, What::Vm),
        ["appdata", "local", "packages", _] => item(Appdata, What::Plain),
        [
            "appdata",
            "local",
            "packages",
            _,
            "localcache" | "ac" | "tempstate",
        ] => item(Caches, What::Plain),
        ["appdata", "local", "wsl"] => item(Vms, What::Vm),
        ["appdata", "local", "docker"] => item(Dev, What::Plain),
        ["appdata", "local", "android"] => item(Dev, What::Plain),
        [
            "appdata",
            "local",
            "android",
            "sdk",
            "system-images",
            _,
            _,
            _,
        ] => item(Dev, What::AndroidImage),
        ["appdata", "local", "jetbrains" | "pnpm" | "go-build"] => item(Dev, What::Plain),
        [
            "appdata",
            "local",
            "npm-cache" | "pnpm-cache" | "yarn" | "pip" | "uv" | "nuget" | "ms-playwright"
            | "crashdumps" | "d3dscache" | "nvidia" | "amd",
        ] => item(Caches, What::Plain),
        ["appdata", "local", "microsoft"] | ["appdata", "local", "microsoft", "windows"] => {
            group(Appdata)
        }
        [
            "appdata",
            "local",
            "microsoft",
            "windows",
            "inetcache" | "explorer" | "webcache",
        ] => item(Caches, What::Plain),
        ["appdata", "local", "microsoft", "outlook"] => item(Mail, What::Plain),
        ["appdata", "local", "microsoft", "onedrive"] => item(Apps, What::Plain),
        ["appdata", "local", "google", "drivefs"] => item(Cloud, What::Plain),
        // A browser's cache: `Google\Chrome\User Data\Default\Cache`.
        ["appdata", "local", _, _, "user data", _, x] if one_of(x, CACHE_NAMES) => {
            item(Caches, What::Plain)
        }
        [
            "appdata",
            "local",
            "mozilla",
            "firefox",
            "profiles",
            _,
            "cache2",
        ] => item(Caches, What::Plain),
        ["appdata", "local" | "locallow", _] => item(Appdata, What::Plain),
        ["appdata", "local", "microsoft", "windows", _] | ["appdata", "local", "microsoft", _] => {
            item(Appdata, What::Plain)
        }
        // An Electron app's caches: `Roaming\Code\Cache`.
        ["appdata", "roaming", _, x] if one_of(x, CACHE_NAMES) => item(Caches, What::Plain),
        ["appdata", "roaming", "apple computer", "mobilesync"] | ["apple", "mobilesync"] => {
            item(Appdata, What::Plain)
        }
        ["appdata", "roaming", _] | ["appdata", _] => item(Appdata, What::Plain),
        [d] if cloud_folder(d) => item(Cloud, What::Plain),
        ["pictures", "icloud photos"] => item(Cloud, What::Plain),
        ["pictures" | "music" | "videos"] => group(Media),
        ["pictures" | "music" | "videos", _] => item(Media, What::Plain),
        ["documents", "outlook files"] => item(Mail, What::Plain),
        ["documents", "virtual machines"] | ["virtualbox vms"] => group(Vms),
        ["documents", "virtual machines", _] | ["virtualbox vms", _] => item(Vms, What::Vm),
        [".cache"] => group(Caches),
        [".cache", _] => item(Caches, What::Cache),
        [".android", "avd", x] if x.ends_with(".avd") => item(Dev, What::Avd),
        [t] if one_of(t, DEV_HOME) || one_of(t, DEV_PROFILE) => item(Dev, What::Plain),
        [root] if code_root(root) => item(Files, What::Plain),
        // .NET's only beside a project file, which `space::classify` checks.
        [root, _, .., last]
            if code_root(root) && (one_of(last, ARTIFACTS) || one_of(last, DOTNET_OUTPUT)) =>
        {
            item(Build, What::Artifact)
        }
        [_] => item(Files, What::Plain),
        _ => None,
    }
}

/// How a folder elsewhere on the drive is sorted: `c` is its path below the
/// drive's root (`Windows\WinSxS`).
pub(super) fn in_system(c: &[String]) -> Option<Class> {
    use Kind::*;
    let l = lower(c);
    let c: Vec<&str> = l.iter().map(String::as_str).collect();
    match c.as_slice() {
        // The root's own files are the page and hibernation files.
        [] | ["windows"] => group(Macos),
        ["windows", "temp"] => item(Caches, What::Plain),
        ["windows", _] => item(Macos, What::Plain),
        [
            "windows.old"
            | "$windows.~bt"
            | "$windows.~ws"
            | "$getcurrent"
            | "$sysreset"
            | "$winreagent"
            | "recovery"
            | "perflogs"
            | "system volume information",
        ] => item(Macos, What::Plain),
        ["program files" | "program files (x86)"]
        | ["program files", "windowsapps"]
        | ["xboxgames"] => group(Apps),
        ["program files" | "program files (x86)", _] | ["xboxgames", _] => item(Apps, What::Plain),
        ["program files", "windowsapps", _] => item(Apps, What::Plain),
        ["programdata"] => group(Appdata),
        ["programdata", "microsoft"] => group(Macos),
        ["programdata", "microsoft", "windows", "virtual hard disks"] => item(Vms, What::Vm),
        ["programdata", "microsoft", _] => item(Macos, What::Plain),
        ["programdata", "docker" | "dockerdesktop"] => item(Dev, What::Plain),
        ["programdata", "package cache"] => item(Apps, What::Plain),
        ["programdata", _] => item(Appdata, What::Plain),
        ["$recycle.bin"] => group(Files),
        ["$recycle.bin", _] => item(Files, What::Trash),
        ["users"] => group(Other),
        ["users", "public"] => item(Files, What::Plain),
        [
            "users",
            "public",
            "documents",
            "hyper-v",
            "virtual hard disks",
        ] => item(Vms, What::Vm),
        ["users", "default" | "default user"] => item(Macos, What::Plain),
        ["users", _] | [_] => item(Other, What::Plain),
        _ => None,
    }
}

/// What Windows' own folders and the well-known ones in a profile are
/// called, and what each is: `rel` is the path below the profile when the
/// folder is in it, `abs` the path below the drive's root otherwise.
pub(super) fn known(rel: Option<&[String]>, abs: &[String]) -> Option<(String, &'static str)> {
    let named = |n: &str, what: &'static str| Some((n.to_string(), what));
    if let Some(rel) = rel {
        let l = lower(rel);
        let c: Vec<&str> = l.iter().map(String::as_str).collect();
        return match c.as_slice() {
            ["appdata", "local", "temp"] => named(
                "Temporary files",
                "What programs left behind. Storage Sense (Settings → System → Storage) deletes \
                 the old ones",
            ),
            ["appdata", "local", "packages", p] if wsl_package(p) => named(
                "Linux (WSL)",
                "A WSL distribution's disk: it grows as Linux writes, and doesn't shrink by \
                 itself",
            ),
            ["appdata", "local", "wsl"] => named(
                "Linux (WSL)",
                "WSL's distributions' disks: they grow as Linux writes, and don't shrink by \
                 themselves",
            ),
            ["appdata", "local", "docker"] => named(
                "Docker",
                "Docker Desktop's disk: your images, containers, volumes and build cache",
            ),
            ["appdata", "local", "microsoft", "outlook"] => named(
                "Outlook",
                "Outlook's copy of your mailboxes (.ost). Your mail stays on the server: \
                 Outlook downloads it again",
            ),
            ["documents", "outlook files"] => named(
                "Outlook data files",
                "Your .pst files: mail that may be kept only here. Never deleted",
            ),
            ["appdata", "local", "microsoft", "windows", "inetcache"] => {
                named("Internet cache", "What Windows' web views downloaded")
            }
            ["appdata", "local", "microsoft", "windows", "explorer"] => named(
                "Thumbnails",
                "File Explorer's thumbnails and icons: it makes them again",
            ),
            ["appdata", "local", "crashdumps"] => named(
                "Crash dumps",
                "What programs that crashed left, for their developers",
            ),
            ["appdata", "local", "android"] => named(
                "Android SDK",
                "Android Studio's tools, platforms and emulator images",
            ),
            ["appdata", "roaming", "apple computer", "mobilesync"] | ["apple", "mobilesync"] => {
                named(
                    "iPhone and iPad backups",
                    "Backups made by iTunes or Apple Devices: the only copy unless iCloud has one",
                )
            }
            _ => None,
        };
    }
    let l = lower(abs);
    let c: Vec<&str> = l.iter().map(String::as_str).collect();
    match c.as_slice() {
        ["windows", "winsxs"] => named(
            "Component store",
            "Windows' components, and the versions updates replaced. Much of it is the same \
             files as in C:\\Windows (hard links), so it takes less than it seems. \
             `Dism /Online /Cleanup-Image /StartComponentCleanup`, run as administrator, removes \
             what's superseded",
        ),
        ["windows", "installer"] => named(
            "Installer cache",
            "What installed programs need to repair, update or uninstall themselves: never \
             delete from it by hand",
        ),
        ["windows", "softwaredistribution"] => named(
            "Windows Update",
            "Updates downloaded or installed. Settings → System → Storage → Temporary files \
             cleans them",
        ),
        ["windows", "temp"] => named(
            "Windows' temporary files",
            "Settings → System → Storage → Temporary files cleans them",
        ),
        ["windows", "system32"] => named("System32", "Windows itself"),
        ["windows.old"] => named(
            "Previous Windows installation",
            "What going back to it needs. Windows deletes it after 10 days; Settings → System → \
             Storage → Temporary files → Previous Windows installation(s) deletes it now",
        ),
        ["$windows.~bt" | "$windows.~ws" | "$getcurrent" | "$winreagent"] => named(
            "Windows upgrade files",
            "Left by an upgrade of Windows. Settings → System → Storage → Temporary files \
             removes them",
        ),
        ["system volume information"] => named(
            "System Volume Information",
            "Restore points and the indexes Windows keeps of the drive: only Windows can read it",
        ),
        ["programdata", "microsoft", "windows defender"] => named(
            "Microsoft Defender",
            "Its virus definitions and what it quarantined",
        ),
        ["programdata", "package cache"] => named(
            "Installer packages",
            "Kept by Visual Studio and other installers to repair or remove what they \
             installed",
        ),
        ["program files", "windowsapps"] => named(
            "Microsoft Store apps",
            "Apps from the Microsoft Store: Settings → Apps → Installed apps removes them",
        ),
        ["$recycle.bin", _] => Some(("Recycle Bin".into(), "Freed when you empty the Recycle Bin")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_own_files_on_windows_are_in_your_folders_and_nothing_else() {
        let names = |p: &str| -> Vec<String> { p.split('/').map(str::to_lowercase).collect() };
        for p in [
            "Videos/trip.mp4",
            "Documents/2019/backup.zip",
            "Downloads/setup.exe",
            "Downloads/Win11.iso",
            "Desktop/old.mkv",
            "Pictures/Camera Roll/clip.mov",
            "source/repos/app/data/model.bin",
            "Music/iTunes/iTunes Media/a.m4a",
            "big.iso",
            // A sync service's name, but not its folder: one of yours.
            "Documents/Dropbox export/x.zip",
            "Syncing/x.zip",
            "Boxes/x.zip",
        ] {
            assert!(own_file(&names(p)), "{p}");
        }
        for p in [
            "AppData/Local/Temp/x.bin",
            "AppData/Roaming/Slack/x.bin",
            "OneDrive/Videos/trip.mp4",
            "OneDrive - Contoso/Documents/big.zip",
            "Dropbox/x.zip",
            "Dropbox (Personal)/x.zip",
            "Dropbox (Contoso)/x.zip",
            "iCloudDrive/x.mov",
            "iCloud Photos/x.mov",
            "Pictures/iCloud Photos/Photos/clip.mov",
            "My Drive/x.zip",
            "Google Drive/x.zip",
            "Box/x.zip",
            "Nextcloud/x.zip",
            "MEGA/x.zip",
            "Creative Cloud Files/x.psd",
            "Creative Cloud Files Personal Account/x.psd",
            "Proton Drive/x.zip",
            "SynologyDrive/x.zip",
            "Sync/x.zip",
            "Documents/Virtual Machines/Win/Win.vmdk",
            "Documents/Virtual Machines/Win/notes.zip",
            "VirtualBox VMs/Ubuntu/Ubuntu.vdi",
            "wsl/ubuntu/ext4.vhdx",
            "Documents/Outlook Files/archive.pst",
            "Documents/old-mail.PST",
            "Apple/MobileSync/Backup/abc/x",
            "source/repos/app/bin/Release/app.zip",
            "scoop/apps/x/x.zip",
            "go/pkg/mod/cache/x.zip",
            "tools/portable/app.exe",
            "Desktop/setup.EXE",
            "drivers/x.sys",
        ] {
            assert!(!own_file(&names(p)), "{p}");
        }
        assert!(!own_file(&[]));
    }

    fn p(s: &str) -> Vec<String> {
        s.split('\\')
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect()
    }

    fn home(s: &str) -> Option<(Kind, bool, What)> {
        in_home(&p(s)).map(|c| (c.kind, c.item, c.what))
    }

    fn system(s: &str) -> Option<(Kind, bool, What)> {
        in_system(&p(s)).map(|c| (c.kind, c.item, c.what))
    }

    #[test]
    fn a_profiles_folders_are_sorted_as_windows_names_them() {
        use Kind::*;
        let it = |k, w| Some((k, true, w));
        assert_eq!(home(""), Some((Files, false, What::Plain)));
        assert_eq!(home(r"AppData\Local\Temp"), it(Caches, What::Plain));
        // Case is Windows' to ignore.
        assert_eq!(home(r"appdata\LOCAL\temp"), it(Caches, What::Plain));
        assert_eq!(
            home(r"AppData\Local\Google\Chrome\User Data\Default\Cache"),
            it(Caches, What::Plain)
        );
        assert_eq!(
            home(r"AppData\Local\Microsoft\Edge\User Data\Profile 1\Code Cache"),
            it(Caches, What::Plain)
        );
        assert_eq!(
            home(r"AppData\Roaming\Code\CachedData"),
            it(Caches, What::Plain)
        );
        assert_eq!(home(r"AppData\Roaming\Code"), it(Appdata, What::Plain));
        assert_eq!(
            home(r"AppData\Local\Programs\Microsoft VS Code"),
            it(Apps, What::Plain)
        );
        assert_eq!(
            home(r"AppData\Local\Packages\CanonicalGroupLimited.Ubuntu_79rhkp1fndgsc"),
            it(Vms, What::Vm)
        );
        assert_eq!(
            home(r"AppData\Local\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe"),
            it(Appdata, What::Plain)
        );
        assert_eq!(home(r"AppData\Local\Docker"), it(Dev, What::Plain));
        assert_eq!(
            home(r"AppData\Local\Microsoft\Outlook"),
            it(Mail, What::Plain)
        );
        assert_eq!(home(r"Documents\Outlook Files"), it(Mail, What::Plain));
        assert_eq!(home("OneDrive - Contoso"), it(Cloud, What::Plain));
        assert_eq!(home("Dropbox (Personal)"), it(Cloud, What::Plain));
        assert_eq!(home("My Drive"), it(Cloud, What::Plain));
        assert_eq!(home(r"Pictures\iCloud Photos"), it(Cloud, What::Plain));
        assert_eq!(home(r"Pictures\Camera Roll"), it(Media, What::Plain));
        assert_eq!(home("Syncthing"), it(Files, What::Plain));
        assert_eq!(home("Videos"), Some((Media, false, What::Plain)));
        assert_eq!(home(r"Videos\Captures"), it(Media, What::Plain));
        assert_eq!(home(r"VirtualBox VMs\dev"), it(Vms, What::Vm));
        assert_eq!(home(r".cache\huggingface"), it(Caches, What::Cache));
        assert_eq!(home(".nuget"), it(Dev, What::Plain));
        assert_eq!(home(".cargo"), it(Dev, What::Plain));
        assert_eq!(home(r"source\repos\app\target"), it(Build, What::Artifact));
        // .NET's by name; `space::classify` checks the project file beside them.
        assert_eq!(
            home(r"source\repos\Shop\Shop.Web\bin"),
            it(Build, What::Artifact)
        );
        assert_eq!(home(r"devel\api\OBJ"), it(Build, What::Artifact));
        assert_eq!(home(r"devel\web\node_modules"), it(Build, What::Artifact));
        // Build output only in a code folder.
        assert_eq!(home(r"Downloads\app\target"), None);
        assert_eq!(home("Downloads"), it(Files, What::Plain));
    }

    #[test]
    fn the_drives_folders_are_sorted_as_windows_names_them() {
        use Kind::*;
        let it = |k, w| Some((k, true, w));
        assert_eq!(system(""), Some((Macos, false, What::Plain)));
        assert_eq!(system(r"Windows\WinSxS"), it(Macos, What::Plain));
        assert_eq!(system(r"WINDOWS\Temp"), it(Caches, What::Plain));
        assert_eq!(system("Windows.old"), it(Macos, What::Plain));
        assert_eq!(system(r"Program Files\Adobe"), it(Apps, What::Plain));
        assert_eq!(system(r"Program Files (x86)\Steam"), it(Apps, What::Plain));
        assert_eq!(
            system(r"Program Files\WindowsApps\Microsoft.Office_16"),
            it(Apps, What::Plain)
        );
        assert_eq!(
            system(r"ProgramData\Microsoft\Windows Defender"),
            it(Macos, What::Plain)
        );
        assert_eq!(
            system(r"ProgramData\Microsoft\Windows\Virtual Hard Disks"),
            it(Vms, What::Vm)
        );
        assert_eq!(system(r"ProgramData\chocolatey"), it(Appdata, What::Plain));
        assert_eq!(
            system(r"$Recycle.Bin\S-1-5-21-1-2-3-1001"),
            it(Files, What::Trash)
        );
        assert_eq!(system(r"Users\other"), it(Other, What::Plain));
        assert_eq!(system(r"Users\Public"), it(Files, What::Plain));
        assert_eq!(system("Intel"), it(Other, What::Plain));
    }

    #[test]
    fn windows_own_folders_say_what_they_are() {
        let named = |rel: Option<&str>, abs: &str| {
            let rel = rel.map(p);
            known(rel.as_deref(), &p(abs)).map(|(n, _)| n)
        };
        assert_eq!(
            named(None, r"Windows\WinSxS").as_deref(),
            Some("Component store")
        );
        assert_eq!(
            named(None, r"windows\installer").as_deref(),
            Some("Installer cache")
        );
        assert_eq!(
            named(None, r"$Recycle.Bin\S-1-5-21-1").as_deref(),
            Some("Recycle Bin")
        );
        assert_eq!(
            named(Some(r"AppData\Local\Temp"), r"Users\me\AppData\Local\Temp").as_deref(),
            Some("Temporary files")
        );
        assert_eq!(
            named(Some(r"Documents\notes"), r"Users\me\Documents\notes"),
            None
        );
        for (file, name, what) in SYSTEM_FILES {
            assert!(file.ends_with(".sys") && !name.is_empty() && !what.is_empty());
        }
    }

    /// What Windows cleans itself says where, in these words: the window puts
    /// a button to Settings → System → Storage beside each folder whose note
    /// has them (`storageSettings` in app-windows/ui/js/pages/storage.js).
    #[test]
    fn what_windows_cleans_itself_points_to_its_storage_settings() {
        const PHRASE: &str = "Settings → System → Storage";
        let note = |rel: Option<&str>, abs: &str| {
            let rel = rel.map(p);
            known(rel.as_deref(), &p(abs)).map_or("", |(_, what)| what)
        };
        for (rel, abs) in [
            (Some(r"AppData\Local\Temp"), r"Users\me\AppData\Local\Temp"),
            (None, r"Windows\Temp"),
            (None, r"Windows\SoftwareDistribution"),
            (None, r"Windows.old"),
            (None, r"$Windows.~BT"),
        ] {
            assert!(note(rel, abs).contains(PHRASE), "{abs}: {}", note(rel, abs));
        }
        // Not where Settings doesn't clean.
        for abs in [
            r"Windows\Installer",
            r"Windows\WinSxS",
            r"$Recycle.Bin\S-1-5-21-1",
        ] {
            assert!(!note(None, abs).contains(PHRASE), "{abs}");
        }
    }
}

/// A whole report of a made-up Windows drive: what only Windows sorts this
/// way, so it runs there.
#[cfg(all(test, windows))]
mod report_tests {
    use crate::keep::Keep;
    use crate::space::{BigFile, Dir, Measured, VERSION, VolumeSpace, report, strata};

    const GB: u64 = 1_000_000_000;

    fn d(name: &str, own: u64, children: Vec<Dir>) -> Dir {
        let allocated = own + children.iter().map(|c| c.allocated).sum::<u64>();
        Dir {
            name: name.into(),
            allocated,
            private: allocated,
            children,
            ..Default::default()
        }
    }

    #[test]
    fn a_windows_drive_is_sorted_into_kinds_that_add_up() {
        let tree = d(
            r"C:\",
            40 * GB,
            vec![
                d(
                    "Windows",
                    5 * GB,
                    vec![d("WinSxS", 8 * GB, vec![]), d("System32", 6 * GB, vec![])],
                ),
                d("Program Files", 0, vec![d("Adobe", 10 * GB, vec![])]),
                d(
                    "$Recycle.Bin",
                    0,
                    vec![d("S-1-5-21-1-2-3-1001", 3 * GB, vec![])],
                ),
                d(
                    "Users",
                    0,
                    vec![d(
                        "me",
                        GB,
                        vec![
                            d(
                                "AppData",
                                0,
                                vec![d(
                                    "Local",
                                    0,
                                    vec![
                                        d("Temp", 5 * GB, vec![]),
                                        d(
                                            "Packages",
                                            0,
                                            vec![d(
                                                "CanonicalGroupLimited.Ubuntu_79rhkp1fndgsc",
                                                10 * GB,
                                                vec![],
                                            )],
                                        ),
                                    ],
                                )],
                            ),
                            d("Videos", 15 * GB, vec![]),
                            d("OneDrive", 12 * GB, vec![]),
                            d(
                                "source",
                                0,
                                vec![d(
                                    "repos",
                                    0,
                                    vec![d("app", 5 * GB, vec![d("target", 20 * GB, vec![])])],
                                )],
                            ),
                        ],
                    )],
                ),
            ],
        );
        let unseen = 10 * GB;
        let used = tree.allocated + unseen;
        let file = |name: &str, bytes: u64| BigFile {
            path: format!(r"C:\{name}"),
            allocated: bytes,
            private: 0,
            accessed: 0,
            modified: 0,
        };
        let m = Measured {
            version: VERSION,
            measured: 1_800_000_000,
            took_secs: 1.0,
            home: r"C:\Users\me".into(),
            volume: VolumeSpace {
                total: used + 100 * GB,
                used,
                free: 100 * GB,
                purgeable: 0,
            },
            volumes: Vec::new(),
            data_in_use: used,
            data_real: tree.allocated,
            snapshots: 0,
            unreadable_dirs: 3,
            tree,
            apps: Vec::new(),
            runtimes: Vec::new(),
            files: Vec::new(),
            simulators: Vec::new(),
            avds: None,
            system_files: vec![file("pagefile.sys", 32 * GB), file("hiberfil.sys", 6 * GB)],
            quota_limited: false,
        };
        let r = report(&m, &Keep::default(), &[]);
        let kind = |id: &str| r.kinds.iter().find(|k| k.id == id).unwrap();
        let names =
            |id: &str| -> Vec<String> { kind(id).items.iter().map(|i| i.name.clone()).collect() };
        assert_eq!(kind("macos").title, "Windows");
        assert!(
            names("macos").contains(&"Page file".to_string()),
            "{:?}",
            names("macos")
        );
        assert!(names("macos").contains(&"Hibernation file".to_string()));
        assert!(names("macos").contains(&"Component store".to_string()));
        assert_eq!(names("build"), [r"repos\app\target"]);
        assert_eq!(names("vms"), ["Linux (WSL)"]);
        assert_eq!(names("caches"), ["Temporary files"]);
        assert!(names("files").contains(&"Recycle Bin".to_string()));
        assert!(names("other").contains(&"System data".to_string()));
        // Every byte once: the kinds are what's in use.
        assert_eq!(r.kinds.iter().map(|k| k.bytes).sum::<u64>(), used);
        // Nothing is offered to clean yet, and nothing is called old.
        assert!(
            r.kinds
                .iter()
                .flat_map(|k| &k.items)
                .all(|i| i.action.is_none())
        );
        assert!(r.old_files.is_empty());
        // An app's folder found in the tree from the drive's root: what its
        // handler says comes with it.
        let about = r.kinds.iter().flat_map(|k| &k.items).find_map(|i| {
            i.about
                .as_ref()
                .filter(|a| a.title == "OneDrive's downloads")
        });
        assert_eq!(
            about.map(|a| a.path.as_str()),
            Some(r"C:\Users\me\OneDrive")
        );
        let layers = strata(&r);
        assert!(layers.contains(&("trash", 3 * GB)), "{layers:?}");
        // The bin the scan found too small to keep is measured where it is
        // now: the account's folder in the drive's.
        let sid = crate::os::sys::user_sid().unwrap();
        assert_eq!(
            crate::space::own_recycle_bin(&m),
            Some(std::path::PathBuf::from(r"C:\$Recycle.Bin").join(sid))
        );
        assert!(layers.contains(&("unseen", unseen)), "{layers:?}");
    }
}
