// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The marker a `!` run prints just before its command, so the holder can
//! drop what the shell's startup files printed first: a banner, a title, a
//! theme's prompt.
//!
//! The run's shell gets `printf '<marker>'; <command>`. Everything before
//! the first marker goes nowhere, the marker included, and offsets count
//! from the byte after it. Only the first one counts: the command's own
//! output passes untouched, even when it prints the same bytes.
//!
//! A startup file may run something else, end the shell or fail, so the
//! marker never comes. Then the holder lets out what it held, so the user
//! sees what happened:
//!
//! - when the program ends,
//! - when more than [`HELD_CAP`] bytes wait,
//! - when the terminal waits for a password, or for keys as a shell's line
//!   editor or a full-screen program reads them.

/// The marker's body: a private OSC code of our own, which terminals that
/// don't know it skip. Not OSC 133, which shell integrations send.
macro_rules! body {
    () => {
        "7346;maxhq-run"
    };
}

/// What the run prints before its command.
pub(super) const MARKER: &[u8] = concat!("\x1b]", body!(), "\x07").as_bytes();

/// The shell code that prints [`MARKER`]. sh, bash, zsh and fish all read
/// these octal escapes in `printf`.
const PRINT_MARKER: &str = concat!(r"printf '\033]", body!(), r"\007'; ");

/// How much output may wait for the marker.
pub(super) const HELD_CAP: usize = 64 * 1024;

/// The script for the shell's `-c`: the marker, then `command` as typed.
/// They share a line, so the shell's errors give the command's own line
/// numbers.
pub(super) fn after_marker(command: &str) -> String {
    format!("{PRINT_MARKER}{command}")
}

/// Output held back until the marker.
pub(super) struct Cutoff {
    held: Vec<u8>,
}

impl Cutoff {
    pub fn new() -> Self {
        Self { held: Vec::new() }
    }

    /// Takes the next output. `None` while it waits for the marker. Then
    /// what follows the marker, or all it held once that passes the cap,
    /// and the wait is over.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        // The marker may have started in what's held already.
        let from = self.held.len().saturating_sub(MARKER.len() - 1);
        self.held.extend_from_slice(bytes);
        let found = self.held[from..]
            .windows(MARKER.len())
            .position(|window| window == MARKER);
        if let Some(at) = found {
            return Some(self.held.split_off(from + at + MARKER.len()));
        }
        (self.held.len() > HELD_CAP).then(|| std::mem::take(&mut self.held))
    }

    /// All it held, for when the marker won't come.
    pub fn release(self) -> Vec<u8> {
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What goes on from `chunks`, and whether the wait is over.
    fn cut(chunks: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut cutoff = Some(Cutoff::new());
        let mut shown = Vec::new();
        for chunk in chunks {
            match cutoff.as_mut().map(|cutoff| cutoff.feed(chunk)) {
                None => shown.extend_from_slice(chunk),
                Some(None) => {}
                Some(Some(rest)) => {
                    cutoff = None;
                    shown.extend(rest);
                }
            }
        }
        (shown, cutoff.is_none())
    }

    #[test]
    fn the_script_prints_the_marker_then_the_command() {
        assert_eq!(MARKER, b"\x1b]7346;maxhq-run\x07");
        assert_eq!(
            after_marker("ls -la"),
            r"printf '\033]7346;maxhq-run\007'; ls -la"
        );
    }

    #[test]
    fn what_comes_before_the_marker_goes_and_what_follows_stays() {
        let banner: &[u8] = b"Welcome!\r\n\x1b]0;rc title\x07";
        let output = [banner, MARKER, b"hello\r\n"].concat();
        assert_eq!(cut(&[&output]), (b"hello\r\n".to_vec(), true));
        assert_eq!(
            cut(&[banner, MARKER, b"one\r\n", b"two\r\n"]).0,
            b"one\r\ntwo\r\n"
        );
        // A marker at the very end leaves nothing yet, and the wait is over.
        assert_eq!(cut(&[banner, MARKER]), (Vec::new(), true));
        // Without it, everything waits.
        assert_eq!(cut(&[banner, b"more"]), (Vec::new(), false));
    }

    #[test]
    fn a_marker_split_anywhere_still_counts() {
        let output = [b"noise".as_slice(), MARKER, b"after"].concat();
        for at in 0..output.len() {
            let (head, tail) = output.split_at(at);
            assert_eq!(cut(&[head, tail]), (b"after".to_vec(), true), "{at}");
        }
        // One byte at a time.
        let bytes: Vec<&[u8]> = output.chunks(1).collect();
        assert_eq!(cut(&bytes), (b"after".to_vec(), true));
    }

    #[test]
    fn only_the_first_marker_counts() {
        let output = [b"rc".as_slice(), MARKER, b"a", MARKER, b"b"].concat();
        assert_eq!(cut(&[&output]).0, [b"a".as_slice(), MARKER, b"b"].concat());
        // A second one in a later read passes too.
        assert_eq!(
            cut(&[b"rc", MARKER, b"a", MARKER]).0,
            [b"a".as_slice(), MARKER].concat()
        );
    }

    #[test]
    fn past_the_cap_everything_held_goes_on() {
        let line = [b'x'; 1024];
        let mut cutoff = Cutoff::new();
        for _ in 0..HELD_CAP / line.len() {
            assert_eq!(cutoff.feed(&line), None);
        }
        let all = cutoff.feed(b"!").expect("the wait is over past the cap");
        assert_eq!(all.len(), HELD_CAP + 1);
        assert!(all.ends_with(b"x!"));
        // After it nothing is held, and a late marker passes like any byte.
        let (shown, over) = cut(&[&[b'y'; HELD_CAP + 1], MARKER, b"z"]);
        assert!(over);
        assert!(shown.ends_with(&[MARKER, b"z"].concat()));
        assert_eq!(shown.len(), HELD_CAP + 1 + MARKER.len() + 1);
    }

    #[test]
    fn a_program_that_ends_without_the_marker_shows_all_it_held() {
        let mut cutoff = Cutoff::new();
        assert_eq!(cutoff.feed(b"zsh: no such file: .zshrc\r\n"), None);
        assert_eq!(cutoff.feed(b"\x1b]7346;max"), None);
        assert_eq!(
            cutoff.release(),
            b"zsh: no such file: .zshrc\r\n\x1b]7346;max"
        );
        assert!(Cutoff::new().release().is_empty());
    }
}
