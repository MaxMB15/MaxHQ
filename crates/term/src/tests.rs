// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use crate::{emulator, Emulator, Transcript, FULL_SCREEN, MAX_BYTES, MAX_LINES, SECRET_SENT};

fn run(cols: u16, rows: u16, output: &[u8]) -> Transcript {
    let mut emulator = emulator(cols, rows);
    emulator.feed(output);
    emulator.transcript()
}

fn text(output: &[u8]) -> String {
    run(80, 24, output).text
}

#[test]
fn plain_lines_come_back_as_printed() {
    let transcript = run(80, 24, b"a\r\nb\r\n");
    assert_eq!(
        transcript,
        Transcript {
            text: "a\nb".into(),
            lines: 2,
            truncated: false,
        }
    );
    // Blank lines a program printed stay; the screen's empty rows below go.
    assert_eq!(text(b"\r\nx  \r\n\r\ny\r\n\r\n"), "\nx\n\ny");
    assert_eq!(run(80, 24, b"").lines, 0);
    assert_eq!(text(b"").as_str(), "");
}

#[test]
fn colors_and_styles_come_back_as_sgr_and_each_line_ends_unstyled() {
    assert_eq!(
        text(b"\x1b[31mFAIL\x1b[0m src/cache.test.ts\r\n"),
        "\x1b[0;31mFAIL\x1b[0m src/cache.test.ts"
    );
    assert_eq!(
        text(b"\x1b[1;92;44mok\r\nnext\x1b[0m\r\n"),
        "\x1b[0;1;92;44mok\x1b[0m\n\x1b[0;1;92;44mnext\x1b[0m"
    );
    assert_eq!(
        text(b"\x1b[38;5;208mo\x1b[48;2;10;20;30mr\x1b[m"),
        "\x1b[0;38;5;208mo\x1b[0;38;5;208;48;2;10;20;30mr\x1b[0m"
    );
    assert_eq!(
        text(b"\x1b[2;3;4;7;9mx\x1b[22;23;24;27;29my"),
        "\x1b[0;2;3;4;7;9mx\x1b[0my"
    );
    // Hidden text stays hidden.
    assert_eq!(text(b"a\x1b[8mpw\x1b[28mb"), "a  b");
}

#[test]
fn nothing_but_sgr_survives() {
    let out = text(b"\x1b]0;title\x07\x1b[2J\x1b[H\x1b[5;10Hx\x1b[K\x1b[?25l\x1b]8;;https://example.com\x07link\x1b]8;;\x07");
    assert!(!out.contains("title"), "{out:?}");
    for (at, _) in out.match_indices('\x1b') {
        let rest = &out[at..];
        let end = rest.find('m').unwrap();
        assert!(
            rest[1..end].starts_with('[')
                && rest[2..end]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b';'),
            "{out:?}"
        );
    }
    assert!(out.ends_with("         xlink"), "{out:?}");
}

#[test]
fn a_progress_bar_leaves_its_last_state() {
    assert_eq!(text(b"\r50%\r100%"), "100%");
    assert_eq!(
        text(b"build [##  ]\rbuild [####]\r\ndone\r\n"),
        "build [####]\ndone"
    );
    // A bar that moves back up a line to redraw.
    assert_eq!(
        text(b"a 1/2\r\nb\r\n\x1b[2A\x1b[2Ka 2/2\r\n\x1b[1B"),
        "a 2/2\nb"
    );
}

#[test]
fn clearing_the_screen_keeps_what_scrolled_above_it() {
    let transcript = run(20, 4, b"one\r\ntwo\r\n\x1b[H\x1b[2Jthree\r\n");
    assert_eq!(transcript.text, "one\ntwo\nthree");
    assert!(!transcript.truncated);
    // `clear` also empties the scrollback, as a terminal does.
    let transcript = run(20, 4, b"one\r\ntwo\r\n\x1b[H\x1b[2J\x1b[3Jthree\r\n");
    assert_eq!(transcript.text, "three");
    assert_eq!(transcript.lines, 1);
    assert!(!transcript.truncated);
}

#[test]
fn a_long_line_that_wrapped_reads_as_one() {
    let long = "x".repeat(25);
    let transcript = run(10, 5, format!("{long}\r\nend\r\n").as_bytes());
    assert_eq!(transcript.text, format!("{long}\nend"));
    assert_eq!(transcript.lines, 2);
}

#[test]
fn the_terminal_keeps_its_last_lines_and_counts_them_all() {
    let mut emulator = emulator(80, 24);
    let total = MAX_LINES + 7_000;
    for at in 1..=total {
        emulator.feed(format!("line {at}\r\n").as_bytes());
    }
    let transcript = emulator.transcript();
    assert_eq!(transcript.lines, total as u32);
    assert!(transcript.truncated);
    let lines: Vec<&str> = transcript.text.split('\n').collect();
    assert_eq!(lines.len(), MAX_LINES);
    assert_eq!(lines[0], format!("line {}", total - MAX_LINES + 1));
    assert_eq!(lines[MAX_LINES - 1], format!("line {total}"));

    // The same in one read, past the backend's own scrollback.
    let all: String = (1..=total).map(|at| format!("line {at}\r\n")).collect();
    let transcript = run(80, 24, all.as_bytes());
    assert_eq!(transcript.lines, total as u32);
    assert!(transcript.text.ends_with(&format!("line {total}")));
    assert_eq!(transcript.text.split('\n').count(), MAX_LINES);
}

#[test]
fn the_terminal_keeps_its_last_bytes() {
    let line = "y".repeat(1_000);
    let all: String = (0..1_000).map(|_| format!("{line}\r\n")).collect();
    let transcript = run(200, 24, all.as_bytes());
    assert_eq!(transcript.lines, 1_000);
    assert!(transcript.truncated);
    assert!(transcript.text.len() <= MAX_BYTES);
    assert!(transcript.text.len() > MAX_BYTES - 1_001);
    assert!(transcript.text.split('\n').all(|kept| kept == line));
}

#[test]
fn one_line_past_the_byte_limit_keeps_its_end() {
    let mut emulator = emulator(100, 24);
    let mut start = String::from("\x1b[32m");
    start.push_str(&"é".repeat(MAX_BYTES / 2));
    emulator.feed(start.as_bytes());
    emulator.feed(b"tail\x1b[0m\r\n");
    let transcript = emulator.transcript();
    assert!(transcript.truncated);
    assert_eq!(transcript.lines, 1);
    assert!(transcript.text.len() <= MAX_BYTES);
    assert!(
        transcript.text.ends_with("tail\x1b[0m"),
        "{}",
        &transcript.text[transcript.text.len() - 20..]
    );
}

#[test]
fn a_line_longer_than_the_scrollback_keeps_its_end() {
    let mut emulator = emulator(80, 24);
    emulator.feed(b"first\r\n");
    for _ in 0..600 {
        emulator.feed(&[b'z'; 1_000]);
    }
    emulator.feed(b"END\r\n");
    let transcript = emulator.transcript();
    assert!(transcript.truncated);
    assert_eq!(transcript.lines, 2);
    assert!(transcript.text.ends_with("zzzEND"));
    // The line before it ended, so it stays whole.
    assert!(transcript.text.starts_with("first\nzzz"));
}

#[test]
fn a_full_screen_program_leaves_only_the_normal_screen() {
    let transcript = run(
        40,
        10,
        b"$ before\r\n\x1b[?1049h\x1b[H\x1b[2JALT CONTENT\x1b[5;5Hmore\x1b[?1049lafter\r\n",
    );
    assert_eq!(transcript.text, format!("$ before\nafter\n{FULL_SCREEN}"));
    assert_eq!(transcript.lines, 2);
    // Modes 47 and 1047 too, which the backend doesn't know itself.
    for mode in ["47", "1047"] {
        let output = format!("x\r\n\x1b[?{mode}hALT\x1b[?{mode}ly\r\n");
        let transcript = run(40, 10, output.as_bytes());
        assert!(
            !transcript.text.contains("ALT"),
            "{mode}: {:?}",
            transcript.text
        );
        assert!(transcript.text.ends_with(FULL_SCREEN), "{mode}");
    }
    // A program that never left it.
    let transcript = run(40, 10, b"x\r\n\x1b[?1049hALT");
    assert_eq!(transcript.text, format!("x\n{FULL_SCREEN}"));
    // A program that only drew on the alternate screen.
    let transcript = run(40, 10, b"\x1b[?1049hALT\x1b[?1049l");
    assert_eq!(transcript.text, FULL_SCREEN);
    assert_eq!(transcript.lines, 0);
}

#[test]
fn an_escape_split_across_reads_still_counts() {
    let mut emulator = emulator(40, 10);
    emulator.feed(b"x\r\n\x1b[?10");
    emulator.feed(b"49hALT\x1b[");
    emulator.feed(b"?1049l\x1b[3");
    emulator.feed(b"1my\x1b[0m");
    let transcript = emulator.transcript();
    assert_eq!(
        transcript.text,
        format!("x\n\x1b[0;31my\x1b[0m\n{FULL_SCREEN}")
    );
}

#[test]
fn a_note_follows_the_prompt() {
    let mut emulator = emulator(40, 10);
    emulator.feed(b"[sudo] password for dev:");
    emulator.note(SECRET_SENT);
    emulator.feed(b"\r\nok\r\n");
    assert_eq!(
        emulator.transcript().text,
        "[sudo] password for dev: 🔒 sent\nok"
    );
    // After a space, the note's own space goes.
    let mut emulator = emulator_after(b"Token: ");
    emulator.note(SECRET_SENT);
    assert_eq!(emulator.transcript().text, "Token: 🔒 sent");
    let mut emulator = emulator_after(b"");
    emulator.note(SECRET_SENT);
    assert_eq!(emulator.transcript().text, "🔒 sent");
}

fn emulator_after(output: &[u8]) -> Box<dyn Emulator> {
    let mut emulator = emulator(40, 10);
    emulator.feed(output);
    emulator
}

#[test]
fn wide_characters_show_once() {
    assert_eq!(text("漢字 ok 🦀!\r\n".as_bytes()), "漢字 ok 🦀!");
    // One that wraps at the edge leaves no gap behind.
    let transcript = run(5, 4, "abcd漢字\r\n".as_bytes());
    assert_eq!(transcript.text, "abcd漢字");
    // Combining marks stay with their letter.
    assert_eq!(text("e\u{301}\r\n".as_bytes()), "e\u{301}");
}

#[test]
fn a_resize_keeps_the_text() {
    let mut emulator = emulator(10, 5);
    emulator.feed(b"0123456789abc\r\nnext\r\n");
    emulator.resize(40, 5);
    emulator.feed(b"last\r\n");
    let transcript = emulator.transcript();
    assert_eq!(transcript.text, "0123456789abc\nnext\nlast");
    emulator.resize(0, 0);
    emulator.feed(b"tiny\r\n");
}

#[test]
fn tabs_keep_their_columns() {
    assert_eq!(text(b"a\tb\r\n"), "a       b");
}
