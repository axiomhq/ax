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

/// Euclidean wrap-around for cyclic selection: maps any (possibly
/// negative) index into `0..len`, wrapping at both ends. Returns 0
/// when `len == 0`. Replaces the open-coded `((i % n) + n) % n`
/// idiom used by the completion / picker movers.
pub fn wrap_index(i: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    (((i % len) + len) % len) as usize
}

#[cfg(test)]
mod tests {
    use super::{take_chars, wrap_index};

    #[test]
    fn wrap_index_cycles_both_directions() {
        assert_eq!(wrap_index(0, 3), 0);
        assert_eq!(wrap_index(3, 3), 0);
        assert_eq!(wrap_index(-1, 3), 2);
        assert_eq!(wrap_index(4, 3), 1);
        assert_eq!(wrap_index(-4, 3), 2);
        assert_eq!(wrap_index(5, 0), 0); // empty → 0, no panic
    }

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
