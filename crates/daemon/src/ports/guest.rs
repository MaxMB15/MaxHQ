// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's port watch. Once the host sends `WatchPorts`, it polls
//! `/proc` for the whole container at the host's pace and sends `Ports`
//! each time the shown set changes, and after each `WatchPorts`. A port
//! in a watched holder's tree carries its session, and the rest carry
//! none. Its own sockets never show.
//!
//! One watch runs for the whole container and outlives host links, so a
//! new link gets the shown set at once. While no link is up, it polls at
//! the slow pace until a host asks again.
//!
//! The guest's tunnels read [`PortWatch::detected`], so they connect only
//! to ports this watch found.

use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::guest::{GuestPort, GuestToHost, HostToGuest, Pace, WatchedHolder, MAX_PORTS};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

use super::{interval, Debounce, Procfs, Scanner};
use crate::tunnel::{Out, Sink};

/// What the host last asked for.
type Ask = Option<(Pace, Vec<WatchedHolder>)>;

/// The container's port watch. Dropping it stops the polls.
pub struct PortWatch {
    asks: watch::Sender<Ask>,
    detected: watch::Receiver<Vec<GuestPort>>,
    task: JoinHandle<()>,
}

impl PortWatch {
    /// Starts a watch over this container's `/proc` that reports on
    /// `sink`. It polls nothing until the first `WatchPorts`.
    pub fn start(sink: Arc<dyn Sink<GuestToHost>>) -> Self {
        Self::with(sink, Procfs::new("/proc", true), interval)
    }

    /// Starts a watch that polls `scanner`, waiting `tick` of a pace
    /// between polls.
    pub(crate) fn with(
        sink: Arc<dyn Sink<GuestToHost>>,
        scanner: impl Scanner,
        tick: fn(Pace) -> Duration,
    ) -> Self {
        let (asks, asked) = watch::channel(None);
        let (shown, detected) = watch::channel(Vec::new());
        let task = tokio::spawn(run(sink, asked, shown, scanner, tick));
        Self {
            asks,
            detected,
            task,
        }
    }

    /// Takes a `WatchPorts` from the host. False for any other message.
    pub fn handle(&self, message: &HostToGuest) -> bool {
        match message {
            HostToGuest::WatchPorts { pace, holders } => {
                self.asks.send_replace(Some((*pace, holders.clone())));
                true
            }
            _ => false,
        }
    }

    /// Polls at the slow pace from now on and keeps the holders, since no
    /// host link is up to hear. The next `WatchPorts` sets the pace again.
    pub fn slow(&self) {
        self.asks.send_if_modified(|ask| match ask {
            Some((pace, _)) if *pace != Pace::Slow => {
                *pace = Pace::Slow;
                true
            }
            _ => false,
        });
    }

    /// Whether a host has asked for ports since the watch started.
    pub fn asked(&self) -> bool {
        self.asks.borrow().is_some()
    }

    /// The ports shown now, for the tunnels.
    pub fn detected(&self) -> watch::Receiver<Vec<GuestPort>> {
        self.detected.clone()
    }
}

impl Drop for PortWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run<S: Scanner>(
    sink: Arc<dyn Sink<GuestToHost>>,
    mut asked: watch::Receiver<Ask>,
    shown: watch::Sender<Vec<GuestPort>>,
    scanner: S,
    tick: fn(Pace) -> Duration,
) {
    let mut scanner = Some(scanner);
    let mut debounce = Debounce::new();
    let mut last: Option<Vec<GuestPort>> = None;
    let mut fresh = true;
    loop {
        let Some((pace, holders)) = asked.borrow_and_update().clone() else {
            if asked.changed().await.is_err() {
                return;
            }
            continue;
        };
        let Some(mut taken) = scanner.take() else {
            return;
        };
        let found = match tokio::task::spawn_blocking(move || {
            let found = taken.scan(&holders);
            (taken, found)
        })
        .await
        {
            Ok((back, found)) => {
                scanner = Some(back);
                found
            }
            Err(error) => {
                warn!(%error, "the port scan failed, so the port watch stops");
                return;
            }
        };
        let mut ports = debounce.poll(found);
        ports.truncate(MAX_PORTS);
        shown.send_replace(ports.clone());
        if fresh || last.as_ref() != Some(&ports) {
            let message = GuestToHost::Ports {
                listening: ports.clone(),
            };
            if sink.send(Out::Message(message)).is_err() {
                return;
            }
            last = Some(ports);
        }
        tokio::select! {
            () = tokio::time::sleep(tick(pace)) => fresh = false,
            changed = asked.changed() => {
                if changed.is_err() {
                    return;
                }
                fresh = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use maxhq_protocol::ids::SessionId;
    use maxhq_protocol::model::BindAddress;

    use super::*;
    use crate::tunnel::registry_tests::{next, sent, sink};

    /// Gives a fixed list, with the session of the first holder.
    struct Fake(Arc<Mutex<Vec<u16>>>);

    impl Scanner for Fake {
        fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort> {
            let ports = self.0.lock().unwrap();
            ports
                .iter()
                .map(|port| GuestPort {
                    port: *port,
                    bind: BindAddress::Any,
                    session: holders.first().map(|holder| holder.session),
                    process: None,
                })
                .collect()
        }
    }

    fn listed(out: &Out<GuestToHost>) -> Vec<(u16, Option<SessionId>)> {
        match out {
            Out::Message(GuestToHost::Ports { listening }) => listening
                .iter()
                .map(|port| (port.port, port.session))
                .collect(),
            other => panic!("not a port list: {other:?}"),
        }
    }

    fn ask(pace: Pace, holders: Vec<WatchedHolder>) -> HostToGuest {
        HostToGuest::WatchPorts { pace, holders }
    }

    /// A fast pace short enough for a test, and a slow one that never
    /// comes.
    fn tick(pace: Pace) -> Duration {
        match pace {
            Pace::Fast => Duration::from_millis(30),
            Pace::Slow => Duration::from_secs(3600),
        }
    }

    #[tokio::test]
    async fn the_watch_waits_for_the_host_then_reports_changes() {
        let (out, mut rx) = sink();
        let table = Arc::new(Mutex::new(vec![5173]));
        let watch = PortWatch::with(out, Fake(table.clone()), tick);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(sent(&mut rx).is_empty(), "nothing before WatchPorts");

        assert!(watch.handle(&ask(Pace::Slow, Vec::new())));
        assert!(!watch.handle(&HostToGuest::ListHolders { req: 1 }));
        // The first poll only counts the port once, but the host hears.
        assert!(listed(&next(&mut rx).await).is_empty());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(sent(&mut rx).is_empty(), "the slow pace waits");

        // A new ask polls at once.
        watch.handle(&ask(Pace::Fast, Vec::new()));
        assert_eq!(listed(&next(&mut rx).await), [(5173, None)]);
        assert_eq!(watch.detected().borrow().len(), 1);

        // No change, no message.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(sent(&mut rx).is_empty());

        let holder = WatchedHolder {
            session: SessionId::derive("s"),
            agent: None,
            pid: 40,
        };
        watch.handle(&ask(Pace::Fast, vec![holder]));
        assert_eq!(
            listed(&next(&mut rx).await),
            [(5173, Some(SessionId::derive("s")))]
        );

        table.lock().unwrap().clear();
        assert!(listed(&next(&mut rx).await).is_empty());
        assert!(watch.detected().borrow().is_empty());
    }

    #[tokio::test]
    async fn slowing_down_keeps_the_holders() {
        let (out, mut rx) = sink();
        let watch = PortWatch::with(out, Fake(Arc::new(Mutex::new(vec![5173]))), tick);
        // Nothing was asked, so there's nothing to slow.
        watch.slow();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(sent(&mut rx).is_empty());
        assert!(!watch.asked());

        let session = SessionId::derive("s");
        let holder = WatchedHolder {
            session,
            agent: None,
            pid: 40,
        };
        watch.handle(&ask(Pace::Fast, vec![holder]));
        assert!(watch.asked());
        assert!(listed(&next(&mut rx).await).is_empty());
        assert_eq!(listed(&next(&mut rx).await), [(5173, Some(session))]);

        // The slow pace polls at once, for the same holders, then waits.
        watch.slow();
        assert_eq!(listed(&next(&mut rx).await), [(5173, Some(session))]);
        watch.slow();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(sent(&mut rx).is_empty());
    }
}
