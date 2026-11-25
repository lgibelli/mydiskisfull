//! Variables as the user's shell sets them.
//!
//! The app and the watcher start from launchd, not from a shell, so they see
//! nothing `.zshrc` or `.bash_profile` exports. A tool told where its files
//! are by one (`GRADLE_USER_HOME`, `XDG_CONFIG_HOME` for Homebrew's
//! `brew.env`, `ANDROID_HOME`) would otherwise have them looked for, and
//! written, where the tool never reads them.
//!
//! Started from a shell (`SHLVL` is set), the process's environment is the
//! shell's. Otherwise the login shell is asked once per process, as an
//! interactive one, since that's where most people export them. It has
//! nothing to read, at most [`WAIT`] to answer, and
//! `MDIF_RESOLVING_ENVIRONMENT=1` so a startup file can skip work. Background
//! work that is avoiding privacy prompts doesn't ask: a startup file may
//! open a guarded folder ([`crate::privacy`]). On Windows, every process
//! gets the variables a user set, so its own are the user's.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::Duration;

/// The longest the login shell may take to start and answer.
#[cfg(unix)]
const WAIT: Duration = Duration::from_secs(5);

/// `name` as the user's shell has it; an empty value counts as unset.
pub fn var(name: &str) -> Option<OsString> {
    let asked = (!from_a_shell() && !crate::privacy::avoiding())
        .then(login_env)
        .flatten();
    match asked {
        Some(env) => env.get(OsStr::new(name)).cloned(),
        None => std::env::var_os(name),
    }
    .filter(|v| !v.is_empty())
}

/// Whether this process's environment is already the user's.
fn from_a_shell() -> bool {
    !cfg!(unix) || std::env::var_os("SHLVL").is_some()
}

/// The login shell's environment, asked once: `None` if it couldn't be.
fn login_env() -> Option<&'static HashMap<OsString, OsString>> {
    static ENV: OnceLock<Option<HashMap<OsString, OsString>>> = OnceLock::new();
    ENV.get_or_init(ask_login_shell).as_ref()
}

#[cfg(unix)]
fn ask_login_shell() -> Option<HashMap<OsString, OsString>> {
    login_shell().and_then(|sh| asked(&sh, WAIT))
}

#[cfg(not(unix))]
fn ask_login_shell() -> Option<HashMap<OsString, OsString>> {
    None
}

/// The account's login shell, as its record names it, else `SHELL`.
#[cfg(unix)]
fn login_shell() -> Option<PathBuf> {
    let recorded = {
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0 as libc::c_char; 4096];
        // SAFETY: `pw` and `buf` outlive the call; `found` is null or `&pw`.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pw,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        (rc == 0 && !found.is_null() && !pw.pw_shell.is_null()).then(|| {
            use std::os::unix::ffi::OsStrExt;
            // SAFETY: a NUL-ended string in `buf`, which is still alive.
            let s = unsafe { std::ffi::CStr::from_ptr(pw.pw_shell) };
            PathBuf::from(OsStr::from_bytes(s.to_bytes()))
        })
    };
    recorded
        .into_iter()
        .chain(std::env::var_os("SHELL").map(PathBuf::from))
        .find(|p| p.is_absolute() && p.is_file())
}

/// What the shell prints around its environment: a startup file may print
/// before it (a banner), and run something that keeps printing after. The
/// end comes right after the NUL that ends the last variable, and no value
/// can hold a NUL: a value with the marker's text in it isn't the end.
#[cfg(unix)]
const START: &[u8] = b"__MDIF_ENV_START__";
#[cfg(unix)]
const END: &[u8] = b"\0__MDIF_ENV_END__";

/// `shell`'s environment as an interactive login shell has it, if it gives
/// it within `wait`. Only up to the end marker is waited for: something a
/// startup file left running may keep the output open, so the shell's own
/// end isn't.
#[cfg(unix)]
fn asked(shell: &Path, wait: Duration) -> Option<HashMap<OsString, OsString>> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    // By full paths, so every shell (sh, zsh, fish) runs the same.
    let script =
        "/usr/bin/printf __MDIF_ENV_START__; /usr/bin/env -0; /usr/bin/printf __MDIF_ENV_END__";
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", script])
        .env("MDIF_RESOLVING_ENVIRONMENT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&chunk[..n]),
            }
            if let Some(env) = parse(&seen) {
                let _ = tx.send(env);
                return;
            }
        }
    });
    let env = rx.recv_timeout(wait).ok();
    // Done with it, or out of time: what it's doing now is a logout script.
    let _ = child.kill();
    std::thread::spawn(move || child.wait());
    env
}

/// The variables between the start marker and the end marker after it,
/// `NAME=value` each, NUL-ended; `None` until both are there.
#[cfg(unix)]
fn parse(out: &[u8]) -> Option<HashMap<OsString, OsString>> {
    use std::os::unix::ffi::OsStrExt;
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let from = find(out, START)? + START.len();
    let rest = &out[from..];
    // Up to and with the last variable's NUL.
    let end = find(rest, END)? + 1;
    Some(
        rest[..end]
            .split(|&b| b == 0)
            .filter_map(|entry| {
                let eq = entry.iter().position(|&b| b == b'=')?;
                Some((
                    OsStr::from_bytes(&entry[..eq]).to_os_string(),
                    OsStr::from_bytes(&entry[eq + 1..]).to_os_string(),
                ))
            })
            .filter(|(k, _)| !k.is_empty())
            .collect(),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// A stand-in for a login shell: `-l -i -c <script>`, run by `/bin/sh`
    /// after what `startup` does.
    fn shell(dir: &Path, startup: &str) -> PathBuf {
        let path = dir.join("sh");
        std::fs::write(
            &path,
            format!("#!/bin/sh\n{startup}\nshift 3\neval \"$1\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn what_a_startup_file_exports_is_found() {
        let t = tempfile::tempdir().unwrap();
        let sh = shell(
            t.path(),
            "echo 'Last login: today'\nexport GRADLE_USER_HOME=/opt/gradle\n\
             export WITH_LINES='a\nb=c'",
        );
        let env = asked(&sh, WAIT).unwrap();
        assert_eq!(env[OsStr::new("GRADLE_USER_HOME")], "/opt/gradle");
        assert_eq!(env[OsStr::new("WITH_LINES")], "a\nb=c");
        assert_eq!(env[OsStr::new("MDIF_RESOLVING_ENVIRONMENT")], "1");
    }

    #[test]
    fn a_shell_that_doesnt_answer_in_time_isnt_waited_for() {
        let t = tempfile::tempdir().unwrap();
        let sh = shell(t.path(), "sleep 30");
        let start = Instant::now();
        assert!(asked(&sh, Duration::from_millis(300)).is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn something_left_running_by_a_startup_file_isnt_waited_for() {
        let t = tempfile::tempdir().unwrap();
        // It keeps the output open long after the shell is done.
        let sh = shell(t.path(), "(sleep 30; echo late) &\nexport FOUND=yes");
        let start = Instant::now();
        let env = asked(&sh, WAIT).unwrap();
        assert_eq!(env[OsStr::new("FOUND")], "yes");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_value_with_the_markers_text_doesnt_end_the_list() {
        let out = b"banner __MDIF_ENV_START__A=1\0B=__MDIF_ENV_END__\0C=3\0__MDIF_ENV_END__after";
        let env = parse(out).unwrap();
        assert_eq!(env[OsStr::new("B")], "__MDIF_ENV_END__");
        assert_eq!(env[OsStr::new("C")], "3");
        // Read as far as B's value: not the end yet.
        assert!(parse(&out[..47]).is_none(), "not ended yet");
        assert!(parse(b"nothing").is_none());
    }

    #[test]
    #[ignore = "runs your login shell's startup files"]
    fn the_login_shell_answers() {
        let sh = login_shell().unwrap();
        let env = asked(&sh, WAIT).unwrap();
        assert!(env.contains_key(OsStr::new("PATH")), "{sh:?}: {env:?}");
        assert!(env.contains_key(OsStr::new("SHLVL")), "a shell: {env:?}");
    }

    #[test]
    fn a_shells_own_environment_is_used_as_it_is() {
        // Tests run from a shell (or CI's): SHLVL is set, nothing is asked.
        if std::env::var_os("SHLVL").is_some() {
            assert!(from_a_shell());
            assert_eq!(var("PATH"), std::env::var_os("PATH"));
        }
    }
}
