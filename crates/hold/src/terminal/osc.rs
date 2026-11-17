// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the window title (OSC 0 and 2) and the working folder (OSC 7) a
//! shell sets, across reads that split a sequence anywhere.

/// The longest OSC body kept. A longer one is skipped.
const MAX_BODY: usize = 4096;

/// What an OSC sequence set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Set {
    /// The window title.
    Title(String),
    /// The working folder, as a path.
    Cwd(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    Escape,
    Body,
    /// An ESC inside a body, which `\` turns into the end.
    BodyEscape,
}

/// Reads OSC sequences out of a terminal's output.
pub(crate) struct Scanner {
    state: State,
    body: Vec<u8>,
    skipping: bool,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: State::Text,
            body: Vec::new(),
            skipping: false,
        }
    }

    /// Reads `bytes`, calling `found` for each title or folder set.
    pub fn feed(&mut self, bytes: &[u8], mut found: impl FnMut(Set)) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (State::Text, 0x1b) => State::Escape,
                (State::Text, _) => State::Text,
                (State::Escape, b']') => {
                    self.body.clear();
                    self.skipping = false;
                    State::Body
                }
                (State::Escape, 0x1b) => State::Escape,
                (State::Escape, _) => State::Text,
                (State::Body, 0x07) => {
                    self.finish(&mut found);
                    State::Text
                }
                (State::Body, 0x1b) => State::BodyEscape,
                // CAN and SUB cancel a sequence.
                (State::Body, 0x18 | 0x1a) => State::Text,
                (State::Body, _) => {
                    self.keep(byte);
                    State::Body
                }
                (State::BodyEscape, b'\\') => {
                    self.finish(&mut found);
                    State::Text
                }
                (State::BodyEscape, b']') => {
                    self.body.clear();
                    self.skipping = false;
                    State::Body
                }
                (State::BodyEscape, _) => State::Text,
            };
        }
    }

    fn keep(&mut self, byte: u8) {
        if self.body.len() < MAX_BODY {
            self.body.push(byte);
        } else {
            self.skipping = true;
        }
    }

    fn finish(&mut self, found: &mut impl FnMut(Set)) {
        if self.skipping {
            return;
        }
        let body = String::from_utf8_lossy(&self.body);
        let Some((code, value)) = body.split_once(';') else {
            return;
        };
        match code {
            "0" | "2" => found(Set::Title(clean(value))),
            "7" => {
                if let Some(path) = file_url_path(value) {
                    found(Set::Cwd(path));
                }
            }
            _ => {}
        }
    }
}

/// A title without control characters, which a tab can't show.
fn clean(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

/// The path in a `file://host/path` URL, with its `%XX` escapes decoded.
fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| char::from(*b).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push(u8::try_from(high * 16 + low).ok()?);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    let path = String::from_utf8(out).ok()?;
    (!path.chars().any(char::is_control)).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Set> {
        let mut scanner = Scanner::new();
        let mut found = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, |set| found.push(set));
        }
        found
    }

    #[test]
    fn titles_and_folders_come_out_with_either_ending() {
        assert_eq!(
            scan(&[b"a\x1b]0;zsh\x07b\x1b]2;vim notes.md\x1b\\c"]),
            [Set::Title("zsh".into()), Set::Title("vim notes.md".into())]
        );
        assert_eq!(
            scan(&[b"\x1b]7;file://devbox/home/me/code/my%20app\x07"]),
            [Set::Cwd("/home/me/code/my app".into())]
        );
        // Without a host.
        assert_eq!(
            scan(&[b"\x1b]7;file:///tmp\x07"]),
            [Set::Cwd("/tmp".into())]
        );
    }

    #[test]
    fn a_sequence_split_anywhere_still_counts() {
        let whole: &[u8] = b"x\x1b]2;make test\x1b\\y";
        for at in 0..whole.len() {
            let (head, tail) = whole.split_at(at);
            assert_eq!(
                scan(&[head, tail]),
                [Set::Title("make test".into())],
                "{at}"
            );
        }
    }

    #[test]
    fn other_sequences_and_junk_set_nothing() {
        assert!(scan(&[b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07"]).is_empty());
        assert!(scan(&[b"\x1b[31mred\x1b[0m \x1b]7;http://x/y\x07"]).is_empty());
        assert!(scan(&[b"\x1b]0;cut\x18off\x07"]).is_empty());
        let long = [b"\x1b]0;".as_slice(), &[b'x'; MAX_BODY + 10], b"\x07"].concat();
        assert!(scan(&[&long]).is_empty());
        // Control characters leave a title.
        assert_eq!(scan(&[b"\x1b]0;a\tb\x07"]), [Set::Title("ab".into())]);
    }
}
