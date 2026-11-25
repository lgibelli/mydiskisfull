//! Running `mdif`, as the Mac app's `Core.run` does: the app never measures
//! anything itself. Its output is read to the end (`mdif agent install` hands
//! the watcher it starts none of its handles, which CI checks), both pipes at
//! once so a full one never stalls it, within a time limit.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The commands the window may run: those the app has a page or a button
/// for, nothing else (of `knowledge`, `tidy` and `tweaks`, only what Ask AI
/// and Prevention run: [`allowed`]).
const ALLOWED: &[&str] = &[
    "agent",
    "alerts",
    "clean",
    "forecast",
    "keep",
    "knowledge",
    "map",
    "space",
    "status",
    "suggest",
    "tidy",
    "tweaks",
];

/// Whether the window may run `mdif <args>`: a command in [`ALLOWED`]; of
/// `knowledge`, the ChatGPT account and Ask AI (not `update`, which
/// downloads); of `tidy`, the list, `on`, `off` and `run`; of `tweaks`, the
/// list (`tweaks --json`), `apply` and `revert`.
fn allowed(args: &[String]) -> bool {
    let first = args.first().map(String::as_str).unwrap_or_default();
    let second = args.get(1).map(String::as_str);
    ALLOWED.contains(&first)
        && match first {
            "knowledge" => matches!(second, Some("account" | "sign-in" | "sign-out" | "ask")),
            "tidy" => {
                matches!(second, None | Some("on" | "off" | "run"))
                    || second.is_some_and(|s| s.starts_with("--"))
            }
            "tweaks" => {
                matches!(second, None | Some("apply" | "revert"))
                    || second.is_some_and(|s| s.starts_with("--"))
            }
            _ => true,
        }
}

/// Beside the app's own program, where the installer puts it; `MDIF` names
/// another (a build of the command line, while working on the app).
pub fn path() -> PathBuf {
    if let Some(p) = std::env::var_os("MDIF") {
        return PathBuf::from(p);
    }
    let name = if cfg!(windows) { "mdif.exe" } else { "mdif" };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Runs `mdif <args>`, and gives what it printed, or why it failed: the last
/// line it wrote to its error output, as it says what went wrong.
pub fn run(args: &[String], limit: Duration) -> Result<String, String> {
    run_at(args, limit, false)
}

/// [`run`], at idle priority when `low` (nobody pressed a button: out of the
/// way of whatever the computer is doing, as the Mac app's background QoS).
pub fn run_at(args: &[String], limit: Duration, low: bool) -> Result<String, String> {
    let first = args.first().map(String::as_str).unwrap_or_default();
    if !allowed(args) {
        let named: Vec<&str> = args.iter().take(2).map(String::as_str).collect();
        return Err(format!(
            "`mdif {}` isn't one the app runs.",
            named.join(" ")
        ));
    }
    let mut cmd = Command::new(path());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // No console window flashing up for each run.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
        cmd.creation_flags(CREATE_NO_WINDOW | if low { IDLE_PRIORITY_CLASS } else { 0 });
    }
    #[cfg(not(windows))]
    let _ = low;
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("mdif couldn't be started ({}): {e}", path().display()))?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut out);
            }
            out
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // The readers are left: what it started may still hold its pipes.
                return Err(format!(
                    "`mdif {first}` took longer than {} s, and was stopped.",
                    limit.as_secs()
                ));
            }
            Err(e) => return Err(format!("`mdif {first}`: {e}")),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if status.success() {
        return Ok(String::from_utf8_lossy(&out).into_owned());
    }
    let said = String::from_utf8_lossy(&err);
    let why = said
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.strip_prefix("Error: ").unwrap_or(l).to_string());
    Err(why.unwrap_or_else(|| format!("`mdif {first}` failed ({status}).")))
}

/// What the window asks for: `mdif <args>`, off the main thread.
#[tauri::command]
pub async fn mdif(args: Vec<String>, timeout_secs: Option<u64>) -> Result<String, String> {
    let limit = Duration::from_secs(timeout_secs.unwrap_or(120).clamp(5, 3600));
    tauri::async_runtime::spawn_blocking(move || run(&args, limit))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// The text literals a JavaScript array starts with: `"keep", "add"` of
    /// `["keep", "add", ...paths]`, up to the first thing that isn't one.
    fn leading_strings(mut rest: &str) -> Vec<String> {
        let mut out = Vec::new();
        loop {
            rest = rest.trim_start();
            let Some(after) = rest.strip_prefix('"') else {
                return out;
            };
            let Some(end) = after.find('"') else {
                return out;
            };
            out.push(after[..end].to_string());
            rest = after[end + 1..].trim_start();
            match rest.strip_prefix(',') {
                Some(r) => rest = r,
                None => return out,
            }
        }
    }

    fn scripts(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                scripts(&p, out);
            } else if p.extension().is_some_and(|x| x == "js") {
                out.push(p);
            }
        }
    }

    /// Every `mdif` command the window writes out is one it may run: the
    /// window in a browser answers from test files, never through
    /// [`allowed`], so only this catches a page whose commands are refused.
    #[test]
    fn the_window_asks_only_for_what_it_may_run() {
        let mut files = Vec::new();
        scripts(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/js"),
            &mut files,
        );
        let mut calls = 0;
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (i, _) in text.match_indices("mdifJson([") {
                let named = leading_strings(&text[i + "mdifJson([".len()..]);
                assert!(
                    !named.is_empty(),
                    "{}: a command that isn't written out",
                    file.display()
                );
                assert!(
                    allowed(&named),
                    "{}: `mdif {}` is refused",
                    file.display(),
                    named.join(" ")
                );
                calls += 1;
            }
        }
        // Ask AI's question is made by text.js's askArgs.
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/js/text.js"),
        )
        .unwrap();
        let ask = text
            .find("return [\"knowledge\", \"ask\"")
            .expect("askArgs");
        assert!(allowed(&leading_strings(&text[ask + "return [".len()..])));
        assert!(calls >= 15, "only {calls} calls found");
    }

    #[test]
    fn only_the_apps_commands_run() {
        for refused in [
            &["watch"][..],
            &["writers"],
            &[""],
            &["--help"],
            &["knowledge"],
            &["knowledge", "update"],
            &["knowledge", "about", "x"],
            &["knowledge", "--json"],
            &["tweaks", "x"],
            &["tidy", "x"],
        ] {
            assert!(!allowed(&args(refused)), "{refused:?}");
            let why = run(&args(refused), Duration::from_secs(5)).unwrap_err();
            assert!(why.contains("isn't one the app runs"), "{refused:?}: {why}");
        }
        assert!(run(&[], Duration::from_secs(5)).is_err());
        // What the window runs, as it runs it.
        for ok in [
            &["status", "--json"][..],
            &["keep", "add", r"C:\Users\me\big.iso", "--json"],
            &["knowledge", "account", "--json"],
            &["knowledge", "sign-in", "--json"],
            &["knowledge", "sign-out", "--json"],
            &[
                "knowledge",
                "ask",
                r"C:\x",
                "--bytes",
                "5",
                "--dry-run",
                "--json",
            ],
            &["tidy", "--json"],
            &["tidy", "on", "incremental", "--json"],
            &["tidy", "off", "incremental", "--json"],
            &["tidy", "run", "incremental", "--json"],
            &["tweaks", "--json"],
            &[
                "tweaks",
                "apply",
                "recycle-bin-auto-empty",
                "--dry-run",
                "--json",
            ],
            &["tweaks", "revert", "gradle-caches", "--json"],
        ] {
            assert!(allowed(&args(ok)), "{ok:?}");
        }
    }
}
