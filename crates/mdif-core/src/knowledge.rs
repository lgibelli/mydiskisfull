//! What's known about folders, as data: what one is called, the line under
//! it in a list, what's in it, why it grows and what helps. A copy is built
//! in (`knowledge/folders.json`); `mdif knowledge update` downloads a newer
//! one when there is one, signed with the key the app's updates are signed
//! with. It's text only: a newer one changes what's said about a folder,
//! never what's done with it. What MyDiskIsFull may delete, empty or move
//! stays in the code ([`clean`](crate::clean), [`handlers`](crate::handlers)).
//!
//! An entry is found by its place (`path`: from the home folder, or from
//! `/`; `*` stands for any run of characters in a name) or by its app
//! (`app`: an id like `com.apple.mediaanalysisd`, for its container, group
//! container, Application Support and Caches folders). Where several
//! match, the first that says a thing says it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The copy built in.
const BUNDLED: &str = include_str!("../knowledge/folders.json");
/// Where newer copies are, in this format; the signature is beside it,
/// with `.sig` after the name.
pub const URL: &str = "https://www.salamacchine.it/apps/mydiskisfull/folders-1.json";
/// The updates' Ed25519 key: `SUPublicEDKey` in the app's Info.plist.
const KEY: &str = "Ke1DpxYBAViRiRdrOAbohaCV85YCaf9HMbW7wAPR4no=";
/// A copy bigger than this isn't one.
const MAX: usize = 1_000_000;
/// The downloaded copy and its signature, in the folder `init` is given.
const FILE: &str = "folders.json";
const SIG: &str = "folders.json.sig";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Knowledge {
    /// Higher is newer: only a higher one replaces what's known.
    pub version: u64,
    pub folders: Vec<Entry>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What it's called.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The line under its name in a list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helps: Option<String>,
    /// Where it's said (https).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// What's known about a folder: what's in it, why it grows, what helps.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Known {
    pub what: Option<String>,
    pub grows: Option<String>,
    pub helps: Option<String>,
    pub source: Option<String>,
    /// The AI model that said it, when it's an answer asked from this Mac
    /// ([`crate::ask`]) and not the list's: not checked by anyone.
    pub ai: Option<String>,
}

/// What's known now, and whether it's a downloaded copy.
pub struct Loaded {
    pub knowledge: Knowledge,
    pub downloaded: bool,
}

static LOADED: OnceLock<Loaded> = OnceLock::new();

/// Takes the downloaded copy in `dir` when it's signed and newer than the
/// one built in. Once, before anything asks; without it, the one built in.
pub fn init(dir: &Path) {
    let _ = LOADED.set(load(dir, &key()));
}

pub fn loaded() -> &'static Loaded {
    LOADED.get_or_init(|| Loaded {
        knowledge: bundled(),
        downloaded: false,
    })
}

fn bundled() -> Knowledge {
    parse(BUNDLED.as_bytes()).expect("the copy built in is checked by its test")
}

fn load(dir: &Path, key: &VerifyingKey) -> Loaded {
    let built = bundled();
    let read = || -> Result<Knowledge> {
        let bytes = std::fs::read(dir.join(FILE))?;
        verify(&bytes, &std::fs::read_to_string(dir.join(SIG))?, key)?;
        parse(&bytes)
    };
    match read() {
        Ok(k) if k.version > built.version => Loaded {
            knowledge: k,
            downloaded: true,
        },
        _ => Loaded {
            knowledge: built,
            downloaded: false,
        },
    }
}

fn key() -> VerifyingKey {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(KEY)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .expect("the key is 32 bytes of base64");
    VerifyingKey::from_bytes(&bytes).expect("the key is an Ed25519 key")
}

/// Whether `sig` (base64, as Sparkle's `sign_update -p` prints it) is the
/// key's signature of `bytes`.
fn verify(bytes: &[u8], sig: &str, key: &VerifyingKey) -> Result<()> {
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig.trim())
        .context("the signature isn't base64")?;
    let sig = Signature::from_slice(&sig).context("the signature isn't an Ed25519 one")?;
    key.verify_strict(bytes, &sig)
        .map_err(|_| anyhow!("it isn't signed with the updates' key"))
}

/// A copy, if it's one: every entry has a place or an app, and something
/// to say.
fn parse(bytes: &[u8]) -> Result<Knowledge> {
    if bytes.len() > MAX {
        bail!("it's bigger than {MAX} bytes");
    }
    let k: Knowledge = serde_json::from_slice(bytes).context("it isn't the list's JSON")?;
    for e in &k.folders {
        let at = e.path.as_deref().or(e.app.as_deref()).unwrap_or("");
        if e.path.is_some() == e.app.is_some() {
            bail!("an entry needs a path or an app, not both: {at:?}");
        }
        if at.is_empty() || at.ends_with('/') || at.split('/').any(|n| n == ".." || n == ".") {
            bail!("not a place: {at:?}");
        }
        let texts = [&e.name, &e.note, &e.what, &e.grows, &e.helps];
        if texts.iter().all(|t| t.is_none()) || texts.iter().any(|t| t.as_deref() == Some("")) {
            bail!("nothing said about {at:?}");
        }
        if e.source
            .as_deref()
            .is_some_and(|s| !s.starts_with("https://"))
        {
            bail!("a source that isn't https: {at:?}");
        }
    }
    Ok(k)
}

fn entries() -> &'static [Entry] {
    &loaded().knowledge.folders
}

/// The entries for the folder at `path` by its place.
fn at<'a>(path: &'a Path, home: &'a Path) -> impl Iterator<Item = &'static Entry> + 'a {
    let rel = path.strip_prefix(home).ok();
    let abs = path.strip_prefix("/").ok();
    entries().iter().filter(move |e| match e.path.as_deref() {
        Some(p) => match p.strip_prefix('/') {
            Some(p) => abs.is_some_and(|a| crate::handlers::matches(p, a)),
            None => rel.is_some_and(|r| crate::handlers::matches(p, r)),
        },
        None => false,
    })
}

/// An app's entries, for a folder named after it: with a team ID or
/// `group.` in front (`UBF8T346G9.Office`, `group.com.apple.notes`), or not.
fn app(c: &str) -> impl Iterator<Item = &'static Entry> + '_ {
    let bare = bare(c);
    entries()
        .iter()
        .filter(move |e| e.app.as_deref().is_some_and(|a| a == c || a == bare))
}

/// Without the team ID and `group.`.
fn bare(c: &str) -> &str {
    // A team ID: ten capitals and digits.
    let team = |s: &str| {
        s.len() == 10
            && s.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    let bare = match c.split_once('.') {
        Some((t, rest)) if team(t) => rest,
        _ => c,
    };
    bare.strip_prefix("group.").unwrap_or(bare)
}

/// The app whose folder `path` is in: `Library/Containers/<app>/…`, a
/// group container, Application Support or Caches.
fn app_of<'a>(path: &'a Path, home: &Path) -> Option<&'a str> {
    let mut names = path.strip_prefix(home).ok()?.iter().map(|n| n.to_str());
    let (Some(Some("Library")), Some(Some(kind)), Some(Some(c))) =
        (names.next(), names.next(), names.next())
    else {
        return None;
    };
    matches!(
        kind,
        "Containers" | "Group Containers" | "Application Support" | "Caches"
    )
    .then_some(c)
}

/// What the folder at `path` is called, by its place.
pub fn name(path: &Path, home: &Path) -> Option<String> {
    at(path, home).find_map(|e| e.name.clone())
}

/// The line under it in a list, by its place.
pub fn note(path: &Path, home: &Path) -> Option<String> {
    at(path, home).find_map(|e| e.note.clone())
}

/// What an app's folder (`c`, its container or group container's name) is
/// called: the app's name, as far as it's known; otherwise `c` without the
/// team ID and `group.`.
pub fn app_name(c: &str) -> String {
    app(c)
        .find_map(|e| e.name.clone())
        .unwrap_or_else(|| bare(c).to_string())
}

/// What the folder at `path` is called, by its place or, for an app's own
/// folder, by its app; nothing when it isn't known.
pub fn called(path: &Path, home: &Path) -> Option<String> {
    name(path, home).or_else(|| {
        let c = app_of(path, home)?;
        (path.strip_prefix(home).ok()?.iter().count() == 3)
            .then(|| app(c).find_map(|e| e.name.clone()))
            .flatten()
    })
}

/// What's known about the folder at `path`: by its place, or by the app
/// whose folder it's in.
pub fn known(path: &Path, home: &Path) -> Option<Known> {
    let by_app = app_of(path, home).into_iter().flat_map(app);
    at(path, home)
        .chain(by_app)
        .find(|e| e.what.is_some() || e.grows.is_some() || e.helps.is_some())
        .map(|e| Known {
            what: e.what.clone(),
            grows: e.grows.clone(),
            helps: e.helps.clone(),
            source: e.source.clone(),
            ai: None,
        })
}

/// What the Library's folders by these names hold is yours (a cloud
/// account's folder is named after it, a mailbox after its address): only
/// the folder itself is said.
const YOURS: &[&str] = &["CloudStorage", "Mobile Documents", "Mail", "Messages"];

/// Names apps and tools give their own folders, which say nothing about
/// whoever has them: kept in a folder's name past an app's or a tool's.
const STANDARD: &[&str] = &[
    "Data",
    "Library",
    "Caches",
    "Cache",
    "Application Support",
    "tmp",
    "cache",
    "caches",
    "install",
    "registry",
    "bin",
    "lib",
    "share",
    "pkg",
    "mod",
];

/// A folder's name that says nothing about whoever has it, as it may be
/// sent to ask about it: an app's folder (`~/Library/Containers/
/// com.apple.Safari`, `…/Data/Library/Caches` inside it), another of the
/// Library's (`~/Library/Biome`), a tool's dot-folder (`~/.dartServer`,
/// `~/.bun/install/cache`, `~/.cache/<tool>`), or a place outside the home
/// folder (`/usr/local`). Past what's known to be generic, the name stops:
/// `~/.claude`, not the project named inside it. Nothing for your own
/// folders, other accounts' or other disks'.
pub fn generic(path: &Path, home: &Path) -> Option<String> {
    let name = generic_name(path, home)?;
    (!name.split('/').any(identifies)).then_some(name)
}

/// A name made for one Mac (a UUID, a long number) would tell it apart
/// from others: as worker/src/name.ts says.
fn identifies(name: &str) -> bool {
    let run = |ok: fn(char) -> bool| {
        name.chars()
            .fold((0, 0), |(longest, now), c| {
                let now = if ok(c) { now + 1 } else { 0 };
                (longest.max(now), now)
            })
            .0
    };
    let uuid = name.as_bytes().windows(36).any(|w| {
        w.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    });
    uuid || run(|c| c.is_ascii_hexdigit()) >= 16 || run(|c| c.is_ascii_digit()) >= 6
}

fn generic_name(path: &Path, home: &Path) -> Option<String> {
    let names = |p: &Path| -> Option<Vec<String>> {
        p.iter().map(|n| n.to_str().map(str::to_string)).collect()
    };
    let standard = |rest: &[String]| {
        rest.iter()
            .take_while(|n| STANDARD.contains(&n.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    if let Ok(rel) = path.strip_prefix(home) {
        let n = names(rel)?;
        let keep: Vec<String> = match n.as_slice() {
            [] => return None,
            [lib, kind, app, rest @ ..]
                if lib == "Library"
                    && matches!(
                        kind.as_str(),
                        "Containers" | "Group Containers" | "Application Support" | "Caches"
                    ) =>
            {
                [lib.clone(), kind.clone(), app.clone()]
                    .into_iter()
                    .chain(standard(rest))
                    .collect()
            }
            [lib, kind, ..] if lib == "Library" && YOURS.contains(&kind.as_str()) => {
                (n.len() == 2).then(|| n.clone())?
            }
            [lib, ..] if lib == "Library" => {
                let depth = if n.get(1).is_some_and(|k| k == "Developer") {
                    4
                } else {
                    3
                };
                n.iter().take(depth).cloned().collect()
            }
            [dot, rest @ ..] if dot.starts_with('.') => {
                let tool = usize::from(matches!(dot.as_str(), ".cache" | ".config"))
                    + 2 * usize::from(dot == ".local");
                std::iter::once(dot.clone())
                    .chain(rest.iter().take(tool).cloned())
                    .chain(standard(rest.get(tool..).unwrap_or_default()))
                    .collect()
            }
            _ => return None,
        };
        return Some(format!("~/{}", keep.join("/")));
    }
    let n = names(path.strip_prefix("/").ok()?)?;
    if n.is_empty() || matches!(n[0].as_str(), "Users" | "Volumes") {
        return None;
    }
    Some(format!(
        "/{}",
        n.iter().take(3).cloned().collect::<Vec<_>>().join("/")
    ))
}

/// Checks a copy and its signature, as `mdif knowledge check` does before
/// one is put online.
pub fn check(bytes: &[u8], sig: Option<&str>) -> Result<Knowledge> {
    if let Some(sig) = sig {
        verify(bytes, sig, &key())?;
    }
    parse(bytes)
}

/// What an update found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Updated {
    /// The version known now.
    pub version: u64,
    /// Whether it's a newer one than before.
    pub newer: bool,
    pub folders: usize,
}

/// Downloads the newest copy into `dir`, when it's signed and newer than
/// what's known. Sends nothing but the request: the signature first, and
/// the list only when the signature isn't the one already here.
pub fn update(dir: &Path) -> Result<Updated> {
    let now = loaded();
    let same = Updated {
        version: now.knowledge.version,
        newer: false,
        folders: now.knowledge.folders.len(),
    };
    let sig = String::from_utf8(fetch(&format!("{URL}.sig"), 200)?)
        .context("the signature isn't text")?;
    if std::fs::read_to_string(dir.join(SIG)).is_ok_and(|s| s == sig) {
        return Ok(same);
    }
    let bytes = fetch(URL, MAX)?;
    verify(&bytes, &sig, &key())?;
    let k = parse(&bytes)?;
    if k.version <= now.knowledge.version {
        return Ok(same);
    }
    std::fs::create_dir_all(dir)?;
    // The list, then its signature: one without the other is refused.
    for (name, data) in [(FILE, bytes.as_slice()), (SIG, sig.as_bytes())] {
        let tmp: PathBuf = dir.join(format!(".{name}.new"));
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, dir.join(name))?;
    }
    Ok(Updated {
        version: k.version,
        newer: true,
        folders: k.folders.len(),
    })
}

fn fetch(url: &str, max: usize) -> Result<Vec<u8>> {
    let mut curl = Command::new("/usr/bin/curl");
    curl.args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", "30", "--max-filesize", &max.to_string(), url]);
    let out = crate::space::run_within(curl, Duration::from_secs(40))
        .ok_or_else(|| anyhow!("{url} didn't answer"))?;
    if !out.status.success() {
        bail!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if out.stdout.len() > max {
        bail!("{url} is bigger than {max} bytes");
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn the_copy_built_in_says_each_thing_once_and_in_style() {
        let k = bundled();
        assert!(k.version >= 1);
        let mut seen = std::collections::HashSet::new();
        for e in &k.folders {
            let at = e.path.clone().or(e.app.clone()).unwrap();
            assert!(seen.insert(at.clone()), "{at} twice");
            for t in [&e.what, &e.grows, &e.helps].into_iter().flatten() {
                assert!(t.ends_with('.'), "{at}: {t}");
            }
            assert!(!e.note.as_deref().unwrap_or("").ends_with('.'), "{at}");
            // What's in a handler's folder and what helps, its handler
            // says, with its button.
            if let Some(p) = &e.path
                && crate::handlers::find(Path::new(p)).is_some()
            {
                assert!(
                    e.what.is_none() && e.grows.is_none() && e.helps.is_none(),
                    "{p}"
                );
            }
        }
    }

    #[test]
    fn a_folder_is_known_by_its_place_or_its_app() {
        let home = Path::new("/Users/me");
        let h = |p: &str| home.join(p);
        assert_eq!(name(&h(".gradle"), home).as_deref(), Some("Gradle"));
        assert_eq!(
            name(Path::new("/opt/homebrew"), home).as_deref(),
            Some("Homebrew")
        );
        // A place from `/` isn't one in the home folder.
        assert_eq!(name(&h("opt/homebrew"), home), None);
        assert!(note(&h("Library/CloudStorage/Dropbox"), home).is_some());
        assert_eq!(app_name("UBF8T346G9.Office"), "Microsoft Office (shared)");
        assert_eq!(app_name("group.com.apple.notes"), "Notes");
        assert_eq!(app_name("group.com.example.thing"), "com.example.thing");
        // An app's folder by its app, and what's in it too.
        let photos = h("Library/Containers/com.apple.mediaanalysisd");
        assert_eq!(
            called(&photos, home).as_deref(),
            Some("Photo analysis (macOS)")
        );
        assert_eq!(called(&photos.join("Data"), home), None);
        assert!(known(&photos, home).unwrap().grows.is_some());
        assert!(known(&photos.join("Data/Library/Caches"), home).is_some());
        assert!(known(&h("Library/Caches/com.apple.mediaanalysisd"), home).is_some());
        // Only what has something to say about it.
        assert_eq!(known(&h("Library/Metadata"), home), None);
        assert_eq!(called(&h("Library/Containers/com.example.x"), home), None);
    }

    /// The fixture the app decodes (app/Tests/knowledge-update.json).
    #[test]
    fn the_apps_update_fixture_has_the_fields_an_update_prints() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../app/Tests/knowledge-update.json")).unwrap();
        let real = serde_json::to_value(Updated {
            version: 2,
            newer: true,
            folders: 1,
        })
        .unwrap();
        let keys =
            |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&fixture), keys(&real));
    }

    #[test]
    fn only_names_that_say_nothing_about_you_are_generic() {
        let home = Path::new("/Users/me");
        let g = |p: &str| generic(&home.join(p), home);
        assert_eq!(
            g("Library/Containers/com.apple.Safari").as_deref(),
            Some("~/Library/Containers/com.apple.Safari")
        );
        assert_eq!(
            g("Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches").as_deref(),
            Some("~/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches")
        );
        // Inside an app's folder, only the names apps give their own.
        assert_eq!(
            g("Library/Application Support/Code/User/workspaceStorage").as_deref(),
            Some("~/Library/Application Support/Code")
        );
        assert_eq!(g("Library/Biome").as_deref(), Some("~/Library/Biome"));
        assert_eq!(
            g("Library/Developer/Xcode/DerivedData/Shop-abc").as_deref(),
            Some("~/Library/Developer/Xcode/DerivedData")
        );
        // Named after you: the folder alone, never what's in it.
        assert_eq!(
            g("Library/CloudStorage").as_deref(),
            Some("~/Library/CloudStorage")
        );
        assert_eq!(g("Library/CloudStorage/OneDrive-Jane Doe"), None);
        assert_eq!(g("Library/Mail/V10/jane@example.com"), None);
        // A tool's folder, and its own folders in it, not a project's.
        assert_eq!(g(".dartServer").as_deref(), Some("~/.dartServer"));
        assert_eq!(
            g(".bun/install/cache").as_deref(),
            Some("~/.bun/install/cache")
        );
        assert_eq!(
            g(".claude/projects/-Users-me-devel-shop").as_deref(),
            Some("~/.claude")
        );
        assert_eq!(
            g(".cache/codex-runtimes/primary").as_deref(),
            Some("~/.cache/codex-runtimes")
        );
        assert_eq!(
            g(".local/share/pnpm/store").as_deref(),
            Some("~/.local/share/pnpm")
        );
        // Your own folders, the home folder, other accounts and disks.
        for yours in ["devel/shop", "Documents", "Movies/Trip", ""] {
            assert_eq!(g(yours), None, "{yours}");
        }
        // Names made for this Mac would tell it apart.
        for one in [
            "Library/Containers/3F2504E0-4F89-11D3-9A0C-0305E82C3301",
            "Library/Caches/com.example.1234567",
            "Library/Application Support/0123456789abcdef0",
        ] {
            assert_eq!(g(one), None, "{one}");
        }
        assert_eq!(
            g("Library/Group Containers/UBF8T346G9.Office").as_deref(),
            Some("~/Library/Group Containers/UBF8T346G9.Office")
        );
        assert_eq!(generic(Path::new("/Users/other/Library"), home), None);
        assert_eq!(generic(Path::new("/Volumes/Backup/x"), home), None);
        assert_eq!(
            generic(Path::new("/usr/local"), home).as_deref(),
            Some("/usr/local")
        );
        assert_eq!(
            generic(
                Path::new("/Library/Application Support/Final Cut Pro/x/y"),
                home
            )
            .as_deref(),
            Some("/Library/Application Support/Final Cut Pro")
        );
    }

    #[test]
    fn only_a_signed_newer_copy_replaces_the_one_built_in() {
        let dir = tempfile::tempdir().unwrap();
        let signer = SigningKey::from_bytes(&[7; 32]);
        let key = signer.verifying_key();
        let put = |k: &Knowledge, sig_by: &SigningKey| {
            let bytes = serde_json::to_vec(k).unwrap();
            let sig =
                base64::engine::general_purpose::STANDARD.encode(sig_by.sign(&bytes).to_bytes());
            std::fs::write(dir.path().join(FILE), &bytes).unwrap();
            std::fs::write(dir.path().join(SIG), sig).unwrap();
        };
        let built = bundled();
        let mut newer = built.clone();
        newer.version += 1;
        newer.folders.insert(
            0,
            Entry {
                path: Some(".gradle".into()),
                name: Some("Gradle's caches".into()),
                ..Entry::default()
            },
        );

        // Nothing downloaded yet.
        assert!(!load(dir.path(), &key).downloaded);
        put(&newer, &signer);
        let l = load(dir.path(), &key);
        assert!(l.downloaded);
        assert_eq!(
            l.knowledge.folders[0].name.as_deref(),
            Some("Gradle's caches")
        );
        // Signed with another key, or changed after it was signed.
        put(&newer, &SigningKey::from_bytes(&[8; 32]));
        assert!(!load(dir.path(), &key).downloaded);
        put(&newer, &signer);
        let mut bytes = std::fs::read(dir.path().join(FILE)).unwrap();
        *bytes.last_mut().unwrap() = b' ';
        std::fs::write(dir.path().join(FILE), bytes).unwrap();
        assert!(!load(dir.path(), &key).downloaded);
        // An older one, though signed: the one built in.
        let mut older = newer.clone();
        older.version = built.version;
        put(&older, &signer);
        assert!(!load(dir.path(), &key).downloaded);

        // What isn't a list.
        let bad = |json: &str| parse(json.as_bytes()).is_err();
        assert!(bad(r#"{"version": 2, "folders": [{"name": "x"}]}"#));
        assert!(bad(
            r#"{"version": 2, "folders": [{"path": "a", "app": "b", "name": "x"}]}"#
        ));
        assert!(bad(
            r#"{"version": 2, "folders": [{"path": "../x", "name": "x"}]}"#
        ));
        assert!(bad(r#"{"version": 2, "folders": [{"path": "a"}]}"#));
        assert!(bad(
            r#"{"version": 2, "folders": [{"path": "a", "what": "x", "source": "http://x"}]}"#
        ));
        // A later format's field is left for the app that knows it.
        assert!(!bad(
            r#"{"version": 2, "folders": [{"path": "a", "what": "x.", "size": 3}]}"#
        ));
    }
}
