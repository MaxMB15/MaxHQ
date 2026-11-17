// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminals a link has attached. `maxd` sends each one's output in
//! frames, which the link hands to the [`TerminalSink`] the terminal was
//! attached with. The link notes where each terminal's output got to, and
//! a reconnect attaches them all again from there.
//!
//! What you type goes the other way, straight onto the connection, and
//! nothing here keeps it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::ClientMessage;

/// Where a link hands the output of the terminals it attached. The link
/// calls it as frames come in, so it must not block.
pub trait TerminalSink: Send + Sync + 'static {
    /// Output from a terminal, starting at byte `offset` of its stream.
    fn output(&self, terminal: TerminalId, offset: u64, bytes: &[u8]);

    /// Says what a terminal showed is gone, since `maxd` no longer holds
    /// where it ended. Its output starts over at byte `from`.
    fn reset(&self, terminal: TerminalId, from: u64);
}

/// One link's attached terminals.
#[derive(Default)]
pub(crate) struct Terminals {
    attached: Mutex<HashMap<TerminalId, Attached>>,
}

/// One attached terminal.
struct Attached {
    /// Where the output handed on so far ends, which a reattach goes on
    /// from. None until some came, unless the attach named a place.
    after: Option<u64>,
    sink: Arc<dyn TerminalSink>,
}

impl Terminals {
    /// Notes a terminal as attached from `after`, with the sink its output
    /// goes to.
    pub(crate) fn attach(
        &self,
        terminal: TerminalId,
        after: Option<u64>,
        sink: Arc<dyn TerminalSink>,
    ) {
        self.lock().insert(terminal, Attached { after, sink });
    }

    /// Forgets a terminal, so output still on its way goes nowhere. Says
    /// whether it was attached.
    pub(crate) fn detach(&self, terminal: TerminalId) -> bool {
        self.lock().remove(&terminal).is_some()
    }

    /// Hands output from a frame to the terminal's sink, if it's attached.
    pub(crate) fn output(&self, terminal: TerminalId, offset: u64, bytes: &[u8]) {
        let sink = {
            let mut attached = self.lock();
            let Some(one) = attached.get_mut(&terminal) else {
                return;
            };
            one.after = Some(offset + bytes.len() as u64);
            Arc::clone(&one.sink)
        };
        sink.output(terminal, offset, bytes);
    }

    /// Takes `maxd`'s answer to an attach. A reset goes to the sink, and
    /// the output then starts at `from`.
    pub(crate) fn attached(&self, terminal: TerminalId, from: u64, reset: bool) {
        if !reset {
            return;
        }
        let sink = {
            let mut attached = self.lock();
            let Some(one) = attached.get_mut(&terminal) else {
                return;
            };
            one.after = Some(from);
            Arc::clone(&one.sink)
        };
        sink.reset(terminal, from);
    }

    /// The attaches a new connection sends, one for each terminal, each
    /// going on from where its output got to.
    pub(crate) fn replay(&self) -> Vec<ClientMessage> {
        self.lock()
            .iter()
            .map(|(terminal, one)| ClientMessage::TerminalAttach {
                terminal: *terminal,
                after: one.after,
            })
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<TerminalId, Attached>> {
        self.attached.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// What a [`Noted`] sink got: output with its bytes, or a reset.
    pub(crate) type Note = (TerminalId, u64, Option<Vec<u8>>);

    /// A sink that notes what it gets.
    #[derive(Default)]
    pub(crate) struct Noted(pub Mutex<Vec<Note>>);

    impl Noted {
        /// Everything so far, in order.
        pub(crate) fn notes(&self) -> Vec<Note> {
            self.0.lock().unwrap().clone()
        }
    }

    impl TerminalSink for Noted {
        fn output(&self, terminal: TerminalId, offset: u64, bytes: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .push((terminal, offset, Some(bytes.to_vec())));
        }

        fn reset(&self, terminal: TerminalId, from: u64) {
            self.0.lock().unwrap().push((terminal, from, None));
        }
    }

    pub(crate) fn terminal(n: u8) -> TerminalId {
        format!("0199a7c2-0000-7000-8000-0000000000{n:02x}")
            .parse()
            .unwrap()
    }

    #[test]
    fn output_goes_to_the_attached_sink_and_moves_where_a_reattach_starts() {
        let terminals = Terminals::default();
        let sink = Arc::new(Noted::default());
        let (one, two) = (terminal(1), terminal(2));
        terminals.attach(one, None, sink.clone());
        terminals.attach(two, Some(40), sink.clone());
        terminals.output(one, 0, b"hello ");
        terminals.output(one, 6, b"world");
        // Output for a terminal nobody attached goes nowhere.
        terminals.output(terminal(3), 0, b"stray");
        let mut replay = terminals.replay();
        replay.sort_by_key(|message| match message {
            ClientMessage::TerminalAttach { terminal, .. } => *terminal,
            _ => unreachable!(),
        });
        assert_eq!(
            replay,
            vec![
                ClientMessage::TerminalAttach {
                    terminal: one,
                    after: Some(11)
                },
                ClientMessage::TerminalAttach {
                    terminal: two,
                    after: Some(40)
                },
            ]
        );
        assert_eq!(
            sink.notes(),
            vec![
                (one, 0, Some(b"hello ".to_vec())),
                (one, 6, Some(b"world".to_vec())),
            ]
        );
    }

    #[test]
    fn a_reset_moves_the_start_and_a_detach_drops_what_follows() {
        let terminals = Terminals::default();
        let sink = Arc::new(Noted::default());
        let one = terminal(1);
        terminals.attach(one, Some(10), sink.clone());
        // An answer without a reset changes nothing.
        terminals.attached(one, 10, false);
        terminals.attached(one, 900, true);
        assert_eq!(
            terminals.replay(),
            vec![ClientMessage::TerminalAttach {
                terminal: one,
                after: Some(900)
            }]
        );
        assert!(terminals.detach(one));
        assert!(!terminals.detach(one));
        terminals.output(one, 900, b"late");
        terminals.attached(one, 1_000, true);
        assert!(terminals.replay().is_empty());
        assert_eq!(sink.notes(), vec![(one, 900, None)]);
    }
}
