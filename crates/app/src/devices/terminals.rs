// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminal bytes: each window's attaches go to a link to the terminal's
//! device, whose output then comes to the window's terminal channel, and
//! what you type goes straight to the same link. Nothing here keeps or
//! logs what you type.

use std::sync::Arc;

use maxhq_client::LinkError;
use maxhq_protocol::ids::{LinkId, TerminalId};
use maxhq_protocol::state::EntityKey;

use super::{gone, link_name, request_failed, Connector, Core, Devices, LinkControl};
use crate::request::CommandError;
use crate::terminals::{Detach, RecordSink};

impl<C: Connector> Devices<C> {
    /// Has a window attach a terminal from byte `after` of its output on.
    /// The output comes to `sink`, the window's terminal channel, as
    /// records under the number this gives. A link that's down attaches
    /// it once it's back.
    pub async fn attach_terminal(
        &self,
        window: &str,
        sink: Arc<RecordSink>,
        terminal: TerminalId,
        after: Option<u64>,
    ) -> Result<u32, CommandError> {
        self.core
            .attach_terminal(window, sink, terminal, after)
            .await
    }

    /// Takes back a window's attach of a terminal. The terminal detaches on
    /// its link once no window has it attached.
    pub async fn detach_terminal(&self, window: &str, terminal: TerminalId) {
        let _order = self.core.order.lock().await;
        if let Some(link) = self.core.terminals.detach(window, terminal) {
            self.core.detach_terminals(vec![(terminal, link)]).await;
        }
    }

    /// Sends what you typed into a terminal, on the link its output comes
    /// through.
    pub async fn terminal_input(
        &self,
        terminal: TerminalId,
        bytes: Vec<u8>,
    ) -> Result<(), CommandError> {
        let (id, link) = self.core.terminal_link(terminal)?;
        match link.terminal_input(terminal, bytes).await {
            Ok(()) => Ok(()),
            Err(error @ LinkError::TooOld) => Err(self.core.link_failed(&error, id)),
            Err(_) => Err(CommandError::NotConnected {
                device: self.core.view.read(|merge| link_name(merge, id)),
            }),
        }
    }
}

impl<L: LinkControl> Core<L> {
    /// Attaches a terminal for a window on the link its windows share, in
    /// turn with the other changes.
    async fn attach_terminal(
        &self,
        window: &str,
        sink: Arc<RecordSink>,
        terminal: TerminalId,
        after: Option<u64>,
    ) -> Result<u32, CommandError> {
        let _order = self.order.lock().await;
        let (id, link) = self.terminal_link(terminal)?;
        let (number, released) = self.terminals.attach(window, sink, terminal, after, id);
        self.detach_terminals(released).await;
        match link
            .attach_terminal(terminal, after, self.terminals.feed())
            .await
        {
            Err(error @ LinkError::TooOld) => {
                if let Some(id) = self.terminals.detach(window, terminal) {
                    self.detach_terminals(vec![(terminal, id)]).await;
                }
                return Err(self.link_failed(&error, id));
            }
            // The link attaches it again once it's back.
            Err(error) => {
                tracing::debug!(%id, %terminal, %error, "a link didn't attach a terminal yet")
            }
            Ok(()) => {}
        }
        Ok(number)
    }

    /// Detaches terminals no window has attached now, on their links. One
    /// that a window attached again on the same link meanwhile stays.
    pub(super) async fn detach_terminals(&self, detaches: Vec<Detach>) {
        for (terminal, id) in detaches {
            if self.terminals.link_of(terminal) == Some(id) {
                continue;
            }
            let Some(link) = self.link(id) else {
                continue;
            };
            if let Err(error) = link.detach_terminal(terminal).await {
                tracing::debug!(%id, %terminal, %error, "a link didn't detach a terminal");
            }
        }
    }

    /// The link to reach a terminal on: the one its windows share, or else
    /// the first open link that holds it, connected ones first.
    fn terminal_link(&self, terminal: TerminalId) -> Result<(LinkId, Arc<L>), CommandError> {
        if let Some(id) = self.terminals.link_of(terminal)
            && let Some(link) = self.link(id)
        {
            return Ok((id, link));
        }
        let holders: Vec<(LinkId, String)> = self.view.read(|merge| {
            merge
                .holders(&EntityKey::Terminal(terminal))
                .into_iter()
                .map(|link| (link.id, link.name.clone()))
                .collect()
        });
        let Some((_, first)) = holders.first() else {
            return Err(gone("This terminal"));
        };
        let device = first.clone();
        holders
            .into_iter()
            .find_map(|(id, _)| Some((id, self.link(id)?)))
            .ok_or(CommandError::NotConnected { device })
    }

    /// Why a request on link `id` failed, naming its device.
    fn link_failed(&self, error: &LinkError, id: LinkId) -> CommandError {
        request_failed(error, &self.view.read(|merge| link_name(merge, id)))
    }
}

#[cfg(test)]
mod tests {
    use maxhq_client::{LinkEvent, TerminalSink};
    use maxhq_protocol::ids::{AgentId, DeviceId};
    use maxhq_protocol::state::{Change, Entity};
    use maxhq_protocol::terminal::{Terminal, TerminalKind, TerminalStatus};
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::devices::tests::{two_devices, wait_until, Call, Harness};
    use crate::terminals::record;
    use crate::tests::block_on;
    use crate::timeline::tests::Recorder;

    fn shell() -> TerminalId {
        TerminalId::derive("shell")
    }

    /// Has the link `id` report the shell, on the device "nl".
    fn holds(h: &Harness, id: LinkId) {
        let shell = Terminal {
            id: shell(),
            agent: AgentId::derive("b"),
            device: DeviceId::derive("nl"),
            kind: TerminalKind::Shell,
            moved: false,
            title: "zsh".into(),
            cwd: "~/code/example-app".into(),
            status: TerminalStatus::Running,
            busy: false,
            marks: false,
            password_prompt: false,
            reads_keys: false,
            full_screen: false,
            app_cursor: false,
            waiting: false,
            answers: false,
            choices: Vec::new(),
            started_at: Timestamp(1_793_368_920_000),
            cols: 80,
            rows: 24,
        };
        h.report(
            id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: Entity::Terminal(shell),
            }]),
        );
    }

    /// The sink the fake link got with the shell's last attach.
    fn feed(h: &Harness) -> Arc<dyn TerminalSink> {
        h.fake.record().terminal_sinks[&shell()].clone()
    }

    #[test]
    fn windows_share_one_attach_and_the_last_to_go_detaches_it() {
        let (h, _, nl) = two_devices();
        let (main, other) = (Recorder::<Vec<u8>>::new(1), Recorder::<Vec<u8>>::new(2));
        let attach = |window: &str, sink: &Arc<Recorder<Vec<u8>>>, after| {
            block_on(
                h.devices
                    .attach_terminal(window, sink.clone(), shell(), after),
            )
        };
        assert_eq!(attach("main", &main, None), Err(gone("This terminal")));
        holds(&h, nl.id);
        h.fake.calls();

        assert_eq!(attach("main", &main, None), Ok(1));
        assert_eq!(attach("other", &other, Some(2)), Ok(1));
        assert_eq!(
            h.fake.calls(),
            [
                (nl.id, Call::AttachTerminal(shell(), None)),
                (nl.id, Call::AttachTerminal(shell(), Some(2))),
            ]
        );
        // The link's output reaches both windows, past what each has.
        feed(&h).output(shell(), 0, b"$ ls");
        wait_until("the output never came", || {
            main.messages() == [record(1, 0, b"$ ls")] && other.messages() == [record(1, 2, b"ls")]
        });

        block_on(h.devices.detach_terminal("main", shell()));
        assert!(h.fake.calls().is_empty(), "the other window has it");
        block_on(h.devices.release_window("other")).unwrap();
        assert_eq!(h.fake.calls(), [(nl.id, Call::DetachTerminal(shell()))]);
        block_on(h.devices.detach_terminal("main", shell()));
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn a_reloaded_window_keeps_the_terminal_it_attaches_again() {
        let (h, _, nl) = two_devices();
        holds(&h, nl.id);
        let (old, new) = (Recorder::<Vec<u8>>::new(1), Recorder::<Vec<u8>>::new(2));
        block_on(h.devices.attach_terminal("main", old, shell(), None)).unwrap();
        // The page reloads and attaches again before the release takes its
        // turn. `release_window` takes that turn on a task of its own, which
        // may go first, so the test takes its steps itself.
        let released = h.devices.core.terminals.release("main");
        assert_eq!(released, [(shell(), nl.id)]);
        block_on(
            h.devices
                .attach_terminal("main", new.clone(), shell(), None),
        )
        .unwrap();
        block_on(h.devices.core.detach_terminals(released));
        let calls = h.fake.calls();
        assert!(
            !calls
                .iter()
                .any(|(_, call)| matches!(call, Call::DetachTerminal(_))),
            "the new page still has it: {calls:?}"
        );
        feed(&h).output(shell(), 0, b"%");
        wait_until("the output never came", || {
            new.messages() == [record(1, 0, b"%")]
        });
    }

    #[test]
    fn input_goes_on_the_terminals_link_and_only_its_length_is_noted() {
        let (h, _, nl) = two_devices();
        let input = |bytes: &[u8]| block_on(h.devices.terminal_input(shell(), bytes.to_vec()));
        assert_eq!(input(b"ls\r"), Err(gone("This terminal")));
        holds(&h, nl.id);
        input(b"ls\r").unwrap();
        assert_eq!(h.fake.calls(), [(nl.id, Call::TerminalInput(shell(), 3))]);

        h.fake.record().up.remove(&nl.id);
        assert!(matches!(
            input(b"q"),
            Err(CommandError::NotConnected { .. })
        ));
        h.fake.record().old.insert(nl.id);
        h.fake.record().up.insert(nl.id);
        assert_eq!(
            input(b"q"),
            Err(crate::devices::failed(
                "maxd on devbox is too old for this; update it".to_owned()
            ))
        );
        let window = Recorder::<Vec<u8>>::new(1);
        assert_eq!(
            block_on(h.devices.attach_terminal("main", window, shell(), None)),
            Err(crate::devices::failed(
                "maxd on devbox is too old for this; update it".to_owned()
            ))
        );
        assert_eq!(h.devices.core.terminals.link_of(shell()), None);
    }
}
