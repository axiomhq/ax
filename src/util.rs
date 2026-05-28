//! Tiny shared helpers that don't belong to any one feature.
//!
//! Kept deliberately small — anything that grows past a single
//! responsibility should graduate into its own module.

pub mod atomic;

/// First `n` characters of `s` as an owned `String`. A char-safe
/// replacement for `&s[..n]`: slicing a `&str` at a fixed byte
/// offset panics when that offset lands inside a multi-byte UTF-8
/// character, whereas this counts by `char` and can never panic.
/// Returns the whole string when it has `<= n` characters.
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::take_chars;

    #[test]
    fn take_chars_ascii() {
        assert_eq!(take_chars("abcdef", 3), "abc");
        assert_eq!(take_chars("abc", 8), "abc");
        assert_eq!(take_chars("", 4), "");
    }

    #[test]
    fn take_chars_never_panics_on_multibyte_boundary() {
        // 'Ａ' (U+FF21) is 3 bytes; byte index 3 is inside the 2nd
        // char — `&s[..3]` would panic, `take_chars` must not.
        let s = "aＡＡＡＡ"; // 1 + 4*3 = 13 bytes, 5 chars
        assert_eq!(take_chars(s, 3), "aＡＡ");
        assert_eq!(take_chars(s, 99), s);
    }
}
