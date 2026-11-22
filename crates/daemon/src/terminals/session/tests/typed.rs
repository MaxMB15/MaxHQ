// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Lines you type by hand at a session's prompt, a line you send as a
//! secret, and what an older client answers for the terminal.

use maxhq_protocol::terminal::SessionBlock;

use super::*;
use crate::terminals::InputFrom;

/// A value that must never reach a note.
const TOKEN: &str = "abc123-not-for-notes";

impl Fixture {
    /// Types `keys` into the terminal, as you do in the panel.
    fn type_in(&self, terminal: TerminalId, keys: &str) {
        self.terminals
            .input(1, Caller::App, terminal, keys.as_bytes().to_vec())
            .expect("you may type");
    }

    /// Waits for the block a line you typed made, and for it to end.
    async fn typed(&mut self, terminal: TerminalId) -> (SessionBlock, Ran) {
        loop {
            if let RunNote::Typed {
                terminal: said,
                block,
                ..
            } = self.note().await
            {
                assert_eq!(said, terminal);
                let ran = self.ended(terminal, block.id).await;
                return (block, ran);
            }
        }
    }

    /// Waits until the session's shell reads a line again.
    async fn at_prompt(&self, terminal: TerminalId) {
        self.until(terminal, |open| {
            open.is_some_and(|open| {
                !open.keys.sent
                    && !open.keys.line
                    && open.block.is_none()
                    && (open.note.session.as_ref()).is_some_and(|session| session.current.is_none())
            })
        })
        .await;
    }
}

async fn typed_lines_become_private_blocks(name: &str) {
    let Some(mut f) = fixture_on_path(name) else {
        return;
    };
    let ran = f.block("true").await;
    let terminal = ran.terminal;
    f.at_prompt(terminal).await;
    let mut said = Vec::new();

    f.type_in(terminal, &format!("export DEBUG=cues GH_TOKEN={TOKEN}\r"));
    let (block, ran) = f.typed(terminal).await;
    said.push(format!("{block:?} {ran:?}"));
    assert_eq!(block.command, "export DEBUG=cues GH_TOKEN=•••", "{block:?}");
    assert!(block.private && block.typed, "{block:?}");
    assert_eq!(block.id, 2);
    assert_eq!(block.cwd, f.shown(&f.work));
    assert!(block.start.is_some());
    assert_eq!(ran.exit, Some(0), "{ran:?}");

    f.at_prompt(terminal).await;
    f.type_in(terminal, "ls\r");
    let (block, ran) = f.typed(terminal).await;
    said.push(format!("{block:?} {ran:?}"));
    assert_eq!((block.id, block.command.as_str()), (3, "ls"));
    assert!(ran.output.text.contains("sub"), "{ran:?}");
    assert!(!ran.output.text.contains("ls"), "the line isn't output");

    // A line that starts with a space keeps out of the history, and out of
    // the card.
    f.at_prompt(terminal).await;
    f.type_in(terminal, " echo hidden-$((1+1))\r");
    shows(&f.terminals.inner.holds, terminal, "hidden-2").await;
    f.at_prompt(terminal).await;
    let key = f.run("echo $DEBUG");
    loop {
        let note = f.note().await;
        assert!(!matches!(note, RunNote::Typed { .. }), "{note:?}");
        said.push(format!("{note:?}"));
        if let RunNote::Session {
            key: its, result, ..
        } = note
            && its == key
        {
            let SessionResult::Wrote { block, .. } = result else {
                panic!("echo didn't go into the session: {result:?}");
            };
            assert_eq!(block, 4, "the hidden line made no block");
            let ran = f.ended(terminal, block).await;
            assert_eq!(ran.output.text.trim(), "cues", "{ran:?}");
            break;
        }
    }

    let kept = f
        .terminals
        .inner
        .holds
        .load_note::<serde_json::Value>(terminal);
    said.push(format!("{kept:?}"));
    for said in &said {
        assert!(!said.contains(TOKEN), "a note has the token: {said}");
        assert!(
            !said.contains("hidden-"),
            "a note has the hidden line: {said}"
        );
    }
    f.end().await;
}

#[tokio::test]
async fn lines_typed_in_bash_become_private_blocks() {
    typed_lines_become_private_blocks("bash").await;
}

#[tokio::test]
async fn lines_typed_in_zsh_become_private_blocks() {
    typed_lines_become_private_blocks("zsh").await;
}

#[tokio::test]
async fn a_line_typed_while_a_block_runs_is_its_own_block() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    // A line typed ahead runs at the next prompt, after the block that ran.
    let key = f.run("sleep 1");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("sleep didn't go into the session");
    };
    f.started(block).await;
    f.type_in(terminal, "echo ahead-$((2+3))\r");
    // The line's block can start before the one it waited for heard its
    // end.
    let (mut slept, mut typed, mut ahead) = (None, None, None);
    while slept.is_none() || ahead.is_none() {
        match f.note().await {
            RunNote::Typed { block, .. } => typed = Some(block),
            RunNote::Block {
                block: id,
                change: BlockChange::Ended { output, .. },
                ..
            } if id == block => slept = Some(output),
            RunNote::Block {
                block: id,
                change: BlockChange::Ended { output, .. },
                ..
            } if typed.as_ref().is_some_and(|typed| typed.id == id) => ahead = Some(output),
            _ => {}
        }
    }
    let typed = typed.expect("the line made a block");
    assert_eq!(
        (typed.id, typed.command.as_str()),
        (2, "echo ahead-$((2+3))")
    );
    let (slept, ahead) = (slept.expect("it ended"), ahead.expect("it ended"));
    assert!(!slept.text.contains("ahead-5"), "{slept:?}");
    assert_eq!(ahead.text.trim(), "ahead-5", "{ahead:?}");
    f.end().await;
}

#[tokio::test]
async fn a_mark_seen_before_or_from_before_this_maxd_makes_no_block() {
    let mut f = fixture("/bin/sh");
    let terminal = TerminalId::generate();
    let mut open = session_open(1, true);
    let line = Mark::Command {
        cwd: None,
        command: Some("ls".into()),
    };
    // A mark from before this maxd followed the session.
    assert!(!(f.terminals).typed_mark(terminal, &mut open, (10, Timestamp(1)), &line));
    let now = Timestamp::now();
    // The block `maxd` sent: its own start mark is no line you typed.
    if let Some(session) = open.note.session.as_mut() {
        session.current = Some(note(5, None));
    }
    open.sent_at = Some(5);
    assert!(f
        .terminals
        .typed_mark(terminal, &mut open, (20, now), &line));
    assert_eq!(open.sent_at, None);
    // The link sends it again as it comes up.
    assert!(!(f.terminals).typed_mark(terminal, &mut open, (20, now), &line));
    assert!(!(f.terminals).typed_mark(terminal, &mut open, (15, now), &line));
    assert!(f.notes.try_recv().is_err(), "no block yet");

    // A line typed ahead starts after the block's own start, even before
    // this maxd heard the block end.
    assert!(f
        .terminals
        .typed_mark(terminal, &mut open, (30, now), &line));
    let RunNote::Typed { block, .. } = f.note().await else {
        panic!("no block for the line");
    };
    assert_eq!((block.id, block.command.as_str()), (2, "ls"));
    assert!(block.private && block.typed);
    assert_eq!(block.start, Some(30));
    let session = open.note.session.as_ref().expect("a session");
    assert_eq!(session.typed_to, Some(30));
    assert!(session.current.as_ref().is_some_and(|block| block.typed));

    // A line no shell could say makes none.
    if let Some(session) = open.note.session.as_mut() {
        session.current = None;
    }
    let unsaid = Mark::Command {
        cwd: None,
        command: None,
    };
    assert!(f
        .terminals
        .typed_mark(terminal, &mut open, (40, now), &unsaid));
    while let Ok(note) = f.notes.try_recv() {
        assert!(!matches!(note, RunNote::Typed { .. }), "{note:?}");
    }

    // A command the shell never ran, as one it waited for more of, leaves
    // the next line you type a block of its own.
    open.sent_at = Some(50);
    assert!(!(f.terminals).typed_mark(terminal, &mut open, (45, now), &Mark::Prompt));
    assert_eq!(open.sent_at, Some(50));
    assert!(!(f.terminals).typed_mark(terminal, &mut open, (60, now), &Mark::Prompt));
    assert_eq!(open.sent_at, None);
    assert!(f
        .terminals
        .typed_mark(terminal, &mut open, (70, now), &line));
    loop {
        if let RunNote::Typed { block, .. } = f.note().await {
            assert_eq!((block.id, block.start), (3, Some(70)));
            break;
        }
    }
}

#[tokio::test]
async fn a_line_sent_as_a_secret_notes_that_it_went() {
    const CANARY: &str = "canary-58c2-not-for-notes";
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    // A prompt that shows what you type: the lock on the reply line is all
    // that says it's a secret.
    let key = f.run("IFS= read -r -p 'Token: ' tok; echo got ${#tok}");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("read didn't go into the session");
    };
    f.started(block).await;
    shows(&f.terminals.inner.holds, terminal, "Token: ").await;
    let secret = InputFrom {
        secret: true,
        answers_too: false,
    };
    (f.terminals)
        .input_from(
            1,
            Caller::App,
            terminal,
            format!("{CANARY}\r").into(),
            secret,
        )
        .expect("you may type");
    let ran = f.ended(terminal, block).await;
    assert_eq!(ran.secret_prompts, 1, "{ran:?}");
    assert!(ran.output.text.contains("🔒 sent"), "{ran:?}");
    let got = format!("got {}", CANARY.len());
    assert!(ran.output.text.contains(&got), "{ran:?}");
    assert!(!ran.output.text.contains(CANARY), "{ran:?}");
    let kept = f
        .terminals
        .inner
        .holds
        .load_note::<serde_json::Value>(terminal);
    assert!(!format!("{kept:?}").contains(CANARY));
    f.end().await;
}

#[tokio::test]
async fn an_older_clients_answers_go_and_a_newer_ones_keys_stay() {
    let Some(mut f) = fixture_on_path("bash") else {
        return;
    };
    let key = f.run("read -r a; read -r b; printf '%s:%s\\n' \"${#a}\" \"${#b}\"");
    let SessionResult::Wrote {
        terminal, block, ..
    } = f.answer(key).await
    else {
        panic!("read didn't go into the session");
    };
    f.started(block).await;
    f.until(terminal, |open| {
        open.is_some_and(|open| open.entity.answers)
    })
    .await;
    // A client from before protocol 59 answers where its cursor is, as the
    // holder does. Its answer goes and its keys stay.
    let older = InputFrom {
        secret: false,
        answers_too: true,
    };
    (f.terminals)
        .input_from(
            2,
            Caller::App,
            terminal,
            b"\x1b[12;40Rold\r".to_vec(),
            older,
        )
        .expect("you may type");
    // All answers and nothing else: nothing goes.
    (f.terminals)
        .input_from(2, Caller::App, terminal, b"\x1b[?62;22c".to_vec(), older)
        .expect("you may type");
    // A newer one answers nothing, so what it sends is all keys.
    (f.terminals)
        .input_from(
            1,
            Caller::App,
            terminal,
            b"\x1b[5;5Rnew\r".to_vec(),
            InputFrom::default(),
        )
        .expect("you may type");
    let ran = f.ended(terminal, block).await;
    assert!(ran.output.text.contains("3:9"), "{ran:?}");
    f.end().await;
}
