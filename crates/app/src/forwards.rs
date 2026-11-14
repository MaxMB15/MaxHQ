// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Ports that listen on a device, reached from this machine. A forward
//! listens on localhost here and carries each connection over the device's
//! link, so a dev server in a workspace on devbox opens in the browser
//! like one on this Mac.

use maxhq_protocol::ids::ForwardId;
use serde::Serialize;
use specta::Type;

use crate::devices::Forwarded;
use crate::request::CommandError;
use crate::Shell;

/// A forward that opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct OpenedForward {
    /// The device's forward, once the device has answered. A link that's
    /// reconnecting listens here before the device knows.
    pub forward: Option<ForwardId>,
    /// The port it listens on here. It's the port it reaches unless that
    /// one was taken.
    pub local_port: u16,
}

impl From<Forwarded> for OpenedForward {
    fn from(forwarded: Forwarded) -> Self {
        Self {
            forward: forwarded.forward,
            local_port: forwarded.local_port,
        }
    }
}

/// Opens `http://localhost` at `port` in the browser, for a port an agent
/// on this machine listens on or one a forward listens on here.
pub async fn open_localhost(shell: &dyn Shell, port: u16) -> Result<(), CommandError> {
    if port == 0 {
        return Err(CommandError::Failed {
            message: "Port 0 isn't a port anything listens on".to_owned(),
        });
    }
    shell
        .open_browser(localhost(port))
        .await
        .map_err(|message| CommandError::Failed { message })
}

/// The address of a port on this machine.
fn localhost(port: u16) -> String {
    format!("http://localhost:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::tests::{connect_doing, two_devices, Call, Harness};
    use crate::tests::{block_on, FakeShell};
    use maxhq_client::LinkEvent;
    use maxhq_protocol::ids::{ClientId, LinkId, PortId, WorkspaceId};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::{BindAddress, Forward, ListeningPort};
    use maxhq_protocol::state::{Change, Entity};
    use maxhq_protocol::time::Timestamp;

    /// A port that listens in `workspace`.
    fn listening(name: &str, workspace: &str, port: u16) -> Entity {
        Entity::ListeningPort(ListeningPort {
            id: PortId::derive(name),
            workspace: WorkspaceId::derive(workspace),
            agent: None,
            port,
            bind: BindAddress::Loopback,
            process: None,
            url_hint: None,
            first_seen: Timestamp(1),
            gone_since: None,
        })
    }

    /// Has a link report an entity changed, and waits for the view.
    fn reports(h: &Harness, link: LinkId, entity: Entity) {
        h.report(
            link,
            LinkEvent::Changed(vec![Change::Upsert { value: entity }]),
        );
    }

    fn forward_calls(h: &Harness) -> Vec<(LinkId, Call)> {
        h.fake
            .calls()
            .into_iter()
            .filter(|(_, call)| matches!(call, Call::OpenForward(_) | Call::CloseForward(_)))
            .collect()
    }

    #[test]
    fn a_port_forwards_through_its_workspaces_device_and_closes_again() {
        let (h, _, nl) = two_devices();
        reports(&h, nl.id, listening("vite", "w-nl", 5173));
        h.fake.calls();

        let opened = block_on(h.devices.open_forward(PortId::derive("vite"))).unwrap();
        assert_eq!(opened.local_port, 5173);
        let forward = opened.forward.unwrap();
        assert_eq!(
            forward_calls(&h),
            [(nl.id, Call::OpenForward(PortId::derive("vite")))]
        );

        // The device reports the forward, and closing it reaches the link.
        reports(
            &h,
            nl.id,
            Entity::Forward(Forward {
                id: forward,
                port: PortId::derive("vite"),
                client: ClientId::derive("this mac"),
                local_port: 5173,
                opened_at: Timestamp(2),
            }),
        );
        h.devices.close_forward(forward).unwrap();
        assert_eq!(
            forward_calls(&h),
            [(nl.id, Call::CloseForward(PortId::derive("vite")))]
        );
        assert!(h.fake.record().forwards.values().all(Vec::is_empty));

        // One that's gone already changes nothing.
        assert_eq!(h.devices.close_forward(ForwardId::derive("gone")), Ok(()));
        assert!(forward_calls(&h).is_empty());
    }

    #[test]
    fn a_device_that_doesnt_forward_ports_refuses_before_anything_listens() {
        let (h, _, nl) = two_devices();
        connect_doing(
            &h,
            &nl,
            "nl",
            Capabilities {
                transfers: true,
                tunnels: false,
                workspaces: true,
                github_checks: true,
                inbox: false,
                ..Capabilities::default()
            },
        );
        reports(&h, nl.id, listening("vite", "w-nl", 5173));
        h.fake.calls();

        assert_eq!(
            block_on(h.devices.open_forward(PortId::derive("vite"))),
            Err(CommandError::Failed {
                message: "devbox doesn't forward ports".into()
            })
        );
        assert!(forward_calls(&h).is_empty());

        assert_eq!(
            block_on(h.devices.open_forward(PortId::derive("missing"))),
            Err(CommandError::NotFound {
                what: "This port".into()
            })
        );
    }

    #[test]
    fn localhost_is_this_machine_at_the_port() {
        let shell = FakeShell::default();
        block_on(open_localhost(&shell, 5173)).unwrap();
        assert_eq!(
            block_on(open_localhost(&shell, 0)),
            Err(CommandError::Failed {
                message: "Port 0 isn't a port anything listens on".into()
            })
        );
        assert_eq!(*shell.0.lock().unwrap(), ["http://localhost:5173"]);
    }
}
