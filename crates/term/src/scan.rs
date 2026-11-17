// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the few escape sequences the emulator acts on beside its backend,
//! one byte at a time, so a sequence split across two reads still counts.

/// The most parameter bytes kept for one sequence. The sequences watched for
/// are far shorter, so a longer one is none of them.
const MAX_PARAMS: usize = 32;

/// A watched sequence, seen at its last byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// The program turned the alternate screen on or off. `legacy` when it
    /// named mode 47 or 1047, which the backend doesn't know, so the
    /// emulator asks for 1049 in their place.
    AltScreen { on: bool, legacy: bool },
    /// `CSI 3 J`, which clears the lines scrolled off the screen.
    ClearSaved,
    /// `ESC c`, which resets the terminal.
    Reset,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    /// Inside `CSI`, `private` once a `?` came first.
    Csi {
        private: bool,
    },
}

/// Watches output for [`Seen`] sequences.
#[derive(Debug, Default)]
pub(crate) struct Scanner {
    state: State,
    params: [u8; MAX_PARAMS],
    len: usize,
    /// The sequence has something none of the watched ones have.
    other: bool,
}

impl Scanner {
    /// Takes the next byte, and says what a sequence it ends asked for.
    pub(crate) fn step(&mut self, byte: u8) -> Option<Seen> {
        match self.state {
            State::Ground => {
                if byte == 0x1b {
                    self.state = State::Escape;
                }
                None
            }
            State::Escape => match byte {
                b'[' => {
                    self.state = State::Csi { private: false };
                    self.len = 0;
                    self.other = false;
                    None
                }
                b'c' => {
                    self.state = State::Ground;
                    Some(Seen::Reset)
                }
                0x1b => None,
                _ => {
                    self.state = State::Ground;
                    None
                }
            },
            State::Csi { private } => match byte {
                b'?' if self.len == 0 && !private && !self.other => {
                    self.state = State::Csi { private: true };
                    None
                }
                b'0'..=b'9' | b';' => {
                    if self.len < MAX_PARAMS {
                        self.params[self.len] = byte;
                        self.len += 1;
                    } else {
                        self.other = true;
                    }
                    None
                }
                0x40..=0x7e => {
                    self.state = State::Ground;
                    if self.other {
                        None
                    } else {
                        self.finish(private, byte)
                    }
                }
                0x1b => {
                    self.state = State::Escape;
                    None
                }
                // A control byte inside a sequence runs on its own and the
                // sequence goes on.
                0x00..=0x1f | 0x7f => None,
                // Other markers and intermediates.
                0x20..=0x3f => {
                    self.other = true;
                    None
                }
                _ => {
                    self.state = State::Ground;
                    None
                }
            },
        }
    }

    fn finish(&self, private: bool, last: u8) -> Option<Seen> {
        let params = &self.params[..self.len];
        let numbers = || {
            params
                .split(|&byte| byte == b';')
                .map(|digits| std::str::from_utf8(digits).ok()?.parse::<u32>().ok())
        };
        match (private, last) {
            (true, b'h' | b'l') => {
                let mut alt = false;
                let mut legacy = false;
                for number in numbers().flatten() {
                    alt |= number == 1049;
                    legacy |= number == 47 || number == 1047;
                }
                (alt || legacy).then_some(Seen::AltScreen {
                    on: last == b'h',
                    legacy,
                })
            }
            (false, b'J') => {
                let mut numbers = numbers();
                (numbers.next() == Some(Some(3)) && numbers.next().is_none())
                    .then_some(Seen::ClearSaved)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Vec<Seen> {
        let mut scanner = Scanner::default();
        bytes.iter().filter_map(|&b| scanner.step(b)).collect()
    }

    #[test]
    fn finds_the_alternate_screen_in_any_of_its_modes() {
        assert_eq!(
            scan(b"a\x1b[?1049hb\x1b[?1049l"),
            [
                Seen::AltScreen {
                    on: true,
                    legacy: false
                },
                Seen::AltScreen {
                    on: false,
                    legacy: false
                },
            ]
        );
        assert_eq!(
            scan(b"\x1b[?47h\x1b[?1047l"),
            [
                Seen::AltScreen {
                    on: true,
                    legacy: true
                },
                Seen::AltScreen {
                    on: false,
                    legacy: true
                },
            ]
        );
        assert_eq!(
            scan(b"\x1b[?25;1049h"),
            [Seen::AltScreen {
                on: true,
                legacy: false
            }]
        );
    }

    #[test]
    fn leaves_other_sequences_alone() {
        assert!(scan(b"\x1b[1049h\x1b[?25l\x1b[2J\x1b[31m\x1b]0;?1049h\x07").is_empty());
        assert!(scan(b"\x1b[>1049h\x1b[?1049$p").is_empty());
    }

    #[test]
    fn finds_clears_and_resets() {
        assert_eq!(
            scan(b"\x1b[H\x1b[2J\x1b[3J\x1bc"),
            [Seen::ClearSaved, Seen::Reset]
        );
    }

    #[test]
    fn a_sequence_split_across_reads_still_counts() {
        let mut scanner = Scanner::default();
        let first: Vec<_> = b"x\x1b[?10"
            .iter()
            .filter_map(|&b| scanner.step(b))
            .collect();
        assert!(first.is_empty());
        let second: Vec<_> = b"49h".iter().filter_map(|&b| scanner.step(b)).collect();
        assert_eq!(
            second,
            [Seen::AltScreen {
                on: true,
                legacy: false
            }]
        );
    }
}
