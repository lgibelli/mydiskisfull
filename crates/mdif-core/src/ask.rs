//! Asking AI what a folder is, when nothing's known about it, and only when
//! someone asks: the folder's name as [`knowledge::generic`] makes it
//! (nothing inside it, none of your own folders), its size rounded and the
//! macOS version go to MyDiskIsFull's server (`worker/`), which asks Claude
//! once per name and keeps the answer for whoever asks next. Answers are
//! kept here too, beside the history, and said as the AI's, not checked,
//! until the list ([`knowledge`]) says something of the folder.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::knowledge::{self, Known};

pub const URL: &str = "https://ask.salamacchine.it/mydiskisfull/v1/folder";
/// The answers asked from this Mac, in the folder `init` is given.
const FILE: &str = "answers.json";
/// An answer bigger than this isn't one.
const MAX: usize = 16_000;
/// A text longer than this isn't shown.
const LONGEST: usize = 600;
/// What the AI couldn't say is asked again after this long.
const RETRY_UNKNOWN: i64 = 30 * 86_400;

/// What's sent, all of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Question {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub macos: String,
}

/// What the AI said of a folder; `known` false when it couldn't say.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Answer {
    pub name: String,
    pub known: bool,
    pub what: Option<String>,
    pub grows: Option<String>,
    pub helps: Option<String>,
    /// The model that wrote it.
    pub by: String,
    /// Whether the server had it already, from someone else's question.
    #[serde(default)]
    pub cached: bool,
    /// When it was asked from this Mac (Unix seconds).
    #[serde(default)]
    pub asked: i64,
}

#[derive(Default, Deserialize, Serialize)]
struct Answers {
    answers: Vec<Answer>,
}

static ANSWERS: OnceLock<Vec<Answer>> = OnceLock::new();

/// Takes the answers kept in `dir`. Once, before anything asks.
pub fn init(dir: &Path) {
    let _ = ANSWERS.set(read(dir).answers);
}

fn read(dir: &Path) -> Answers {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// What the AI said of the folder by this name (as `knowledge::generic`
/// makes it), when it could say.
pub fn answered(name: &str) -> Option<Known> {
    ANSWERS
        .get()?
        .iter()
        .find(|a| a.known && same(&a.name, name))
        .map(|a| Known {
            what: a.what.clone(),
            grows: a.grows.clone(),
            helps: a.helps.clone(),
            source: None,
            ai: Some(a.by.clone()),
        })
}

/// Two significant digits: enough to say how big, no more.
fn rounded(bytes: u64) -> u64 {
    let mut step = 1;
    while bytes / step >= 100 {
        step *= 10;
    }
    (bytes + step / 2) / step * step
}

fn macos() -> Result<String> {
    let mut sw = Command::new("/usr/bin/sw_vers");
    sw.arg("-productVersion");
    let out = crate::space::run_within(sw, Duration::from_secs(10))
        .filter(|o| o.status.success())
        .ok_or_else(|| anyhow!("couldn't read the macOS version"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// What would be sent to ask about `path`, or why nothing may be.
pub fn question(path: &Path, home: &Path, bytes: Option<u64>) -> Result<Question> {
    let name = knowledge::generic(path, home).ok_or_else(|| {
        anyhow!("its name could say something about you or this Mac, so it isn't sent")
    })?;
    Ok(Question {
        name,
        bytes: bytes.map(rounded),
        macos: macos()?,
    })
}

/// Asked from this Mac already: a known answer, or one the AI couldn't give
/// lately.
pub fn kept(dir: &Path, name: &str, now: i64) -> Option<Answer> {
    read(dir)
        .answers
        .into_iter()
        .find(|a| same(&a.name, name) && (a.known || now - a.asked < RETRY_UNKNOWN))
}

/// A text as it may be shown: printable, not too long, or nothing.
fn plain(t: Option<String>) -> Option<String> {
    let t = t?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty() && t.chars().count() <= LONGEST && !t.chars().any(char::is_control)).then_some(t)
}

/// The server, or for the Worker's local tests one on this Mac
/// (`MDIF_ASK_URL=http://127.0.0.1:<port>/…`).
fn url() -> (String, &'static str) {
    match std::env::var("MDIF_ASK_URL") {
        Ok(u) if u.starts_with("http://127.0.0.1:") => (u, "=http"),
        _ => (URL.to_string(), "=https"),
    }
}

/// Sends `q` and keeps the answer in `dir`.
pub fn ask(dir: &Path, q: &Question, now: i64) -> Result<Answer> {
    let (url, proto) = url();
    let body = serde_json::to_string(q)?;
    let mut curl = Command::new("/usr/bin/curl");
    curl.args(["--silent", "--show-error", "--proto", proto])
        .args(["--max-time", "90", "--max-filesize", &MAX.to_string()])
        .args(["--header", "content-type: application/json"])
        .args(["--data-raw", &body, "--write-out", "\n%{http_code}", &url]);
    let out = crate::space::run_within(curl, Duration::from_secs(100))
        .ok_or_else(|| anyhow!("MyDiskIsFull's server didn't answer"))?;
    if !out.status.success() {
        bail!(
            "couldn't reach MyDiskIsFull's server: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (reply, status) = text.rsplit_once('\n').unwrap_or(("", &text));
    match status.trim() {
        "200" => {}
        "400" => bail!("MyDiskIsFull's server doesn't take this folder's name"),
        "429" => bail!("too many questions from this Mac in a minute: try again in a minute"),
        "503" => bail!("the day's questions are used up: try again tomorrow"),
        "502" => bail!("the AI didn't answer: try again later"),
        s => bail!("MyDiskIsFull's server answered {s}"),
    }
    let a: Answer =
        serde_json::from_str(reply).context("MyDiskIsFull's server said something unexpected")?;
    if !same(&a.name, &q.name) {
        bail!("MyDiskIsFull's server answered about another folder");
    }
    let what = plain(a.what);
    let a = Answer {
        known: a.known && what.is_some(),
        what,
        grows: plain(a.grows),
        helps: plain(a.helps),
        by: a.by.chars().take(60).collect(),
        asked: now,
        ..a
    };
    let mut all = read(dir);
    all.answers.retain(|b| !same(&b.name, &a.name));
    all.answers.push(a.clone());
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{FILE}.new"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
    std::fs::rename(&tmp, dir.join(FILE))?;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_sent_to_two_digits() {
        assert_eq!(rounded(0), 0);
        assert_eq!(rounded(87), 87);
        assert_eq!(rounded(1_734_567_890), 1_700_000_000);
        assert_eq!(rounded(1_750_000_000), 1_800_000_000);
        assert_eq!(rounded(999), 1000);
    }

    #[test]
    fn only_a_generic_name_is_asked_about() {
        let home = Path::new("/Users/someone");
        assert!(question(&home.join("Documents/Taxes"), home, None).is_err());
        assert!(question(&home.join("Projects/thing"), home, None).is_err());
        let q = question(
            &home.join(".cache/codex-runtimes/node/v22"),
            home,
            Some(1_734_567_890),
        )
        .unwrap();
        assert_eq!(q.name, "~/.cache/codex-runtimes");
        assert_eq!(q.bytes, Some(1_700_000_000));
        assert!(
            q.macos.chars().next().unwrap().is_ascii_digit(),
            "{}",
            q.macos
        );
        // What's sent, all of it.
        let sent: serde_json::Value = serde_json::to_value(&q).unwrap();
        let mut keys: Vec<_> = sent.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["bytes", "macos", "name"]);
    }

    #[test]
    fn the_apps_answer_fixture_has_the_fields_an_answer_prints() {
        let fixture = include_str!("../../../app/Tests/knowledge-answer.json");
        let a: Answer = serde_json::from_str(fixture).unwrap();
        let keys = |v: serde_json::Value| {
            let mut k: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(serde_json::from_str(fixture).unwrap()),
            keys(serde_json::to_value(&a).unwrap())
        );
    }

    #[test]
    fn answers_are_kept_and_said_as_the_ais() {
        let dir = tempfile::tempdir().unwrap();
        let a = |name: &str, known, asked| Answer {
            name: name.into(),
            known,
            what: known.then(|| "Codex's runtimes.".into()),
            grows: None,
            helps: None,
            by: "claude-opus-5-5".into(),
            cached: false,
            asked,
        };
        let all = Answers {
            answers: vec![
                a("~/.cache/codex-runtimes", true, 0),
                a("~/.cache/nope", false, 100),
            ],
        };
        std::fs::write(dir.path().join(FILE), serde_json::to_vec(&all).unwrap()).unwrap();
        assert!(kept(dir.path(), "~/.cache/Codex-Runtimes", 1_000_000_000).is_some());
        assert!(kept(dir.path(), "~/.cache/nope", 200).is_some());
        assert!(kept(dir.path(), "~/.cache/nope", 100 + RETRY_UNKNOWN).is_none());
        assert!(kept(dir.path(), "~/.cache/other", 0).is_none());
        assert_eq!(
            plain(Some(" Two\n words. ".into())).as_deref(),
            Some("Two words.")
        );
        assert_eq!(plain(Some("x".repeat(LONGEST + 1))), None);
        assert_eq!(plain(Some("  ".into())), None);
    }
}
