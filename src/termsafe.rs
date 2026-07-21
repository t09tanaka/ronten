//! Terminal-output safety: escape control characters before printing
//! externally-controlled strings (paths, branch names, git stderr) to
//! stderr. Unix filenames may contain newlines, ESC, or other C0/C1
//! control characters, which can forge extra log lines or alter how the
//! terminal itself renders subsequent output (ANSI/OSC injection). Everyone
//! calling [`sanitize`] on such a string before interpolating it into an
//! `eprintln!` closes that hole the same way the frontend's
//! `revealControlChars` (see `frontend/src/lib/invisibles.ts`) does for the
//! browser UI — both use the same `⟨U+XXXX⟩` visible-token convention so a
//! reviewer sees one consistent escaping style everywhere.

use std::borrow::Cow;

/// True for any codepoint [`sanitize`] escapes: C0 controls (U+0000–U+001F,
/// including TAB and LF), DEL (U+007F), C1 controls (U+0080–U+009F), and the
/// Unicode line/paragraph separators U+2028/U+2029 (not control characters,
/// but capable of the same "extra line" forgery in terminals/logs that
/// interpret them).
fn needs_escape(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'
            | '\u{0080}'..='\u{009F}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Replaces every control character [`needs_escape`] flags with a visible
/// `⟨U+XXXX⟩` token (uppercase hex, zero-padded to at least 4 digits).
/// Returns `Cow::Borrowed` unchanged when `s` contains nothing to escape, so
/// the common case (a clean path or message) allocates nothing.
pub fn sanitize(s: &str) -> Cow<'_, str> {
    if !s.chars().any(needs_escape) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if needs_escape(c) {
            out.push_str(&format!("⟨U+{:04X}⟩", c as u32));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_string_is_borrowed_unchanged() {
        let s = "src/main.rs";
        match sanitize(s) {
            Cow::Borrowed(b) => assert_eq!(b, s),
            Cow::Owned(_) => panic!("expected a borrowed fast path for a clean string"),
        }
    }

    #[test]
    fn escapes_esc_and_newline() {
        assert_eq!(sanitize("a\x1bb\nc"), "a⟨U+001B⟩b⟨U+000A⟩c");
    }

    #[test]
    fn escapes_tab() {
        assert_eq!(sanitize("a\tb"), "a⟨U+0009⟩b");
    }

    #[test]
    fn escapes_carriage_return() {
        assert_eq!(sanitize("a\rb"), "a⟨U+000D⟩b");
    }

    #[test]
    fn escapes_del_and_c1() {
        // U+009C (C1 "STRING TERMINATOR") as raw UTF-8 bytes.
        let s = "a\u{7f}b\u{9c}c";
        assert_eq!(sanitize(s), "a⟨U+007F⟩b⟨U+009C⟩c");
    }

    #[test]
    fn escapes_line_and_paragraph_separator() {
        assert_eq!(sanitize("a\u{2028}b\u{2029}c"), "a⟨U+2028⟩b⟨U+2029⟩c");
    }

    #[test]
    fn leaves_normal_unicode_untouched() {
        let s = "日本語 emoji 🎉 — normal text";
        match sanitize(s) {
            Cow::Borrowed(b) => assert_eq!(b, s),
            Cow::Owned(_) => panic!("expected a borrowed fast path for normal text"),
        }
    }

    #[test]
    fn escapes_null_byte() {
        assert_eq!(sanitize("a\0b"), "a⟨U+0000⟩b");
    }
}
