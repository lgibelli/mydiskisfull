//! Making untrusted names safe to print.
//!
//! File and process names can contain anything but `/` and NUL, including
//! ESC and other control characters. Printed raw, a crafted name in a
//! scanned folder (say, `~/Downloads`) could move the cursor, rewrite earlier
//! output or retitle the terminal. Bidi overrides can make a name display as
//! something else. Everything shown to a terminal goes through [`printable`].

use std::borrow::Cow;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};

/// `s` with control characters (C0, DEL, C1) and bidi override/isolate
/// characters replaced by visible `\u{..}` escapes. Ordinary text, including
/// non-ASCII letters, is returned unchanged and unallocated.
pub fn printable(s: &str) -> Cow<'_, str> {
    if !s.chars().any(needs_escape) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if needs_escape(c) {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

fn needs_escape(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A path in JSON whatever its names hold (serde's own refuses one that isn't
/// Unicode, which fails the whole answer): an NTFS name with half a surrogate
/// pair is written with U+FFFD in its place.
pub fn lossy_path<S: serde::Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy())
}

/// The same, for a list of paths ([`lossy_path`]).
pub fn lossy_paths<S: serde::Serializer>(paths: &[PathBuf], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(paths.iter().map(|p| p.to_string_lossy()))
}

/// `path` with the home folder written `~` and its names joined by this
/// system's separator (`~/Library/Logs`, `~\AppData\Local`); `None` when it
/// isn't in `home`. Not yet [`printable`].
pub fn tilde(path: &Path, home: &Path) -> Option<String> {
    let rest = path.strip_prefix(home).ok()?;
    if rest.as_os_str().is_empty() {
        return Some("~".into());
    }
    let names: Vec<Cow<'_, str>> = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    Some(format!(
        "~{MAIN_SEPARATOR}{}",
        names.join(MAIN_SEPARATOR_STR)
    ))
}

/// `s` quoted for the shell a printed command is pasted into, whatever it
/// holds: a POSIX shell's on a Mac, PowerShell's on Windows.
pub fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        powershell_quote(s)
    } else {
        posix_quote(s)
    }
}

/// `'…'`, with a `'` inside written as `'\''`.
fn posix_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `'…'`, in which PowerShell expands nothing; a quote inside is doubled, the
/// curly ones too, as PowerShell takes them for quotes.
fn powershell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_borrowed_unchanged() {
        assert!(matches!(printable("Final Cut Pro.app"), Cow::Borrowed(_)));
        assert_eq!(printable("città · 日本 · ñ"), "città · 日本 · ñ");
    }

    #[test]
    fn terminal_escapes_are_neutralised() {
        assert_eq!(printable("a\x1b[2Jb"), "a\\u{1b}[2Jb");
        assert_eq!(printable("bell\x07\r\n"), "bell\\u{7}\\u{d}\\u{a}");
        assert_eq!(printable("\u{9b}31m"), "\\u{9b}31m", "C1 CSI");
        assert_eq!(printable("del\x7f"), "del\\u{7f}");
    }

    #[test]
    fn the_home_folder_is_a_tilde_with_this_systems_separator() {
        let home = Path::new("/Users/me");
        let sep = MAIN_SEPARATOR;
        assert_eq!(tilde(home, home).as_deref(), Some("~"));
        assert_eq!(
            tilde(Path::new("/Users/me/Library/Logs"), home),
            Some(format!("~{sep}Library{sep}Logs"))
        );
        assert_eq!(tilde(Path::new("/Users/other"), home), None);
    }

    #[test]
    fn a_path_that_isnt_unicode_is_still_json() {
        #[derive(serde::Serialize)]
        struct Named {
            #[serde(serialize_with = "lossy_path")]
            path: PathBuf,
        }
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(b"/a/b\xffc"))
        };
        #[cfg(windows)]
        let odd = {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(&[
                0x43, 0x3a, 0x5c, 0xd800, 0x78,
            ]))
        };
        let json = serde_json::to_string(&Named { path: odd }).unwrap();
        assert!(json.contains('\u{fffd}'), "{json}");
    }

    #[test]
    fn quoted_for_each_shell() {
        assert_eq!(posix_quote("/Users/me/it's"), r"'/Users/me/it'\''s'");
        assert_eq!(posix_quote("$HOME `x`"), "'$HOME `x`'");
        assert_eq!(
            powershell_quote(r"C:\Users\me\it's"),
            r"'C:\Users\me\it''s'"
        );
        assert_eq!(
            powershell_quote("C:\\Luca\u{2019}s $env:x"),
            "'C:\\Luca\u{2019}\u{2019}s $env:x'"
        );
    }

    #[test]
    fn bidi_overrides_are_visible() {
        assert_eq!(
            printable("invoice\u{202e}fdp.exe"),
            "invoice\\u{202e}fdp.exe"
        );
        assert_eq!(printable("x\u{2066}y"), "x\\u{2066}y");
    }
}
