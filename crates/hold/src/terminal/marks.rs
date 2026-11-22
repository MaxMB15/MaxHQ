// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The marks a shell prints around its prompt and commands, OSC 133, as
//! the hooks in `shell.rs` print them: `A` where the prompt starts, `B`
//! where what you type starts, `C` where the command starts and `D;<exit>`
//! where it ended. Each carries the terminal's nonce as `maxhq=<nonce>`.
//! A `C` may carry the command line as `C;<line>`, percent-encoded so it
//! holds no `;`, `%`, control or non-ASCII byte. The holder decodes it,
//! drops any control character but the line break, cuts it at
//! [`MAX_COMMAND`] characters and hides what it sets for a name that reads
//! like a secret, so the line it keeps is safe to show. A line with a
//! leading space, as the shell history leaves out, comes with none.
//!
//! The holder reads them out of the output before it keeps it. A mark with
//! the right nonce goes on in the standard form, without the nonce, and
//! its place is noted. Any other 133 goes nowhere, so a file you print or
//! a program you run can't make marks, and neither can a shell's own
//! integration. The filter reads the bytes the way the drawer's terminal
//! does, so whatever that would take for a 133 passes here first: `ESC ]`
//! or the C1 control U+009D, a number with leading zeros or C0 controls
//! in it, and an end of BEL, `ESC` or U+009C. Other output passes as it
//! came, though the bytes of a sequence that may still be a 133 wait for
//! the next read.

/// The longest 133 body kept: past it, the sequence can't be a mark and
/// goes nowhere through its end. A `C` with its line takes the most: up to
/// 501 characters of four bytes each, each byte as `%XX`.
const MAX_BODY: usize = 8 * 1024;

/// The most characters of a command line a `C` mark keeps. A longer line
/// keeps one less and ends in `…`.
pub const MAX_COMMAND: usize = 500;

/// The most digits read after an OSC introducer, leading zeros and all.
/// Past it, a number that still reads as 133 or less goes nowhere.
const MAX_ID: usize = 32;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
/// The first byte of a C1 control in UTF-8, U+0080 to U+009F.
const C1_LEAD: u8 = 0xc2;
/// The C1 control that starts an OSC, after [`C1_LEAD`].
const OSC: u8 = 0x9d;
/// The C1 control that ends a string, after [`C1_LEAD`].
const ST: u8 = 0x9c;

/// What a shell marked.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mark {
    /// Its prompt starts.
    Prompt,
    /// Its prompt ends, and what you type starts.
    Input,
    /// The command you typed starts.
    Command {
        /// The folder the shell said last before it, which is where the
        /// command starts.
        cwd: Option<String>,
        /// The command line as the shell said it, safe to show: at most
        /// [`MAX_COMMAND`] characters, no control character but the line
        /// break, and `•••` for a value set for a name that reads like a
        /// secret. None when the shell didn't say, as for a line with a
        /// leading space, and from a holder before it said any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// The command ended.
    Done {
        /// Its exit code, when the shell said it.
        exit: Option<i32>,
    },
}

/// A mark as it reads in the output, before the holder adds the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Prompt,
    Input,
    /// With the command line, safe to show, when the shell said it.
    Command(Option<String>),
    Done(Option<i32>),
}

impl Kind {
    /// The mark, with `cwd` for a command.
    pub fn mark(self, cwd: Option<&str>) -> Mark {
        match self {
            Self::Prompt => Mark::Prompt,
            Self::Input => Mark::Input,
            Self::Command(command) => Mark::Command {
                cwd: cwd.map(str::to_owned),
                command,
            },
            Self::Done(exit) => Mark::Done { exit },
        }
    }

    /// The mark in the standard form, without the nonce or the line.
    fn standard(&self) -> Vec<u8> {
        match self {
            Self::Prompt => b"\x1b]133;A\x07".to_vec(),
            Self::Input => b"\x1b]133;B\x07".to_vec(),
            Self::Command(_) => b"\x1b]133;C\x07".to_vec(),
            Self::Done(None) => b"\x1b]133;D\x07".to_vec(),
            Self::Done(Some(exit)) => format!("\x1b]133;D;{exit}\x07").into_bytes(),
        }
    }
}

/// One step of the output: a byte, or a C1 control written in UTF-8.
#[derive(Clone, Copy)]
enum Unit {
    Byte(u8),
    C1(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    /// After an ESC, and any C0 controls since, which the terminal runs.
    Escape,
    /// After an OSC introducer, in its number.
    Id {
        id: u32,
        digits: usize,
    },
    /// In a 133's body.
    Body,
    /// In a sequence that goes nowhere, through its end.
    Discard,
}

/// Reads the marks out of a terminal's output, across reads that split
/// them anywhere.
pub(crate) struct Filter {
    /// What a mark must end with, `maxhq=<nonce>`. Without a nonce, no 133
    /// goes on.
    tag: Option<Vec<u8>>,
    state: State,
    /// The bytes of the sequence that may still be a 133, as they came.
    held: Vec<u8>,
    /// The C0 controls after an ESC, which go out whatever it turns into.
    controls: Vec<u8>,
    body: Vec<u8>,
    /// The last read ended on what may be the first byte of a C1 control.
    lead: bool,
}

impl Filter {
    pub fn new(nonce: Option<&str>) -> Self {
        Self {
            tag: nonce.map(|nonce| format!("maxhq={nonce}").into_bytes()),
            state: State::Text,
            held: Vec::new(),
            controls: Vec::new(),
            body: Vec::new(),
            lead: false,
        }
    }

    /// Passes `bytes` on into `out`, and notes in `marks` each mark that
    /// went on, with where it starts in `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>, marks: &mut Vec<(usize, Kind)>) {
        // Most output has nothing to look at.
        if self.state == State::Text
            && !self.lead
            && !bytes.iter().any(|&b| b == ESC || b == C1_LEAD)
        {
            out.extend_from_slice(bytes);
            return;
        }
        for &byte in bytes {
            if self.lead {
                self.lead = false;
                if (0x80..=0x9f).contains(&byte) {
                    self.step(Unit::C1(byte), out, marks);
                    continue;
                }
                self.step(Unit::Byte(C1_LEAD), out, marks);
            }
            if byte == C1_LEAD {
                self.lead = true;
            } else {
                self.step(Unit::Byte(byte), out, marks);
            }
        }
    }

    fn step(&mut self, unit: Unit, out: &mut Vec<u8>, marks: &mut Vec<(usize, Kind)>) {
        // U+009D starts an OSC whatever came before.
        if let Unit::C1(OSC) = unit {
            self.give_up(out);
            self.held.extend_from_slice(&[C1_LEAD, OSC]);
            self.state = State::Id { id: 0, digits: 0 };
            return;
        }
        self.state = match (self.state, unit) {
            (State::Text, Unit::Byte(ESC)) => {
                self.held.push(ESC);
                State::Escape
            }
            (State::Text, unit) => {
                put(out, unit);
                State::Text
            }
            (State::Escape, Unit::Byte(b']')) => {
                self.held.push(b']');
                State::Id { id: 0, digits: 0 }
            }
            (State::Escape | State::Id { .. }, Unit::Byte(ESC)) if !self.holds_133() => {
                self.flush(out);
                self.held.push(ESC);
                State::Escape
            }
            // The terminal runs these and goes on with the escape, but for
            // CAN and SUB, which end it.
            (State::Escape, Unit::Byte(byte))
                if (byte < 0x20 && byte != CAN && byte != SUB) || byte == 0x7f =>
            {
                self.held.push(byte);
                self.controls.push(byte);
                State::Escape
            }
            (State::Escape, unit) => {
                self.flush(out);
                put(out, unit);
                State::Text
            }
            (State::Id { id, digits }, Unit::Byte(byte @ b'0'..=b'9')) => {
                self.held.push(byte);
                let id = id.saturating_mul(10).saturating_add(u32::from(byte - b'0'));
                if id > 133 {
                    // Not a 133, whatever follows.
                    self.flush(out);
                    State::Text
                } else if digits >= MAX_ID {
                    self.drop_held(out);
                    State::Discard
                } else {
                    State::Id {
                        id,
                        digits: digits + 1,
                    }
                }
            }
            (State::Id { id: 133, .. }, Unit::Byte(b';')) => {
                self.body.clear();
                State::Body
            }
            (state @ State::Id { .. }, Unit::Byte(byte)) if skipped(byte) => {
                self.held.push(byte);
                state
            }
            (State::Id { id: 133, .. }, unit) if ends(unit).is_some() => {
                // A 133 with nothing after its number.
                self.drop_held(out);
                self.after(unit, out)
            }
            (State::Id { .. }, unit) => {
                self.flush(out);
                put(out, unit);
                State::Text
            }
            (State::Body, unit) if ends(unit) == Some(true) => {
                match self.check() {
                    Some(kind) => {
                        self.held.clear();
                        out.append(&mut self.controls);
                        let standard = kind.standard();
                        marks.push((out.len(), kind));
                        out.extend_from_slice(&standard);
                    }
                    None => self.drop_held(out),
                }
                self.after(unit, out)
            }
            (State::Body, unit) if ends(unit) == Some(false) => {
                self.drop_held(out);
                self.after(unit, out)
            }
            (State::Body, Unit::Byte(byte)) if skipped(byte) => State::Body,
            (State::Body, Unit::Byte(byte)) if self.body.len() < MAX_BODY => {
                self.body.push(byte);
                State::Body
            }
            (State::Body, _) => {
                self.drop_held(out);
                State::Discard
            }
            (State::Discard, unit) if ends(unit).is_some() => self.after(unit, out),
            (State::Discard, _) => State::Discard,
        };
    }

    /// Whether what's held is an OSC whose number reads 133 so far.
    fn holds_133(&self) -> bool {
        matches!(self.state, State::Id { id: 133, .. })
    }

    /// Where a 133 that `unit` ended, and that went nowhere or went on in
    /// the standard form, leaves the output: an ESC starts the next
    /// sequence, a BEL or U+009C was its end, and CAN, SUB or another C1
    /// control goes out.
    fn after(&mut self, unit: Unit, out: &mut Vec<u8>) -> State {
        match unit {
            Unit::Byte(ESC) => {
                self.held.push(ESC);
                State::Escape
            }
            Unit::Byte(BEL) | Unit::C1(ST) => State::Text,
            unit => {
                put(out, unit);
                State::Text
            }
        }
    }

    /// The mark a 133's body makes, when it ends with the nonce.
    fn check(&self) -> Option<Kind> {
        let tag = self.tag.as_deref()?;
        let parts: Vec<&[u8]> = self.body.split(|&b| b == b';').collect();
        let (last, parts) = parts.split_last()?;
        if *last != tag {
            return None;
        }
        match parts {
            [b"A"] => Some(Kind::Prompt),
            [b"B"] => Some(Kind::Input),
            [b"C"] => Some(Kind::Command(None)),
            [b"C", line] => Some(Kind::Command(command_line(line))),
            [b"D"] => Some(Kind::Done(None)),
            [b"D", exit] => std::str::from_utf8(exit)
                .ok()?
                .parse()
                .ok()
                .map(|exit| Kind::Done(Some(exit))),
            _ => None,
        }
    }

    /// Lets out what was held as it came: it's no 133.
    fn flush(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.held);
        self.controls.clear();
    }

    /// Drops what was held but the C0 controls after its ESC, which the
    /// terminal would have run.
    fn drop_held(&mut self, out: &mut Vec<u8>) {
        self.held.clear();
        out.append(&mut self.controls);
    }

    /// Ends what was held as U+009D starts a new OSC over it. The terminal
    /// forgets an ESC then, and an OSC it was reading.
    fn give_up(&mut self, out: &mut Vec<u8>) {
        match self.state {
            State::Text => {}
            State::Id { id, .. } if id != 133 => self.flush(out),
            State::Escape | State::Id { .. } | State::Body | State::Discard => {
                self.drop_held(out);
            }
        }
    }
}

/// The command line a `C` mark carries, percent-encoded, as it's safe to
/// show: decoded, with no control character but the line break, a tab as
/// a space, no character that turns text around or hides, cut at
/// [`MAX_COMMAND`] characters and with what it sets for a name that reads
/// like a secret hidden. None when nothing's left.
fn command_line(encoded: &[u8]) -> Option<String> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut at = 0;
    while let Some(&byte) = encoded.get(at) {
        let hex = |at: usize| {
            encoded
                .get(at)
                .and_then(|digit| (*digit as char).to_digit(16))
        };
        if byte == b'%'
            && let (Some(high), Some(low)) = (hex(at + 1), hex(at + 2))
        {
            // Two hex digits make at most 0xff.
            bytes.push((high * 16 + low) as u8);
            at += 3;
        } else {
            bytes.push(byte);
            at += 1;
        }
    }
    let decoded = String::from_utf8_lossy(&bytes);
    let mut line = String::with_capacity(decoded.len());
    let mut count = 0;
    let mut longer = false;
    for c in decoded.chars() {
        let c = match c {
            '\t' => ' ',
            '\n' => '\n',
            '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => {
                continue;
            }
            c if c.is_control() => continue,
            c => c,
        };
        if count == MAX_COMMAND {
            longer = true;
            break;
        }
        line.push(c);
        count += 1;
    }
    let mut line = line.trim().to_owned();
    if longer {
        line.pop();
        line.push('…');
    }
    (!line.is_empty()).then(|| maxhq_term::mask_secrets(&line))
}

/// Whether the terminal skips `byte` inside an OSC: a C0 control but for
/// BEL, CAN, SUB and ESC, which end it.
fn skipped(byte: u8) -> bool {
    byte < 0x20 && !matches!(byte, BEL | CAN | SUB | ESC)
}

/// How `unit` ends an OSC, if it does: `Some(true)` when the terminal
/// takes the OSC (BEL, ESC or U+009C), `Some(false)` when it drops it
/// (CAN, SUB or another C1 control).
fn ends(unit: Unit) -> Option<bool> {
    match unit {
        Unit::Byte(BEL | ESC) | Unit::C1(ST) => Some(true),
        Unit::Byte(CAN | SUB) | Unit::C1(_) => Some(false),
        Unit::Byte(_) => None,
    }
}

fn put(out: &mut Vec<u8>, unit: Unit) {
    match unit {
        Unit::Byte(byte) => out.push(byte),
        Unit::C1(code) => out.extend_from_slice(&[C1_LEAD, code]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "0123abcd";

    /// What comes out of `chunks`, and the marks with where they start.
    fn filter(chunks: &[&[u8]]) -> (Vec<u8>, Vec<(usize, Kind)>) {
        let mut filter = Filter::new(Some(NONCE));
        let (mut out, mut marks) = (Vec::new(), Vec::new());
        for chunk in chunks {
            filter.feed(chunk, &mut out, &mut marks);
        }
        (out, marks)
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn marks_with_the_nonce_go_on_without_it() {
        let input = b"\x1b]133;A;maxhq=0123abcd\x07$ \x1b]133;B;maxhq=0123abcd\x07ls\r\n\
            \x1b]133;C;maxhq=0123abcd\x1b\\a b\r\n\x1b]133;D;2;maxhq=0123abcd\xc2\x9c";
        let (out, marks) = filter(&[input]);
        assert_eq!(
            text(&out),
            "\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b\\a b\r\n\x1b]133;D;2\x07"
        );
        let kinds: Vec<_> = marks.iter().map(|(_, kind)| kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                Kind::Prompt,
                Kind::Input,
                Kind::Command(None),
                Kind::Done(Some(2))
            ]
        );
        for (at, kind) in &marks {
            assert_eq!(&out[*at..*at + kind.standard().len()], kind.standard());
        }
    }

    #[test]
    fn other_133s_go_nowhere() {
        for bad in [
            &b"\x1b]133;A;maxhq=ffff\x07"[..],
            b"\x1b]133;A\x07",
            b"\x1b]133;A;click_events=1\x07",
            b"\x1b]133;D;x;maxhq=0123abcd\x07",
            b"\x1b]133;Z;maxhq=0123abcd\x07",
            b"\x1b]133\x07",
            b"\x1b]0133;A;maxhq=nope\x07",
            b"\x1b]1\x0233;A\x07",
            b"\xc2\x9d133;A\xc2\x9c",
        ] {
            assert_eq!(text(&filter(&[b"<", bad, b">"]).0), "<>", "{}", text(bad));
        }
        // A terminal with no nonce lets no mark on.
        let mut filter = Filter::new(None);
        let (mut out, mut marks) = (Vec::new(), Vec::new());
        filter.feed(b"\x1b]133;A;maxhq=\x07x", &mut out, &mut marks);
        assert_eq!((text(&out), marks), ("x".into(), Vec::new()));
    }

    #[test]
    fn a_mark_split_anywhere_still_counts() {
        let whole: &[u8] =
            b"ab\x1b]133;D;130;maxhq=0123abcd\x07cd\xc2\x9d133;B;maxhq=0123abcd\xc2\x9c";
        let (want, want_marks) = filter(&[whole]);
        assert_eq!(text(&want), "ab\x1b]133;D;130\x07cd\x1b]133;B\x07");
        for at in 0..whole.len() {
            let (head, tail) = whole.split_at(at);
            assert_eq!(
                filter(&[head, tail]),
                (want.clone(), want_marks.clone()),
                "{at}"
            );
        }
        let bytes: Vec<&[u8]> = whole.chunks(1).collect();
        assert_eq!(filter(&bytes), (want, want_marks));
    }

    #[test]
    fn everything_else_passes_as_it_came() {
        for fine in [
            &b"plain text, caf\xc3\xa9, 25\xc2\xb0C"[..],
            b"\x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b]7;file://devbox/tmp\x1b\\",
            b"\x1b]1337;SetMark\x07 \x1b]8;;https://example.com\x07link\x1b]8;;\x07",
            b"\x1b]13;x\x07 \x1b]1330;x\x07 \x1b]134;x\x07 \x1b]2\x1b]0;t\x07",
            b"\xc2\x9d0;title\xc2\x9c",
            b"\x1b\x1b[1m \x1bP1$r0m\x1b\\ \x1b\x07x",
        ] {
            assert_eq!(text(&filter(&[fine]).0), text(fine));
        }
    }

    #[test]
    fn controls_and_odd_ends_read_as_the_terminal_reads_them() {
        // A newline or a BEL between ESC and `]` still runs.
        let (out, marks) = filter(&[b"\x1b\n\x07]133;A;maxhq=0123abcd\x07"]);
        assert_eq!(text(&out), "\n\x07\x1b]133;A\x07");
        assert_eq!(marks, [(2, Kind::Prompt)]);
        assert_eq!(text(&filter(&[b"\x1b\x07]133;A\x07"]).0), "\x07");
        // Leading zeros and controls in the number count as the terminal
        // counts them.
        let (out, _) = filter(&[b"\x1b]01\x0133;C;maxhq=0123abcd\x07"]);
        assert_eq!(text(&out), "\x1b]133;C\x07");
        // CAN ends a mark without it counting.
        assert_eq!(
            text(&filter(&[b"\x1b]133;A;maxhq=0123abcd\x18x"]).0),
            "\x18x"
        );
        // U+009D starts over, dropping what came before.
        let (out, marks) = filter(&[b"\x1b]133;A;maxh\xc2\x9d133;B;maxhq=0123abcd\x07"]);
        assert_eq!(
            (text(&out), marks),
            ("\x1b]133;B\x07".into(), vec![(0, Kind::Input)])
        );
        // A body too long to be a mark goes nowhere through its end.
        let long = [&b"\x1b]133;A;"[..], &[b'x'; MAX_BODY * 2], b"\x07after"].concat();
        assert_eq!(text(&filter(&[&long]).0), "after");
        let zeros = [&b"\x1b]"[..], &[b'0'; MAX_ID * 2], b"133;A\x07after"].concat();
        assert_eq!(text(&filter(&[&zeros]).0), "after");
    }

    /// The line a `C` mark with `encoded` after it carries.
    fn line_of(encoded: &str) -> Option<String> {
        let mark = format!("\x1b]133;C;{encoded};maxhq={NONCE}\x07");
        let (out, marks) = filter(&[mark.as_bytes()]);
        assert_eq!(text(&out), "\x1b]133;C\x07", "the line never goes out");
        match marks.as_slice() {
            [(0, Kind::Command(line))] => line.clone(),
            other => panic!("expected one command mark, got {other:?}"),
        }
    }

    #[test]
    fn a_command_mark_carries_its_line_decoded() {
        assert_eq!(line_of("ls%20-la").as_deref(), Some("ls -la"));
        assert_eq!(
            line_of("echo%20%22a%3Bb%22%20%25%20caf%C3%A9").as_deref(),
            Some("echo \"a;b\" % café")
        );
        // A line break stays, a tab reads as a space, and other controls,
        // escapes and characters that turn text around go.
        assert_eq!(
            line_of("for%20f%20in%20*%0A%09do%20x%1B%5B31m%07%E2%80%AE%C2%9B").as_deref(),
            Some("for f in *\n do x[31m")
        );
        // A `%` that starts no code stays as it is.
        assert_eq!(line_of("printf%20%zz%2").as_deref(), Some("printf %zz%2"));
        // Nothing to say, or only spaces, says nothing.
        assert_eq!(line_of(""), None);
        assert_eq!(line_of("%20%20"), None);
        // What it sets for a name that reads like a secret hides.
        assert_eq!(
            line_of("export%20GH_TOKEN%3Dabc123").as_deref(),
            Some("export GH_TOKEN=•••")
        );
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis() {
        let at_cap = "x".repeat(MAX_COMMAND);
        assert_eq!(line_of(&at_cap), Some(at_cap.clone()));
        // A shell sends one more than the cap for a line that goes on.
        let over = "é".repeat(MAX_COMMAND + 1);
        let encoded = "%C3%A9".repeat(MAX_COMMAND + 1);
        let line = line_of(&encoded).unwrap();
        assert_eq!(line.chars().count(), MAX_COMMAND);
        assert!(line.ends_with("é…") && over.starts_with(line.trim_end_matches('…')));
        // The widest line a shell sends still fits a mark.
        let widest = "%F0%9F%98%80".repeat(MAX_COMMAND + 1);
        assert_eq!(line_of(&widest).unwrap().chars().count(), MAX_COMMAND);
    }

    #[test]
    fn a_line_cant_forge_a_mark_or_end_one() {
        // Without the nonce, a line goes nowhere, however it's written.
        for bad in [
            &b"\x1b]133;C;ls;maxhq=ffff\x07"[..],
            b"\x1b]133;C;ls\x07",
            b"\x1b]133;C;a;b;maxhq=0123abcd\x07",
        ] {
            let (out, marks) = filter(&[b"<", bad, b">"]);
            assert_eq!(
                (text(&out), marks),
                ("<>".into(), Vec::new()),
                "{}",
                text(bad)
            );
        }
        // An encoded `;maxhq=` in a line is part of the line, and the mark
        // still needs the nonce last.
        assert_eq!(
            line_of("x%3Bmaxhq=0123abcd").as_deref(),
            Some("x;maxhq=0123abcd")
        );
        // Raw controls in the body are skipped as the terminal skips them,
        // and an encoded ESC or BEL can't end the mark early.
        let (out, marks) = filter(&[b"\x1b]133;C;a\x01b%1B%5D133%3BA%07;maxhq=0123abcd\x07after"]);
        assert_eq!(text(&out), "\x1b]133;C\x07after");
        assert_eq!(marks, [(0, Kind::Command(Some("ab]133;A".into())))]);
        // A body past the most a mark holds goes nowhere.
        let long = format!("\x1b]133;C;{};maxhq={NONCE}\x07after", "x".repeat(MAX_BODY));
        assert_eq!(filter(&[long.as_bytes()]), (b"after".to_vec(), Vec::new()));
    }
}
