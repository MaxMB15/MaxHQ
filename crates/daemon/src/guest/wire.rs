// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's half of the link's wire: holder events turned into what the
//! link carries, and lists split to fit a message. The host's half, and
//! what both ends share, is in `guestlink/wire.rs`.

use maxhq_hold as hold;
use maxhq_protocol::guest::{
    ChildState, Ended, Framing, HolderExit, HolderHello, HolderLine, WriteOutcome,
    MAX_LINE_MESSAGE_LEN, MAX_MESSAGE_LEN,
};
use serde::Serialize;

/// What a message may take once a list is split to fit, leaving room for
/// the rest of the message around the list.
pub(crate) const CHUNK_BUDGET: usize = MAX_MESSAGE_LEN - 64 * 1024;

/// What one holder line may take in a message of its own, leaving room
/// for the rest of the message.
pub(crate) const LINE_BUDGET: usize = MAX_LINE_MESSAGE_LEN - 64 * 1024;

/// A holder line as the link carries it.
pub(crate) fn line_out(line: hold::Line) -> HolderLine {
    HolderLine {
        seq: line.seq,
        at: line.at,
        text: line.text,
        replayed: line.replayed,
    }
}

/// A holder exit as the link carries it.
pub(crate) fn exit_out(exit: hold::Exit) -> HolderExit {
    HolderExit {
        code: exit.code,
        signal: exit.signal,
        at: exit.at,
        stderr_tail: exit.stderr_tail,
    }
}

/// A holder hello as the link carries it, with no pending lines. They go
/// separately, since they may not fit in one message.
pub(crate) fn hello_out(hello: &hold::Hello) -> HolderHello {
    HolderHello {
        holder_pid: hello.holder_pid,
        child: match &hello.child {
            hold::ChildState::Running { pid } => ChildState::Running { pid: *pid },
            hold::ChildState::Exited(exit) => ChildState::Exited(exit_out(exit.clone())),
        },
        first_seq: hello.first_seq,
        last_seq: hello.last_seq,
        pending: Vec::new(),
    }
}

/// A write outcome as the link carries it.
pub(crate) fn outcome_out(outcome: hold::WriteOutcome) -> WriteOutcome {
    match outcome {
        hold::WriteOutcome::Written => WriteOutcome::Written,
        hold::WriteOutcome::AlreadyAnswered => WriteOutcome::AlreadyAnswered,
    }
}

/// An end's outcome as the link carries it.
pub(crate) fn ended_out(ended: hold::Ended) -> Ended {
    match ended {
        hold::Ended::Gone => Ended::Gone,
        hold::Ended::Unconfirmed { pid, reason } => Ended::Unconfirmed { pid, reason },
    }
}

/// A framing off the link.
pub(crate) fn framing_in(framing: Framing) -> hold::Framing {
    match framing {
        Framing::ClaudeStreamJson => hold::Framing::ClaudeStreamJson,
        Framing::JsonRpc => hold::Framing::JsonRpc,
    }
}

/// Splits `items` into runs whose JSON fits in [`CHUNK_BUDGET`], in order.
/// An item too big to share a run goes in one of its own when it fits
/// `alone`, and is otherwise left out, and `dropped` counts it.
pub(crate) fn chunks<T: Serialize>(
    items: Vec<T>,
    alone: usize,
    dropped: &mut usize,
) -> Vec<Vec<T>> {
    let mut runs = Vec::new();
    let mut run = Vec::new();
    let mut size = 0;
    for item in items {
        let len = serde_json::to_vec(&item).map_or(usize::MAX, |json| json.len() + 1);
        if len > CHUNK_BUDGET {
            if len > alone {
                *dropped += 1;
            } else {
                if !run.is_empty() {
                    runs.push(std::mem::take(&mut run));
                    size = 0;
                }
                runs.push(vec![item]);
            }
            continue;
        }
        if size + len > CHUNK_BUDGET {
            runs.push(std::mem::take(&mut run));
            size = 0;
        }
        size += len;
        run.push(item);
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_fit_the_message_limit() {
        let big = "x".repeat(1024 * 1024);
        let items = vec![big.clone(); 9];
        let mut dropped = 0;
        let runs = chunks(items, CHUNK_BUDGET, &mut dropped);
        assert_eq!(dropped, 0);
        assert_eq!(runs.iter().map(Vec::len).sum::<usize>(), 9);
        assert!(runs.len() >= 3);
        for run in &runs {
            assert!(serde_json::to_vec(run).unwrap().len() <= CHUNK_BUDGET);
        }

        let mut dropped = 0;
        let runs = chunks(
            vec!["x".repeat(MAX_MESSAGE_LEN), "small".into()],
            CHUNK_BUDGET,
            &mut dropped,
        );
        assert_eq!(dropped, 1);
        assert_eq!(runs, vec![vec!["small".to_owned()]]);

        // With room for a big item alone, it goes between the others.
        dropped = 0;
        let big = "x".repeat(MAX_MESSAGE_LEN);
        let runs = chunks(
            vec!["a".into(), big.clone(), "b".into()],
            LINE_BUDGET,
            &mut dropped,
        );
        assert_eq!(dropped, 0);
        assert_eq!(
            runs,
            vec![vec!["a".to_owned()], vec![big], vec!["b".to_owned()]]
        );
    }
}
