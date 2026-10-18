//! Making untrusted names safe to print.
//!
//! File and process names can contain anything but `/` and NUL, including
//! ESC and other control characters. Printed raw, a crafted name in a
//! scanned folder (say, `~/Downloads`) could move the cursor, rewrite earlier
//! output or retitle the terminal. Bidi overrides can make a name display as
//! something else. Everything shown to a terminal goes through [`printable`].

use std::borrow::Cow;

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
    fn bidi_overrides_are_visible() {
        assert_eq!(
            printable("invoice\u{202e}fdp.exe"),
            "invoice\\u{202e}fdp.exe"
        );
        assert_eq!(printable("x\u{2066}y"), "x\\u{2066}y");
    }
}
