// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app's own client and app key against a real `maxd`: it proves its
//! key and gets past the gate, goes in as the first key, waits for your
//! yes as a second Mac's, and moves to a new key.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_app::app_keys::{AppKeys, Keeper, Record};
use maxhq_app::keys::{Key, KeyStore, MemoryStore};
use maxhq_client::pinned::{PeerDial, PeerStream};
use maxhq_client::proof::Prover;
use maxhq_client::{Client, LinkEvent, LinkHandle, LinkSpec, Transport};
use maxhq_protocol::admission::{AppKey, AppKeyOrigin};
use maxhq_protocol::ids::{DeviceId, LinkId};
use maxhq_protocol::link::{ConnectStep, LinkState, Route};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::NEW;
use crate::app_keys::testing::{self, SEED};
use crate::server::tests::{hub, serve, serve_bare, Served, WAIT};

/// What the app calls its key.
const LABEL: &str = "MaxHQ on studio-mac";

/// Dials the test `maxd`'s socket, whatever the route says, and sends
/// `first` ahead of the client's bytes, as `maxd proxy` sends its first
/// frame.
pub(super) struct Dial {
    pub(super) socket: PathBuf,
    pub(super) first: Vec<u8>,
}

impl PeerDial for Dial {
    fn dial(&self, _host: &str) -> BoxFuture<'static, io::Result<PeerStream>> {
        let (socket, first) = (self.socket.clone(), self.first.clone());
        Box::pin(async move {
            let mut stream = tokio::net::UnixStream::connect(socket).await?;
            stream.write_all(&first).await?;
            let (reader, writer) = stream.into_split();
            Ok((Box::new(reader) as _, Box::new(writer) as _))
        })
    }
}

/// Knows one device, and keeps no record.
struct Knows(DeviceId);

impl Keeper for Knows {
    fn save(&self, _record: &Record) {}

    fn devices(&self) -> Vec<DeviceId> {
        vec![self.0]
    }
}

/// The app's key, from a Keychain in memory that holds `seed`.
fn keys(seed: [u8; 32]) -> AppKeys {
    let store = MemoryStore::default();
    store.save(Key::App, &seed).unwrap();
    AppKeys::open(Box::new(store), None, LABEL.into()).unwrap()
}

/// The app, linked to `served` and proving with `keys`, or a phone with
/// its own prover.
pub(super) struct App {
    client: Client,
    pub(super) link: LinkHandle,
    events: mpsc::Receiver<LinkEvent>,
}

impl App {
    fn link(served: &Served, keys: &AppKeys) -> Self {
        let dial = Dial {
            socket: served.socket.clone(),
            first: Vec::new(),
        };
        Self::through(dial, Arc::new(keys.clone()))
    }

    /// The app's client over `dial`, proving with `prover`.
    pub(super) fn through(dial: Dial, prover: Arc<dyn Prover>) -> Self {
        let client = Client::app(Transport::Streams(Arc::new(dial))).proving(prover);
        let (link, events) = client.link(LinkSpec {
            id: LinkId::generate(),
            name: "mac".into(),
            route: Route::Ssh { host: "mac".into() },
            fallbacks: Vec::new(),
            last_good: BTreeMap::new(),
        });
        Self {
            client,
            link,
            events,
        }
    }

    /// The next event, or why the link gave up.
    pub(super) async fn try_next(&mut self) -> Result<LinkEvent, String> {
        let event = timeout(WAIT, self.events.recv()).await;
        let event = event.expect("an event in time").expect("the link runs");
        match &event {
            LinkEvent::Link(link) => match &link.state {
                LinkState::Unreachable { reason, .. } => Err(reason.clone()),
                _ => Ok(event),
            },
            _ => Ok(event),
        }
    }

    /// The next event, failing on a link that gave up.
    pub(super) async fn next(&mut self) -> LinkEvent {
        self.try_next()
            .await
            .unwrap_or_else(|reason| panic!("the link gave up: {reason}"))
    }

    /// The app keys in the first full sync.
    pub(super) async fn synced(&mut self) -> Vec<AppKey> {
        loop {
            if let LinkEvent::Synced(entities) = self.next().await {
                return app_keys(entities);
            }
        }
    }
}

fn app_keys(entities: Vec<Entity>) -> Vec<AppKey> {
    entities
        .into_iter()
        .filter_map(|entity| match entity {
            Entity::AppKey(key) => Some(key),
            _ => None,
        })
        .collect()
}

/// Waits until `done` says so.
async fn until(what: &str, done: impl Fn() -> bool) {
    let waited = timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    waited.await.unwrap_or_else(|_| panic!("{what} in time"));
}

#[tokio::test]
async fn the_app_proves_its_key_and_gets_past_the_gate() {
    let served = serve();
    let mut app = App::link(&served, &keys(SEED));
    let [key] = app.synced().await.try_into().unwrap();
    assert_eq!(key.public_key, testing::public_key());
    assert!(key.proved_at.is_some(), "the proof dates the key");
}

#[tokio::test]
async fn a_device_that_trusts_no_key_takes_the_apps_as_the_first() {
    let served = serve_bare(hub());
    let mut app = App::link(&served, &keys(SEED));
    let shown = app.synced().await;
    let [key] = served.keys.keys().unwrap().try_into().unwrap();
    assert_eq!(key.public_key, testing::public_key());
    assert_eq!(key.origin, AppKeyOrigin::First);
    assert_eq!(key.label, LABEL);
    assert_eq!(shown, [key]);
}

#[tokio::test]
async fn a_second_macs_key_waits_for_your_yes() {
    let served = serve();
    let mut app = App::link(&served, &keys(NEW));
    loop {
        match app.next().await {
            LinkEvent::Link(link)
                if matches!(
                    link.state,
                    LinkState::Connecting {
                        step: ConnectStep::WaitingForApproval,
                        ..
                    }
                ) =>
            {
                break;
            }
            LinkEvent::Synced(_) => panic!("an unknown key got past the gate"),
            _ => {}
        }
    }
    let trusted: Vec<String> = served
        .keys
        .keys()
        .unwrap()
        .into_iter()
        .map(|key| key.public_key)
        .collect();
    assert_eq!(trusted, [testing::public_key()]);
}

#[tokio::test]
async fn a_new_key_goes_in_and_the_old_one_comes_out() {
    let served = serve();
    let device = served.hub.device().id;
    let keys = keys(SEED);
    keys.keep(Box::new(Knows(device)));
    let mut app = App::link(&served, &keys);
    app.synced().await;

    keys.rotate().unwrap();
    let new = keys.public_key();
    assert_ne!(new, testing::public_key());
    app.client.reprove();

    // The device shows the old key going.
    let old = EntityKey::AppKey(AppKey::id_for(device, &testing::public_key()));
    loop {
        if let LinkEvent::Changed(changes) = app.next().await
            && changes
                .iter()
                .any(|change| matches!(change, Change::Remove { key } if *key == old))
        {
            break;
        }
    }
    let [key] = served.keys.keys().unwrap().try_into().unwrap();
    assert_eq!(key.public_key, new);
    assert_eq!(key.origin, AppKeyOrigin::App);
    assert_eq!(key.label, LABEL);

    // The only device moved, so the Mac lets go of the old key.
    until("the old key let go", || keys.view().previous.is_none()).await;
    assert!(!keys.is_own(&testing::public_key()));
}
