//! What you keep: paths MyDiskIsFull never suggests deleting, cleaning or
//! moving, however big or old they get (a VM you need, a cloud folder you
//! want offline, your videos).
//!
//! A text file, one path per line: `~/` for the home folder, `*` and `?`
//! within a name, `#` for comments. A path keeps everything inside it. Names
//! compare without case, as on the Mac's own volume, so a pattern keeps at
//! least what it names. A cleanup that would delete something kept inside
//! the folder it cleans counts as touching it too: `cargo clean` while you
//! keep `target/release`.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::map::DATA_ROOT;

/// Written at the top of a new file.
const HEADER: &str = "# Paths MyDiskIsFull never suggests deleting, cleaning or moving.\n\
                      # One per line: ~/ is your home folder, * and ? match within a name.\n";

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
        match std::fs::read_to_string(file) {
            Ok(text) => Ok(Keep::parse(&text, home)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Keep::default()),
            Err(e) => Err(e),
        }
    }

    pub fn parse(text: &str, home: &Path) -> Keep {
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
    let abs = if path == "~" || path.starts_with("~/") {
        expand(path, home)
    } else {
        std::path::absolute(path)?
    };
    let abs = abs
        .strip_prefix(DATA_ROOT)
        .map_or(abs.clone(), |rest| Path::new("/").join(rest));
    Ok(match abs.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => abs.to_string_lossy().into_owned(),
    })
}

fn read(file: &Path) -> io::Result<String> {
    match std::fs::read_to_string(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        r => r,
    }
}

/// Written whole, then moved into place, so a reader never sees half of it.
fn write(file: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

/// How many files or folders `entry` names now, when that can be told: a
/// pattern only in its last name, and nothing macOS would prompt for.
pub fn present(entry: &str, home: &Path) -> Option<usize> {
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
    let rd = match std::fs::read_dir(parent) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(0),
        Err(_) => return None,
    };
    Some(
        rd.flatten()
            .filter(|e| wild(&pattern, &e.file_name().to_string_lossy().to_lowercase()))
            .count(),
    )
}

/// The path an entry names, `~/` expanded.
pub fn expand(entry: &str, home: &Path) -> PathBuf {
    match entry.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(entry),
    }
}

/// The names in `path`, lowercased, as seen from `/` (the Data volume's own
/// mount point left out).
fn names(path: &Path) -> Vec<String> {
    let path = path.strip_prefix(DATA_ROOT).unwrap_or(path);
    path.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
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
