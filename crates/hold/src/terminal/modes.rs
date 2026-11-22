// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a program that runs waits for, from the pty's modes and the line
//! the cursor is on.
//!
//! The pty says whether the program reads a line or each key, and whether
//! it echoes. It can't say whether the program is reading at all, so a
//! program counts as waiting for you once it has printed nothing for
//! [`QUIET`] and reads keys, hides what you type, or left the cursor after
//! something that ends like a question. `sleep 60`, a build that links
//! for a while and `tail -f` leave the cursor at the start of a line or
//! after a progress bar, and read lines, so they don't.

use std::time::Duration;

use maxhq_term::Look;

use super::TermModes;

/// How long a program prints nothing before it can count as waiting.
pub(super) const QUIET: Duration = Duration::from_secs(1);

/// What the holder knows from the pty and the clock.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pty {
    /// Something other than the shell at its prompt runs.
    pub busy: bool,
    /// The program reads a line without echoing it, as at a password
    /// prompt.
    pub hidden: bool,
    /// The program reads each key as it comes.
    pub keys: bool,
    /// The program has printed nothing for [`QUIET`].
    pub quiet: bool,
}

/// What a terminal waits for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Read {
    /// It waits for a secret: at a prompt that hides what you type, or one
    /// in raw mode whose line asks for a secret.
    pub password: bool,
    pub modes: TermModes,
}

/// Reads what the terminal waits for from the pty and what the screen
/// shows.
pub(super) fn read(pty: Pty, look: &Look) -> Read {
    let full_screen = pty.busy && look.full_screen;
    let reads_keys = pty.busy && pty.keys;
    let raw_secret = reads_keys && !full_screen && look.asks_for_secret();
    let password = pty.hidden || raw_secret;
    let asks = pty.busy && !pty.keys && !pty.hidden && look.ends_like_a_question();
    let waiting = pty.busy && !full_screen && pty.quiet && (reads_keys || password || asks);
    Read {
        password,
        modes: TermModes {
            reads_keys: reads_keys && !password,
            full_screen,
            app_cursor: look.app_cursor,
            waiting,
            choices: if waiting { look.choices() } else { Vec::new() },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the screen shows after `output`.
    fn look(output: &str) -> Look {
        let mut screen = maxhq_term::screen(80, 24);
        screen.feed(output.as_bytes());
        screen.look()
    }

    const LINES: Pty = Pty {
        busy: true,
        hidden: false,
        keys: false,
        quiet: true,
    };

    const KEYS: Pty = Pty {
        keys: true,
        ..LINES
    };

    #[test]
    fn a_quiet_run_that_reads_lines_waits_only_at_a_question() {
        // `sleep 60`, a build that links for a while, `tail -f`.
        for output in [
            "",
            "$ sleep 60\r\n",
            "   Compiling maxd v0.1.6\r\n    Building [=======================> ] 431/432: maxd(bin)",
            "Nov 22 10:00:01 devbox app[12]: started\r\nNov 22 10:00:02 devbox app[12]: ready\r\n",
            "Downloading 45%",
        ] {
            let read = read(LINES, &look(output));
            assert!(!read.modes.waiting && !read.password, "{output:?}");
        }
        for output in [
            "package name: (demo) ",
            "Name: ",
            "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
        ] {
            assert!(read(LINES, &look(output)).modes.waiting, "{output:?}");
        }
    }

    #[test]
    fn nothing_waits_while_it_prints_or_once_the_shell_is_back() {
        let at = look("Continue? [y/N] ");
        let printing = Pty {
            quiet: false,
            ..KEYS
        };
        assert_eq!(
            read(printing, &at).modes,
            TermModes {
                reads_keys: true,
                ..TermModes::default()
            }
        );
        let shell = Pty {
            busy: false,
            ..KEYS
        };
        assert_eq!(read(shell, &at), Read::default());
    }

    #[test]
    fn a_raw_prompt_that_asks_for_a_secret_is_a_password_prompt() {
        let gh = read(KEYS, &look("\x1b[32m?\x1b[0m Paste your secret: ****"));
        assert!(gh.password && gh.modes.waiting && !gh.modes.reads_keys);
        // A picker that names a key reads keys, and asks for no secret.
        let picker = read(
            KEYS,
            &look("? Select a key type:  [Use arrows to move, type to filter]\r\n> ed25519"),
        );
        assert!(!picker.password && picker.modes.reads_keys && picker.modes.waiting);
        let any_key = read(KEYS, &look("Press any key to continue"));
        assert!(!any_key.password && any_key.modes.reads_keys);
        // A prompt that hides what you type in line mode, as `sudo` does.
        let sudo = read(
            Pty {
                hidden: true,
                ..LINES
            },
            &look("[sudo] password for max: "),
        );
        assert!(sudo.password && sudo.modes.waiting && !sudo.modes.reads_keys);
    }

    #[test]
    fn a_full_screen_program_never_waits() {
        let top = read(KEYS, &look("\x1b[?1049h\x1b[?1hPID  COMMAND\r\n1    init"));
        assert!(top.modes.full_screen && top.modes.app_cursor);
        assert!(!top.modes.waiting && top.modes.choices.is_empty());
    }

    #[test]
    fn a_waiting_yes_no_question_offers_both() {
        let ask = read(
            KEYS,
            &look("? Authenticate Git with your GitHub credentials? (Y/n) "),
        );
        assert_eq!(ask.modes.choices, ["y", "n"]);
        let printing = Pty {
            quiet: false,
            ..KEYS
        };
        assert!(read(printing, &look("Continue? [y/N] "))
            .modes
            .choices
            .is_empty());
    }
}
