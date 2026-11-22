// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What you type into a terminal by hand, as far as `maxd` treats it
//! differently.
//!
//! A line you type at the prompt of an agent's session becomes a block of
//! its own in the agent's card, private until you share it. Its text is
//! what the shell's start mark carries, each value set for a name that
//! reads like a secret already masked by the holder. A line that starts
//! with a space, as it keeps out of the history, carries none, so it's no
//! block. Neither is a command in a nested shell or a full-screen program,
//! which marks nothing `maxd` takes.
//!
//! A line you send as a secret notes in the block that a password went,
//! never what it was. And a client from before the holder answered queries
//! answers them too, so `maxd` leaves its answers out.

use maxhq_hold::Mark;
use maxhq_protocol::terminal::SessionBlock;

use super::session::BlockNote;
use super::*;

/// How a client sent what it typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputFrom {
    /// You sent it as a secret, from a reply line you locked: the block
    /// notes that a password went, and never what it was.
    pub secret: bool,
    /// The client is from before the holder answered queries, so its view
    /// answers them too, and the program would read each answer twice.
    pub answers_too: bool,
}

/// `data` without the whole answers a terminal gives to where its cursor
/// is, its status and its kind: `ESC [ r ; c R`, `ESC [ ? … c`,
/// `ESC [ > … c` and `ESC [ 0 n`. The rest stays, in order. A key that
/// reads like one of them, such as Shift-F3 as `ESC [ 1 ; 2 R`, goes too.
pub(super) fn without_answers(data: &[u8]) -> Vec<u8> {
    let mut kept = Vec::with_capacity(data.len());
    let mut at = 0;
    while at < data.len() {
        match answer_len(&data[at..]) {
            Some(len) => at += len,
            None => {
                kept.push(data[at]);
                at += 1;
            }
        }
    }
    kept
}

/// The length of the answer `data` starts with, if it does.
fn answer_len(data: &[u8]) -> Option<usize> {
    let [0x1b, b'[', body @ ..] = data else {
        return None;
    };
    let (lead, params) = match body.first()? {
        lead @ (b'?' | b'>') => (Some(*lead), &body[1..]),
        _ => (None, body),
    };
    let end = params
        .iter()
        .position(|byte| !(byte.is_ascii_digit() || *byte == b';'))?;
    let (numbers, last) = (&params[..end], params[end]);
    let whole = 2 + usize::from(lead.is_some()) + end + 1;
    let answer = match (lead, last) {
        // Device attributes, primary or secondary.
        (Some(_), b'c') => true,
        // The cursor's place: a row and a column.
        (None, b'R') => {
            let mut parts = numbers.split(|byte| *byte == b';');
            let row = parts.next().is_some_and(|row| !row.is_empty());
            let col = parts.next().is_some_and(|col| !col.is_empty());
            row && col && parts.next().is_none()
        }
        // The status: all is well.
        (None, b'n') => numbers == b"0",
        _ => false,
    };
    answer.then_some(whole)
}

impl Terminals {
    /// Takes in a mark a session's shell made at `offset`, and makes the
    /// line you typed that a start mark carries a block of its own, which
    /// it follows. Returns whether the session's note changed.
    pub(super) fn typed_mark(
        &self,
        terminal: TerminalId,
        open: &mut Open,
        (offset, at): (u64, Timestamp),
        mark: &Mark,
    ) -> bool {
        let Mark::Command { cwd, command } = mark else {
            // A prompt with no start since `maxd` sent a command: the shell
            // never ran it, as for a line it waited for more of.
            if matches!(mark, Mark::Prompt) && open.sent_at.is_some_and(|sent| offset > sent) {
                open.sent_at = None;
            }
            return false;
        };
        let since = open.since;
        let Some(session) = open.note.session.as_mut() else {
            return false;
        };
        // The link sends each mark the holder keeps again as it comes up.
        // A mark from before this `maxd` followed is a command typed while
        // none did, whose block nothing could have cut.
        if at < since || session.typed_to.is_some_and(|to| offset <= to) {
            return false;
        }
        session.typed_to = Some(offset);
        // The first start since `maxd` sent a command is that command's,
        // whether or not its block heard it yet. A later one is a line you
        // typed, ahead or at the prompt, whether or not that block ended.
        if open.sent_at.take().is_some() {
            return true;
        }
        let Some(line) = command.clone() else {
            return true;
        };
        let cwd = cwd
            .as_deref()
            .map_or_else(|| open.entity.cwd.clone(), |cwd| shown(Path::new(cwd)));
        session.blocks += 1;
        let block = BlockNote {
            id: session.blocks,
            command: line,
            after: offset,
            written_at: at,
            start: Some(offset),
            cwd,
            done: false,
            exit: None,
            typed: true,
        };
        session.current = Some(block.clone());
        let (secrets, secrets_rx) = mpsc::unbounded_channel();
        open.block = Some((block.id, secrets));
        open.stopped_at = None;
        let agent = open.note.agent;
        // The manager hears of the block before anything about it.
        self.note(RunNote::Typed {
            agent,
            terminal,
            block: SessionBlock {
                id: block.id,
                command: block.command.clone(),
                cwd: block.cwd.clone(),
                cwd_after: None,
                private: true,
                typed: true,
                process: TerminalStatus::Running,
                started_at: at,
                duration_ms: None,
                lines: 0,
                output: String::new(),
                output_truncated: false,
                secret_prompts: 0,
                start: Some(offset),
            },
        });
        let (cols, rows) = (open.entity.cols, open.entity.rows);
        tokio::spawn(
            self.clone()
                .follow_block(terminal, agent, block, secrets_rx, cols, rows),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_answers_go_and_keys_stay() {
        for (sent, kept) in [
            (&b"\x1b[12;40R"[..], &b""[..]),
            (b"\x1b[?62;22c", b""),
            (b"\x1b[>0;95;0c", b""),
            (b"\x1b[0n", b""),
            (b"y\x1b[3;1R\r", b"y\r"),
            (b"\x1b[A\x1b[B\r", b"\x1b[A\x1b[B\r"),
            (b"\x1bOA", b"\x1bOA"),
            (b"\x1b[5n", b"\x1b[5n"),
            (b"\x1b[12R", b"\x1b[12R"),
            (b"\x1b[c", b"\x1b[c"),
            (b"\x1b[200~paste\x1b[201~", b"\x1b[200~paste\x1b[201~"),
            (b"\x1b[12;4", b"\x1b[12;4"),
            (b"hunter2\r", b"hunter2\r"),
            (b"", b""),
        ] {
            assert_eq!(
                without_answers(sent),
                kept,
                "{:?}",
                String::from_utf8_lossy(sent)
            );
        }
    }
}
