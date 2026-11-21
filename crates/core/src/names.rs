//! Comparing names the way a person reads them (#132).
//!
//! One name can be written in two Unicode forms: `café` with one character
//! for the é (composed, NFC, what Windows and Linux keyboards type), or with
//! an e followed by a combining accent (decomposed, what macOS and some
//! editors write). File systems keep the two apart (NTFS and ext4 both), so
//! two files can carry names that look the same. Wherever the app checks a
//! name is free or finds something by its name, it compares names in one
//! form, NFC; what is on disk keeps the form it was written in.

use unicode_normalization::UnicodeNormalization;

/// `name` in its composed form (NFC).
pub(crate) fn composed(name: &str) -> String {
    name.nfc().collect()
}

/// `name` as names are compared ignoring case: composed, then in lower
/// case (two names are the same name when these are equal).
pub(crate) fn folded(name: &str) -> String {
    composed(&name.to_lowercase())
}

/// Whether `a` and `b` are one name ignoring case and Unicode form.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    a == b || folded(a) == folded(b)
}

/// Whether `a` and `b` are one name ignoring surrounding blanks, case and
/// Unicode form.
pub(crate) fn same_trimmed_name(a: &str, b: &str) -> bool {
    same_name(a.trim(), b.trim())
}
