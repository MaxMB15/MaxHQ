// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Words for a POSIX shell. `word` leaves a word as it is when it holds
//! nothing a shell reads, as the shared `shellQuote` does, and `quoted`
//! always puts it in single quotes.

/// `text` as one word for `sh`: as it is when it holds nothing a shell
/// reads, else in single quotes.
pub fn word(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        text.to_owned()
    } else {
        quoted(text)
    }
}

/// `text` in single quotes for `sh`, each quote in it closed, escaped and
/// opened again.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_quoted_only_when_a_shell_would_read_them() {
        assert_eq!(word("dev@devbox"), "dev@devbox");
        assert_eq!(word("/usr/bin/brew"), "/usr/bin/brew");
        assert_eq!(word(""), "''");
        assert_eq!(word("a b"), "'a b'");
        assert_eq!(word("it's"), r"'it'\''s'");
        assert_eq!(word("$(x)"), "'$(x)'");
    }

    #[test]
    fn quoted_always_quotes() {
        assert_eq!(quoted("plain"), "'plain'");
        assert_eq!(quoted("it's"), r"'it'\''s'");
    }
}
