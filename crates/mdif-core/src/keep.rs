//! What you keep: paths MyDiskIsFull never suggests deleting, cleaning or
//! moving, however big or old they get (a VM you need, a cloud folder you
//! want offline, your videos).
//!
//! A text file, one path per line: `~/` for the home folder (`~\` too on
//! Windows), `*` and `?` within a name, `#` for comments. A path keeps everything inside it. Names
//! compare without case, as on the Mac's own volume, so a pattern keeps at
//! least what it names. A cleanup that would delete something kept inside
//! the folder it cleans counts as touching it too: `cargo clean` while you
//! keep `target/release`.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::map::DATA_ROOT;

/// Written at the top of a new file.
#[cfg(not(windows))]
const HEADER: &str = "# Paths MyDiskIsFull never suggests deleting, cleaning or moving.\n\
                      # One per line: ~/ is your home folder, * and ? match within a name.\n";
#[cfg(windows)]
const HEADER: &str = "# Paths MyDiskIsFull never suggests deleting, cleaning or moving.\n\
                      # One per line: ~\\ is your home folder, * and ? match within a name.\n";

/// Where a home-relative entry starts: `~/`, and on Windows `~\` too.
fn home_relative(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('~')?;
    rest.strip_prefix('/')
        .or_else(|| rest.strip_prefix('\\').filter(|_| cfg!(windows)))
}

#[derive(Debug, Clone)]
struct Entry {
    /// As written in the file.
    text: String,
    /// The absolute path's names, lowercased.
    names: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Keep {
    entries: Vec<Entry>,
}

impl Keep {
    /// The list in `file`; empty when there is none.
    pub fn load(file: &Path, home: &Path) -> io::Result<Keep> {
        Ok(Keep::parse(&read(file)?, home))
    }

    pub fn parse(text: &str, home: &Path) -> Keep {
        // An editor may start the file with a byte-order mark, which is no name.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let entries = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| Entry {
                text: l.to_string(),
                names: names(&expand(l, home)),
            })
            .filter(|e| !e.names.is_empty())
            .collect();
        Keep { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, as written.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.text.as_str())
    }

    /// The entry that keeps `path`: `path` itself, or a folder it is in.
    pub fn covering(&self, path: &Path) -> Option<&str> {
        let p = names(path);
        self.entries
            .iter()
            .find(|e| e.names.len() <= p.len() && matches(&e.names, &p))
            .map(|e| e.text.as_str())
    }

    /// An entry for something inside `path`, which deleting `path` would
    /// delete too.
    pub fn inside(&self, path: &Path) -> Option<&str> {
        let p = names(path);
        self.entries
            .iter()
            .find(|e| e.names.len() > p.len() && matches(&e.names, &p))
            .map(|e| e.text.as_str())
    }

    /// Whether deleting `path` would delete something kept.
    pub fn touches(&self, path: &Path) -> bool {
        self.covering(path).is_some() || self.inside(path).is_some()
    }
}

/// `file` with `path` added as an entry, unless one says the same already;
/// `false` when it did. A new file starts with a comment saying what it is.
pub fn add(file: &Path, home: &Path, path: &str) -> io::Result<bool> {
    let entry = entry(path, home)?;
    let text = read(file)?;
    let wanted = names(&expand(&entry, home));
    if Keep::parse(&text, home)
        .entries
        .iter()
        .any(|e| e.names == wanted)
    {
        return Ok(false);
    }
    let mut out = if text.is_empty() {
        HEADER.to_string()
    } else {
        text
    };
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&entry);
    out.push('\n');
    write(file, &out)?;
    Ok(true)
}

/// `file` without the entries that say the same as `path` (comments and the
/// other entries as they were); `false` when there was none.
pub fn remove(file: &Path, home: &Path, path: &str) -> io::Result<bool> {
    let wanted = names(&expand(&entry(path, home)?, home));
    let text = read(file)?;
    let mut removed = false;
    let mut out = String::new();
    for line in text.lines() {
        let l = line.trim();
        if !l.is_empty() && !l.starts_with('#') && names(&expand(l, home)) == wanted {
            removed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if removed {
        write(file, &out)?;
    }
    Ok(removed)
}

/// `path` as an entry: absolute, `~/` for the home folder. A relative path is
/// taken from the current directory.
pub fn entry(path: &str, home: &Path) -> io::Result<String> {
    let path = path.trim();
    if path.is_empty() || path.contains(['\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a path to keep is one line, and not empty",
        ));
    }
    let abs = if path == "~" || home_relative(path).is_some() {
        expand(path, home)
    } else {
        std::path::absolute(path)?
    };
    // `../game/build` from `~/devel/app` is `~/devel/game/build`.
    let abs = resolved(&abs);
    let abs = abs
        .strip_prefix(DATA_ROOT)
        .map_or(abs.clone(), |rest| Path::new("/").join(rest));
    // Its names joined by this system's separator, however they were typed.
    Ok(crate::text::tilde(&abs, home).unwrap_or_else(|| abs.to_string_lossy().into_owned()))
}

fn read(file: &Path) -> io::Result<String> {
    match std::fs::read_to_string(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Ok(text) => Ok(match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_string(),
            None => text,
        }),
        r => r,
    }
}

/// Written whole, then moved into place, so a reader never sees half of it.
fn write(file: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Its own name, so two writers never write into one file.
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

/// How many files or folders `entry` names now, when that can be told: a
/// pattern only in its last name, and nothing macOS would prompt for.
pub fn present(entry: &str, home: &Path) -> Option<usize> {
    // Listing a folder a cloud provider keeps online would download it (macOS).
    crate::os::fs::forbid_dataless_materialization().ok()?;
    let path = expand(entry, home);
    let wild_name = |n: &OsStr| n.to_string_lossy().contains(['*', '?']);
    let parent = path.parent()?;
    if parent.iter().any(wild_name) || !crate::privacy::may_open(parent) {
        return None;
    }
    let name = path.file_name()?;
    if !wild_name(name) {
        return Some(std::fs::symlink_metadata(&path).is_ok() as usize);
    }
    let pattern = name.to_string_lossy().to_lowercase();
    // Windows: a folder a cloud provider keeps online-only would be downloaded
    // by an ordinary listing, so the names come through the scanner's checked
    // open, which refuses it (unknown, not none).
    #[cfg(windows)]
    let names = match crate::os::bulk::names(parent) {
        Ok(names) => names,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(0),
        Err(_) => return None,
    };
    #[cfg(windows)]
    {
        Some(
            names
                .iter()
                .filter(|n| wild(&pattern, &n.to_string_lossy().to_lowercase()))
                .count(),
        )
    }
    #[cfg(not(windows))]
    {
        match std::fs::read_dir(parent) {
            Ok(rd) => Some(
                rd.flatten()
                    .filter(|e| wild(&pattern, &e.file_name().to_string_lossy().to_lowercase()))
                    .count(),
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(0),
            Err(_) => None,
        }
    }
}

/// The path an entry names, `~/` expanded.
pub fn expand(entry: &str, home: &Path) -> PathBuf {
    if entry == "~" {
        return home.to_path_buf();
    }
    match home_relative(entry) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(entry),
    }
}

/// `path` with each `..` taking off the name before it, by name: what the
/// entry was meant to name, and what the paths it's compared with spell.
pub(crate) fn resolved(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// The names in `path`, lowercased, as seen from `/` (the Data volume's own
/// mount point left out), each `..` taking off the name before it. On
/// Windows the drive or share comes first, as a name: `D:\Archive` isn't
/// `C:\Archive`. Each name decomposed ([`crate::os::fs::decomposed`]), as
/// a Mac's disk compares them: `Vidéos` typed by hand is Finder's.
fn names(path: &Path) -> Vec<String> {
    use std::path::Prefix::*;
    let path = path.strip_prefix(DATA_ROOT).unwrap_or(path);
    let mut out = Vec::new();
    // `..` never goes above the drive or share.
    let mut floor = 0;
    for c in path.components() {
        match c {
            Component::Normal(n) => {
                out.push(crate::os::fs::decomposed(&n.to_string_lossy()).to_lowercase())
            }
            // `a/../b` names what `b` does: a kept `../target` keeps the target
            // folder, and `mdif clean src/../target` is checked as `target`.
            Component::ParentDir => {
                if out.len() > floor {
                    out.pop();
                }
            }
            Component::Prefix(p) => {
                out.push(match p.kind() {
                    Disk(d) | VerbatimDisk(d) => format!("{}:", (d as char).to_ascii_lowercase()),
                    UNC(server, share) | VerbatimUNC(server, share) => format!(
                        r"\\{}\{}",
                        server.to_string_lossy().to_lowercase(),
                        share.to_string_lossy().to_lowercase()
                    ),
                    _ => p.as_os_str().to_string_lossy().to_lowercase(),
                });
                floor = out.len();
            }
            Component::RootDir | Component::CurDir => {}
        }
    }
    out
}

/// Whether the shorter of `pattern` and `path` matches the start of the
/// other, name by name.
fn matches(pattern: &[String], path: &[String]) -> bool {
    pattern.iter().zip(path).all(|(p, n)| wild(p, n))
}

/// `*` (any run of characters) and `?` (one) within a name.
fn wild(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut i, mut j) = (0, 0);
    // Where the last `*` was, and where in `name` it started matching.
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn keep(text: &str) -> Keep {
        Keep::parse(text, Path::new(HOME))
    }

    // A Mac's disk takes both forms for one name; NTFS keeps them apart.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_name_typed_composed_keeps_the_one_finder_wrote_decomposed() {
        let k = keep("~/Vid\u{e9}os\n");
        assert_eq!(
            k.covering(Path::new("/Users/me/Vide\u{301}os/clip.mov")),
            Some("~/Vid\u{e9}os")
        );
        let k = keep("~/Vide\u{301}os\n");
        assert!(k.covering(Path::new("/Users/me/VID\u{c9}OS")).is_some());
        assert!(k.covering(Path::new("/Users/me/Videos")).is_none());
    }

    #[test]
    fn a_kept_path_keeps_what_is_inside_it_without_case() {
        let k = keep(
            "# mine\n\n~/Virtual Machines.localized/Windows 11.vmwarevm\n  \
             ~/Movies/Clip*  \n/Volumes/Archive\n",
        );
        assert_eq!(k.entries().count(), 3);
        let vm = "~/Virtual Machines.localized/Windows 11.vmwarevm";
        assert_eq!(
            k.covering(Path::new(
                "/Users/me/Virtual Machines.localized/Windows 11.vmwarevm/disk.vmdk"
            )),
            Some(vm)
        );
        // The Data volume's own path names the same file.
        assert_eq!(
            k.covering(Path::new(
                "/System/Volumes/Data/Users/me/virtual machines.localized/WINDOWS 11.vmwarevm"
            )),
            Some(vm)
        );
        assert!(
            k.covering(Path::new("/Users/me/Movies/clip_002.mp4"))
                .is_some()
        );
        assert!(
            k.covering(Path::new("/Users/me/Movies/Concert.mp4"))
                .is_none()
        );
        assert!(
            k.covering(Path::new("/Users/me/Virtual Machines.localized"))
                .is_none()
        );
        assert!(k.covering(Path::new("/Volumes/Archive2")).is_none());
    }

    // A Mac's paths; on Windows a path from the root is on a drive.
    #[cfg(not(windows))]
    #[test]
    fn an_entry_with_dot_dot_keeps_what_it_names() {
        let home = Path::new(HOME);
        // Given from `~/devel/app` as `../game/build`.
        assert_eq!(
            entry("/Users/me/devel/app/../game/./build", home).unwrap(),
            "~/devel/game/build"
        );
        // One saved with it before still keeps that, not another.
        let k = keep("~/devel/app/../game/build\n");
        let build = Path::new("/Users/me/devel/game/build");
        assert!(k.covering(build).is_some());
        assert!(k.touches(Path::new("/Users/me/devel/game")));
        assert!(
            k.covering(Path::new("/Users/me/devel/app/game/build"))
                .is_none()
        );
    }

    #[test]
    fn deleting_a_folder_touches_what_is_kept_inside_it() {
        let k = keep("~/devel/app/target/release\n~/Movies/Clip*\n");
        let target = Path::new("/Users/me/devel/app/target");
        assert_eq!(k.inside(target), Some("~/devel/app/target/release"));
        assert!(k.touches(target));
        assert!(k.touches(Path::new("/Users/me/Movies")));
        assert!(!k.touches(Path::new("/Users/me/devel/app/target/debug")));
        assert!(!k.touches(Path::new("/Users/me/devel/other")));
    }

    #[test]
    fn present_counts_what_an_entry_names_now() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir(home.join("Storage")).unwrap();
        for f in ["Clip_001.mp4", "clip_002.mp4", "film.mkv"] {
            std::fs::write(home.join("Storage").join(f), "").unwrap();
        }
        assert_eq!(present("~/Storage/Clip*", home), Some(2));
        assert_eq!(present("~/Storage/film.mkv", home), Some(1));
        assert_eq!(present("~/Storage/gone.mkv", home), Some(0));
        assert_eq!(present("~/Gone/*", home), Some(0));
        assert_eq!(present("~/*/film.mkv", home), None);
    }

    #[test]
    fn wildcards_match_within_a_name() {
        assert!(wild("clip*", "clip_001.mp4"));
        assert!(wild("*.mp4", "clip_001.mp4"));
        assert!(wild("c?ip_*1.mp4", "clip_001.mp4"));
        assert!(wild("*", ""));
        assert!(wild("a*b*c", "axxbyyc"));
        assert!(!wild("a*b*c", "axxbyy"));
        assert!(!wild("clip", "clip_001.mp4"));
        assert!(!wild("?", ""));
    }

    // The same in a home that is a real folder, as it is on any OS.
    #[test]
    fn add_and_remove_keep_the_rest_of_the_file_in_a_real_home() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("MyDiskIsFull/keep.txt");
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let vm = home.join("Virtual Machines.localized");
        assert!(add(&file, &home, vm.to_str().unwrap()).unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        // `~` and this system's separator (`~\` on Windows).
        let sep = std::path::MAIN_SEPARATOR;
        assert!(
            text.ends_with(&format!("\n~{sep}Virtual Machines.localized\n")),
            "{text}"
        );
        // The same path, however it's written, is added once.
        assert!(!add(&file, &home, "~/virtual machines.localized").unwrap());
        assert!(add(&file, &home, "~/Movies/Clip*").unwrap());
        assert!(remove(&file, &home, "~/Virtual Machines.localized").unwrap());
        assert!(!remove(&file, &home, "~/Virtual Machines.localized").unwrap());
        let k = Keep::load(&file, &home).unwrap();
        assert_eq!(
            k.entries().collect::<Vec<_>>(),
            [format!("~{sep}Movies{sep}Clip*")]
        );
    }

    // `..` names what it leads to, never above the root; Notepad starts a
    // file it saves with a byte-order mark.
    #[test]
    fn parent_names_and_a_byte_order_mark_are_read_as_meant() {
        let k = keep("\u{feff}~/devel/app/../site/dist\n");
        assert_eq!(k.entries().count(), 1);
        let kept = |p: &str| k.covering(Path::new(p)).is_some();
        assert!(kept("/Users/me/devel/site/dist/index.html"));
        assert!(!kept("/Users/me/devel/app/dist"));
        // A path to clean is checked as what it names.
        assert!(kept("/Users/me/devel/x/../site/dist"));
        assert!(k.touches(Path::new("/Users/me/devel/site/x/..")));
        assert_eq!(
            names(Path::new("/../../Users/me")),
            names(Path::new("/Users/me"))
        );
    }

    // The home folder as `~\` or `~/`, either way.
    #[cfg(windows)]
    #[test]
    fn a_home_path_may_be_written_with_either_slash() {
        let home = Path::new(r"C:\Users\me");
        let k = Keep::parse("~\\Videos\\Reels\n~/Music\n", home);
        assert!(
            k.covering(Path::new(r"C:\Users\me\Videos\Reels\a.mp4"))
                .is_some()
        );
        assert!(k.covering(Path::new(r"c:\users\me\music")).is_some());
        assert_eq!(expand(r"~\Videos", home), home.join("Videos"));
        assert_eq!(entry("~/Videos/Reels", home).unwrap(), r"~\Videos\Reels");
    }

    // A drive is part of the name: the same folder on two drives is two folders.
    #[cfg(windows)]
    #[test]
    fn the_same_folder_on_two_drives_is_two_folders() {
        assert_ne!(
            names(Path::new(r"C:\Archive")),
            names(Path::new(r"D:\Archive"))
        );
        assert_eq!(
            names(Path::new(r"c:\archive")),
            names(Path::new(r"\\?\C:\Archive"))
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("MyDiskIsFull/keep.txt");
        let home = dir.path().join("home");
        assert!(add(&file, &home, r"C:\Archive").unwrap());
        assert!(add(&file, &home, r"D:\Archive").unwrap());
        assert!(remove(&file, &home, r"D:\Archive").unwrap());
        let k = Keep::load(&file, &home).unwrap();
        assert_eq!(k.entries().collect::<Vec<_>>(), [r"C:\Archive"]);
    }

    // A Mac's home, and its firmlinked alias.
    #[cfg(target_os = "macos")]
    #[test]
    fn add_and_remove_keep_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("MyDiskIsFull/keep.txt");
        let home = Path::new(HOME);
        assert!(add(&file, home, "/Users/me/Virtual Machines.localized").unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.starts_with("# Paths MyDiskIsFull never suggests"),
            "{text}"
        );
        assert!(text.ends_with("\n~/Virtual Machines.localized\n"), "{text}");
        // The same path, however it's written, is added once.
        assert!(!add(&file, home, "~/virtual machines.localized").unwrap());
        assert!(
            !add(
                &file,
                home,
                "/System/Volumes/Data/Users/me/Virtual Machines.localized"
            )
            .unwrap()
        );
        assert!(add(&file, home, "~/Movies/Clip*").unwrap());
        std::fs::write(
            &file,
            std::fs::read_to_string(&file).unwrap() + "# my note\n",
        )
        .unwrap();
        assert!(remove(&file, home, "~/Virtual Machines.localized").unwrap());
        assert!(!remove(&file, home, "~/Virtual Machines.localized").unwrap());
        let k = Keep::load(&file, home).unwrap();
        assert_eq!(k.entries().collect::<Vec<_>>(), ["~/Movies/Clip*"]);
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("# my note\n")
        );
        assert!(add(&file, home, "bad\npath").is_err());
        assert!(
            Keep::load(&dir.path().join("none"), home)
                .unwrap()
                .is_empty()
        );
    }
}
