// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's ports and tunnels on its host links. One port watch runs
//! for the container and reports to whichever link is up. A new link
//! hears the shown ports as soon as it comes up, then each change. The
//! tunnels belong to the link they came on, and end with it.
//!
//! The SSH server, when there is one, outlives links. Each new link hears
//! its host key, and the keys that link pushes are forgotten when it
//! drops.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::guest::{GuestToHost, HostToGuest};
use maxhq_protocol::ids::StreamId;
use tracing::warn;

use super::ssh::Ssh;
use super::tunnel::GuestTunnels;
use super::{Outbox, Tunnels};
use crate::guestlink::wire::SendError;
use crate::ports::guest::PortWatch;
use crate::tunnel::{Gone, Out, Queue, Sink, Writer};

/// Where messages for the host go.
type ToHost = Arc<dyn Sink<GuestToHost>>;

/// The container's port watch, and the tunnels of the link that's up.
pub(crate) struct Ports {
    watch: PortWatch,
    reports: Arc<Reports>,
    /// The link that's up, by number, and its tunnels.
    tunnels: Mutex<Option<(u64, GuestTunnels)>>,
    /// The SSH server, if the guest runs one.
    ssh: Option<Arc<Ssh>>,
}

impl Ports {
    /// Starts the watch over this container's `/proc`.
    pub(crate) fn start() -> Self {
        Self::with(PortWatch::start)
    }

    /// Starts the watch `make` gives, which reports on the sink it's
    /// handed.
    pub(crate) fn with(make: impl FnOnce(ToHost) -> PortWatch) -> Self {
        let reports = Arc::new(Reports::default());
        let watch = make(reports.clone());
        Self {
            watch,
            reports,
            tunnels: Mutex::new(None),
            ssh: None,
        }
    }

    /// Takes SSH sessions to `ssh`.
    pub(crate) fn with_ssh(mut self, ssh: Arc<Ssh>) -> Self {
        self.ssh = Some(ssh);
        self
    }
}

impl Tunnels for Ports {
    fn link_up(&self, link: u64, outbox: &Outbox) {
        let queue: ToHost = Arc::new(Queue::spawn(LinkWriter(outbox.clone())));
        if let Some(ssh) = &self.ssh {
            let public = ssh.host_key().to_owned();
            let _ = queue.send(Out::Message(GuestToHost::SshHostKey { public }));
        }
        let tunnels = GuestTunnels::new(queue.clone(), self.watch.detected(), self.ssh.clone());
        let old = lock(&self.tunnels).replace((link, tunnels));
        drop(old);
        self.reports.link_up(link, queue, &self.watch);
    }

    fn message(&self, message: HostToGuest, outbox: &Outbox) {
        if self.watch.handle(&message) {
            return;
        }
        if let HostToGuest::SshAuthorized { keys } = &message {
            if let Some(ssh) = &self.ssh {
                ssh.authorize(keys);
            }
            return;
        }
        if let Some((_, tunnels)) = lock(&self.tunnels).as_ref() {
            tunnels.handle(&message);
        } else if let HostToGuest::TunnelOpen { stream, .. } = message {
            outbox.post(&GuestToHost::TunnelRefused {
                stream,
                reason: "the host link is down".to_owned(),
            });
        }
    }

    fn data(&self, stream: StreamId, data: Vec<u8>, _outbox: &Outbox) {
        if let Some((_, tunnels)) = lock(&self.tunnels).as_ref() {
            tunnels.data(stream, &data);
        }
    }

    fn link_down(&self, link: u64) {
        let gone = {
            let mut tunnels = lock(&self.tunnels);
            match tunnels.as_ref() {
                Some((at, _)) if *at == link => tunnels.take(),
                _ => None,
            }
        };
        if gone.is_none() {
            return;
        }
        // Dropping the tunnels aborts their streams.
        drop(gone);
        if let Some(ssh) = &self.ssh {
            ssh.forget();
        }
        self.reports.link_down(link);
        self.watch.slow();
    }
}

/// Where the watch reports: the link that's up, or nowhere.
#[derive(Default)]
struct Reports {
    link: Mutex<Option<(u64, ToHost)>>,
}

impl Reports {
    /// Sends the shown ports on a new link, once a host has asked for
    /// them, then sends it each change. Both happen under one lock, so
    /// the link never hears an older set after a newer one.
    fn link_up(&self, link: u64, sink: ToHost, watch: &PortWatch) {
        let mut current = lock(&self.link);
        if watch.asked() {
            let listening = watch.detected().borrow().clone();
            if sink
                .send(Out::Message(GuestToHost::Ports { listening }))
                .is_err()
            {
                return;
            }
        }
        *current = Some((link, sink));
    }

    fn link_down(&self, link: u64) {
        let mut current = lock(&self.link);
        if current.as_ref().is_some_and(|(at, _)| *at == link) {
            *current = None;
        }
    }
}

impl Sink<GuestToHost> for Reports {
    fn send(&self, out: Out<GuestToHost>) -> Result<(), Gone> {
        let mut current = lock(&self.link);
        if let Some((_, sink)) = current.as_ref()
            && sink.send(out).is_err()
        {
            *current = None;
        }
        // The watch carries on with no link up, so the next one hears.
        Ok(())
    }
}

/// Writes what a link's queue holds to its outbox.
struct LinkWriter(Outbox);

impl Writer<GuestToHost> for LinkWriter {
    async fn write(&mut self, out: Out<GuestToHost>) -> Result<(), Gone> {
        let sent = match out {
            Out::Message(message) => self.0.send(&message).await,
            Out::Data { stream, data } => self.0.tunnel_data(stream, &data).await,
        };
        match sent {
            Ok(()) => Ok(()),
            Err(SendError::Encode(err)) => {
                warn!("couldn't encode a message for the host: {err}");
                Ok(())
            }
            Err(_) => Err(Gone),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
