// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Two or three `maxd` peer links in one process, joined by in-memory
//! streams in place of SSH.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_client::pinned::{PeerDial, PeerStream, Pins};
use maxhq_client::Client;
use maxhq_core::prove::{self, Challenge, Kind, Statement};
use maxhq_protocol::agents::{
    Actor, AgentMoveStep, ClientRole, LeaveContainer, Limits, PeerBook, PeerCall, PeerEntry,
    PeerRequest, PeerRoute, PeerWake, Permission, ToolAnswer, ToolCall, WakeTurn, DEPTH,
};
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{
    AgentId, BootId, ClientId, CredentialId, DeviceId, IdempotencyKey, InstanceId, NoteId,
    PermissionId, ProjectId, TerminalId, TransferId,
};
use maxhq_protocol::local_models::{ModelLoad, ServerModel};
use maxhq_protocol::message::{
    ActionOutcome, Capabilities, ClientHello, ClientInfo, ErrorCode, ServerHello,
};
use maxhq_protocol::model::Vendor;
use maxhq_protocol::notes::VersionHash;
use maxhq_protocol::secret::Secret;
use maxhq_protocol::terminal::TerminalKind;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::*;
use crate::local_models::linked;
use crate::local_models::runner::tests::{self as runner_tests, KEY, MODEL};
use crate::local_models::runner::Runner;
use crate::notes::tests as note_tests;
use crate::session::clock::FakeClock;

/// How long a test waits for what it expects. It only runs out when a test
/// fails, and a full test run on a busy machine can take tens of seconds to
/// fill a link that nobody reads.
const WAIT: Duration = Duration::from_secs(60);

/// A host key that parses, the same for every device in these tests.
const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

/// What a device's `maxd` runs for its peers: it counts calls, answers
/// with who called, and holds each call while `gate` is shut.
#[derive(Default)]
struct Served {
    calls: AtomicUsize,
    wakes: Mutex<Vec<(DeviceId, PeerWake)>>,
    moves: Mutex<Vec<(DeviceId, AgentMoveStep)>>,
    gate: Mutex<Option<Arc<Notify>>>,
    targets: Mutex<Vec<(DeviceId, Vec<AgentId>)>>,
}

impl Serve for Served {
    fn call(&self, from: DeviceId, call: PeerCall) -> BoxFuture<'_, ToolAnswer> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            ToolAnswer {
                text: format!("{} from {from}", call.call.tool),
                failed: false,
                images: Vec::new(),
            }
        })
    }

    fn wake(&self, from: DeviceId, wake: PeerWake) -> BoxFuture<'_, Result<(), String>> {
        self.wakes.lock().unwrap().push((from, wake));
        Box::pin(async { Ok(()) })
    }

    fn task_targets(&self, from: DeviceId, agents: Vec<AgentId>) -> Result<(), String> {
        self.targets.lock().unwrap().push((from, agents));
        Ok(())
    }

    fn move_step(&self, from: DeviceId, step: AgentMoveStep) -> BoxFuture<'_, ActionOutcome> {
        self.moves.lock().unwrap().push((from, step));
        Box::pin(async { ActionOutcome::Done })
    }
}

/// The devices in a test, and the links into each.
#[derive(Clone, Default)]
struct Net(Arc<NetInner>);

#[derive(Default)]
struct NetInner {
    devices: Mutex<HashMap<DeviceId, Peers>>,
    /// The device each one says hello as, when it isn't itself.
    claims: Mutex<HashMap<DeviceId, DeviceId>>,
    /// Each link's far end, to cut it.
    open: Mutex<Vec<(maxhq_wire::Sender, JoinHandle<()>)>>,
    dials: AtomicUsize,
    /// The devices whose `maxd` is from before peer proofs, with how many
    /// proofs each was sent.
    old: Mutex<HashMap<DeviceId, Arc<AtomicUsize>>>,
}

impl Net {
    /// Ends every link, as a dropped network would.
    fn cut(&self) {
        for (wire, task) in self.0.open.lock().unwrap().drain(..) {
            wire.close();
            task.abort();
        }
    }
}

/// Dials for `from`: the route host names the device, and the stream
/// goes into that device's [`serve`].
struct Dial {
    net: Net,
    from: DeviceId,
}

impl PeerDial for Dial {
    fn dial(&self, host: &str) -> BoxFuture<'static, io::Result<PeerStream>> {
        let net = self.net.clone();
        let from = self.from;
        let host = host.to_owned();
        Box::pin(async move {
            net.0.dials.fetch_add(1, Ordering::SeqCst);
            let to = net
                .0
                .devices
                .lock()
                .unwrap()
                .iter()
                .find(|(device, _)| host.starts_with(&format!("maxhq-peer-{device}-")))
                .map(|(device, peers)| (*device, peers.clone()));
            let Some((to, peers)) = to else {
                return Err(io::Error::other("no such device"));
            };
            let says = net.0.claims.lock().unwrap().get(&to).copied().unwrap_or(to);
            let old = net.0.old.lock().unwrap().get(&to).cloned();
            let (near, far) = tokio::io::duplex(1 << 16);
            let (reader, writer) = tokio::io::split(far);
            let (wire, _) = maxhq_wire::spawn(writer);
            let task = match old {
                Some(proofs) => tokio::spawn(before_proofs(reader, wire.clone(), says, proofs)),
                None => tokio::spawn(serve(
                    peers,
                    from,
                    reader,
                    FrameDecoder::new(),
                    wire.clone(),
                    hello(says),
                )),
            };
            net.0.open.lock().unwrap().push((wire, task));
            let (reader, writer) = tokio::io::split(near);
            Ok((Box::new(reader) as _, Box::new(writer) as _))
        })
    }
}

fn hello(device: DeviceId) -> ServerHello {
    ServerHello {
        protocol: ProtocolRange::OURS,
        maxd_version: "0.0.0".into(),
        device,
        boot_id: BootId("boot".into()),
        instance: InstanceId::generate(),
        capabilities: Capabilities {
            agent_tools: true,
            peers: true,
            peer_proofs: true,
            ..Capabilities::default()
        },
        build: None,
        target: None,
        challenge: Challenge::generate().map(|challenge| challenge.encode()),
    }
}

/// What a `maxd` from before peer proofs did with a link: it took the
/// hello, which had a challenge for the app's proof, served calls with no
/// proof, and turned a proof down as a message a peer doesn't send. It
/// counts the proofs that came.
async fn before_proofs(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    wire: maxhq_wire::Sender,
    device: DeviceId,
    proofs: Arc<AtomicUsize>,
) {
    let mut said = hello(device);
    said.capabilities.peer_proofs = false;
    let control = Control::new(wire.clone());
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0; 4096];
    loop {
        while let Some(Frame::Message(json)) = decoder.next_frame().unwrap() {
            let reply = match serde_json::from_slice(json).unwrap() {
                ClientMessage::Hello(_) => ServerMessage::Hello(said.clone()),
                ClientMessage::Ping { nonce } => ServerMessage::Pong { nonce },
                ClientMessage::Prove { .. } => {
                    proofs.fetch_add(1, Ordering::SeqCst);
                    ServerMessage::Error {
                        code: ErrorCode::NotAllowed,
                        message: "a link from another device carries only agent calls and wakes"
                            .into(),
                    }
                }
                ClientMessage::PeerRequest { key, .. } => ServerMessage::ActionResult {
                    key,
                    outcome: ActionOutcome::Tool {
                        answer: ToolAnswer {
                            text: "answered without a proof".into(),
                            failed: false,
                            images: Vec::new(),
                        },
                    },
                },
                _ => continue,
            };
            control.send(&reply);
        }
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => decoder.push(&buf[..n]),
        }
    }
    wire.close();
}

/// One device in a test.
struct Device {
    id: DeviceId,
    peers: Peers,
    served: Arc<Served>,
    clock: Arc<FakeClock>,
    db: Db,
    dir: PathBuf,
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Device {
    fn new(net: &Net, name: &str) -> Self {
        let id = DeviceId::derive(name);
        let dir = std::env::temp_dir().join(format!("maxhq-peers-{name}-{}", uuid::Uuid::now_v7()));
        let files = Files {
            ssh: dir.join(".config/maxhq/ssh"),
            home: dir.clone(),
        };
        let db = Db::open_in_memory().unwrap();
        let clock = Arc::new(FakeClock::new());
        let client = Client::peers_over(Arc::new(Dial {
            net: net.clone(),
            from: id,
        }));
        let peers = Peers::new(
            id,
            files,
            db.clone(),
            clock.clone(),
            client,
            Pins::default(),
        );
        let served = Arc::new(Served::default());
        peers.serve_with(served.clone());
        net.0.devices.lock().unwrap().insert(id, peers.clone());
        Self {
            id,
            peers,
            served,
            clock,
            db,
            dir,
        }
    }

    /// The seed of this device's own key, which its links prove with.
    fn seed(&self) -> zeroize::Zeroizing<[u8; 32]> {
        self.peers.public_key().unwrap();
        let folder = Folder::open(self.peers.0.files.ssh.clone()).unwrap();
        key::seed(&folder).unwrap().expect("a key")
    }

    /// Lets this device link to `others`, which take SSH logins when
    /// `dials` says so, and lets their keys in, as pairing them does.
    fn knows(&self, ssh: bool, others: &[(&Device, bool)]) {
        for (other, _) in others {
            let public_key = other.peers.public_key().unwrap();
            self.peers.authorize(other.id, &public_key).unwrap();
        }
        let peers = others
            .iter()
            .map(|(other, dials)| PeerEntry {
                device: other.id,
                name: format!("device {}", other.id),
                routes: if *dials {
                    vec![PeerRoute {
                        host: "devbox.example.ts.net".into(),
                        port: 22,
                        user: "dev".into(),
                    }]
                } else {
                    Vec::new()
                },
                host_key: HOST_KEY.into(),
                test_kinds: Vec::new(),
                home: false,
                model_server: None,
                models_from: Vec::new(),
            })
            .collect();
        self.peers.set_book(PeerBook { ssh, peers }).unwrap();
    }

    /// Marks `home` as the device that runs the MaxHQ app, in this
    /// device's book.
    fn homed_at(&self, home: &Device) {
        let mut book = self.peers.lock().book.clone();
        for peer in &mut book.peers {
            peer.home = peer.device == home.id;
        }
        self.peers.set_book(book).unwrap();
    }

    fn outgoing(&self) -> usize {
        self.peers.lock().out.len()
    }
}

fn call(tool: &str) -> PeerCall {
    PeerCall {
        by: Actor::Agent {
            agent: AgentId::derive("caller"),
        },
        permission: PermissionId::derive("p"),
        level: 0,
        call: ToolCall {
            tool: tool.into(),
            arguments: json!({}),
        },
    }
}

async fn within<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .unwrap_or_else(|_| panic!("{what} took too long"))
}

#[tokio::test]
async fn a_call_reaches_the_other_device_and_answers() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);

    let answer = within("the call", a.peers.call(b.id, call("list_agents"))).await;
    assert_eq!(
        answer,
        Ok(ToolAnswer {
            text: format!("list_agents from {}", a.id),
            failed: false,
            images: Vec::new(),
        })
    );
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 1);

    // A second call takes the same link.
    within("the second call", a.peers.call(b.id, call("read_agent")))
        .await
        .unwrap();
    assert_eq!(net.0.dials.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_device_cant_read_anothers_answer_by_its_key() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let key = IdempotencyKey::generate();
    let request = || PeerRequest::Call {
        call: call("read_agent"),
    };
    let text = |outcome: ActionOutcome| match outcome {
        ActionOutcome::Tool { answer } => answer.text,
        other => panic!("{other:?}"),
    };

    let (first, second) = (DeviceId::derive("first"), DeviceId::derive("second"));
    let theirs = text(a.peers.answer(first, key, request()).await);
    assert_eq!(theirs, format!("read_agent from {first}"));

    // The same key from another device runs for that device.
    let other = text(a.peers.answer(second, key, request()).await);
    assert_eq!(other, format!("read_agent from {second}"));
    assert_eq!(a.served.calls.load(Ordering::SeqCst), 2);

    // Sent again by the first device, it still answers once.
    assert_eq!(text(a.peers.answer(first, key, request()).await), theirs);
    assert_eq!(a.served.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_device_not_in_the_book_is_unreachable() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let answer = a.peers.call(b.id, call("list_agents")).await;
    assert!(matches!(answer, Err(Unreachable(reason)) if reason.contains("isn't one")));
}

#[tokio::test]
async fn a_wait_survives_a_dropped_link_and_runs_once() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    let gate = Arc::new(Notify::new());
    *b.served.gate.lock().unwrap() = Some(gate.clone());

    let peers = a.peers.clone();
    let to = b.id;
    let waiting = tokio::spawn(async move { peers.call(to, call("wait_agents")).await });
    within("the call to arrive", async {
        while b.served.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    net.cut();
    // The link dials again and asks again under the same key.
    within("the redial", async {
        while net.0.dials.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    // The wait holds the link open while it's out.
    a.clock.advance(IDLE * 2);
    a.peers.sweep();
    assert_eq!(a.outgoing(), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    gate.notify_waiters();
    let answer = within("the answer", waiting).await.unwrap().unwrap();
    assert_eq!(answer.text, format!("wait_agents from {}", a.id));
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 1, "it ran once");
}

#[tokio::test]
async fn an_idle_link_closes_after_ten_minutes() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    within("the call", a.peers.call(b.id, call("list_agents")))
        .await
        .unwrap();

    a.clock.advance(IDLE - Duration::from_secs(1));
    a.peers.sweep();
    assert_eq!(a.outgoing(), 1);
    a.clock.advance(Duration::from_secs(1));
    a.peers.sweep();
    assert_eq!(a.outgoing(), 0);
    // The next call dials again.
    within("the next call", a.peers.call(b.id, call("list_agents")))
        .await
        .unwrap();
    assert_eq!(net.0.dials.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn when_both_dial_the_lower_devices_link_wins() {
    let net = Net::default();
    let mut one = Device::new(&net, "one");
    let mut two = Device::new(&net, "two");
    if two.id < one.id {
        std::mem::swap(&mut one, &mut two);
    }
    let (low, high) = (one, two);
    low.knows(true, &[(&high, true)]);
    high.knows(true, &[(&low, true)]);

    let (up, down) = tokio::join!(
        low.peers.call(high.id, call("from_low")),
        high.peers.call(low.id, call("from_high")),
    );
    up.unwrap();
    down.unwrap();
    // Once both links are up, the higher device's own closes as soon as
    // nothing uses it, and the lower device's stays.
    within("both links", async {
        while !(high.peers.lock().incoming.contains_key(&low.id)
            && low.peers.lock().incoming.contains_key(&high.id))
        {
            tokio::task::yield_now().await;
        }
    })
    .await;
    high.peers.sweep();
    low.peers.sweep();
    assert_eq!(high.outgoing(), 0);
    assert_eq!(low.outgoing(), 1);

    // The higher device's calls now go back over the lower one's link.
    let dials = net.0.dials.load(Ordering::SeqCst);
    let answer = within("a call back", high.peers.call(low.id, call("again")))
        .await
        .unwrap();
    assert_eq!(answer.text, format!("again from {}", high.id));
    assert_eq!(net.0.dials.load(Ordering::SeqCst), dials);
    assert_eq!(high.outgoing(), 0);
}

#[tokio::test]
async fn a_device_that_takes_no_ssh_holds_a_link_to_its_permissions_home() {
    let net = Net::default();
    // The laptop takes no SSH logins; the devbox does.
    let laptop = Device::new(&net, "laptop");
    let devbox = Device::new(&net, "devbox");
    laptop.knows(false, &[(&devbox, true)]);
    devbox.knows(true, &[(&laptop, false)]);

    // With no link from the laptop, the devbox can't reach it.
    let answer = devbox.peers.call(laptop.id, call("list_agents")).await;
    assert!(matches!(answer, Err(Unreachable(reason)) if reason.contains("takes no SSH")));

    // A permission at home on the devbox that names the laptop.
    laptop
        .db
        .put_permission(&Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Agent {
                agent: AgentId::derive("lead"),
            },
            home: devbox.id,
            devices: vec![devbox.id, laptop.id],
            vendors: vec![Vendor::Claude],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Ask,
        })
        .unwrap();
    laptop.peers.sweep();
    within("the held link", async {
        while !devbox.peers.lock().incoming.contains_key(&laptop.id) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let answer = within(
        "the call",
        devbox.peers.call(laptop.id, call("list_agents")),
    )
    .await
    .unwrap();
    assert_eq!(answer.text, format!("list_agents from {}", devbox.id));

    // A held link never closes as idle.
    laptop.clock.advance(IDLE * 3);
    laptop.peers.sweep();
    assert_eq!(laptop.outgoing(), 1);
}

#[tokio::test]
async fn a_wake_reaches_the_callers_device() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    let wake = PeerWake {
        permission: PermissionId::derive("p"),
        parent: AgentId::derive("parent"),
        agent: AgentId::derive("child"),
        title: "Fix the tests".into(),
        vendor: Vendor::Codex,
        device_name: "devbox".into(),
        turn: WakeTurn::Finished,
    };
    within("the wake", PeerCalls::wake(&b.peers, a.id, wake.clone()))
        .await
        .unwrap();
    assert_eq!(*a.served.wakes.lock().unwrap(), [(b.id, wake)]);
}

#[tokio::test]
async fn the_agents_a_devices_tasks_reach_reach_that_device() {
    use crate::tasks::targets::Reach;

    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    let agent = AgentId::derive("watched");
    within("the targets", Reach::tell(&b.peers, a.id, vec![agent]))
        .await
        .unwrap();
    assert_eq!(*a.served.targets.lock().unwrap(), [(b.id, vec![agent])]);
}

#[tokio::test]
async fn only_the_device_that_runs_the_app_moves_agents() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    let step = AgentMoveStep::Targets;
    let refused = within("the step", a.peers.move_step(b.id, step.clone())).await;
    assert!(
        matches!(&refused, Ok(ActionOutcome::Refused { reason }) if reason.contains("runs the MaxHQ app")),
        "{refused:?}"
    );
    assert!(b.served.moves.lock().unwrap().is_empty());
    assert_eq!(PeerCalls::home(&b.peers), None);

    b.homed_at(&a);
    assert_eq!(PeerCalls::home(&b.peers), Some(a.id));
    let done = within("the step", a.peers.move_step(b.id, step.clone())).await;
    assert_eq!(done, Ok(ActionOutcome::Done));
    assert_eq!(*b.served.moves.lock().unwrap(), [(a.id, step)]);
}

#[tokio::test]
async fn a_link_that_says_hello_as_another_device_is_refused() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    net.0
        .claims
        .lock()
        .unwrap()
        .insert(b.id, DeviceId::derive("someone else"));

    let peers = a.peers.clone();
    let to = b.id;
    let call = tokio::spawn(async move { peers.call(to, call("list_agents")).await });
    within("a dial", async {
        while net.0.dials.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !*a.peers.lock().out[&b.id].up.borrow(),
        "the link never came up"
    );
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 0);
    call.abort();
}

#[tokio::test]
async fn the_book_writes_pinned_host_keys_and_refuses_a_bad_one() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    let file = a.dir.join(".config/maxhq/ssh").join(KNOWN_HOSTS);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("maxhq-dev-{} {HOST_KEY}\n", b.id)
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let refused = a.peers.set_book(PeerBook {
        ssh: true,
        peers: vec![PeerEntry {
            device: b.id,
            name: "devbox".into(),
            routes: Vec::new(),
            host_key: "ssh-rsa AAAAnope".into(),
            test_kinds: Vec::new(),
            home: false,
            model_server: None,
            models_from: Vec::new(),
        }],
    });
    assert!(refused.is_err());
    // The book the database keeps is the last good one.
    assert_eq!(
        a.db.peer_book().unwrap().unwrap().peers[0].host_key,
        HOST_KEY
    );
}

#[tokio::test]
async fn a_device_with_no_routes_needs_no_host_key() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let book = |routes: Vec<PeerRoute>| PeerBook {
        ssh: true,
        peers: vec![PeerEntry {
            device: b.id,
            name: "laptop".into(),
            routes,
            host_key: String::new(),
            test_kinds: Vec::new(),
            home: false,
            model_server: None,
            models_from: Vec::new(),
        }],
    };
    a.peers.set_book(book(Vec::new())).expect("taken");
    let route = PeerRoute {
        host: "laptop".into(),
        port: 22,
        user: "dev".into(),
    };
    assert!(a.peers.set_book(book(vec![route])).is_err());
}

#[tokio::test]
async fn authorize_and_revoke_touch_only_that_devices_line() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = DeviceId::derive("b");
    let file = a.dir.join(".ssh/authorized_keys");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "ssh-rsa AAAAown me@laptop\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(a.peers.authorize(b, "not a key").is_err());
    a.peers.authorize(b, HOST_KEY).unwrap();
    a.peers.authorize(b, HOST_KEY).unwrap();
    let home = a.dir.display();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!(
            "ssh-rsa AAAAown me@laptop\nrestrict,command=\"{home}/.local/bin/maxd proxy --peer {b}\" {HOST_KEY} maxhq-peer-{b}\n"
        )
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o644
    );
    a.peers.revoke(b).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ssh-rsa AAAAown me@laptop\n"
    );
}

/// A raw connection into a device's [`serve`], as `maxd proxy --peer`
/// gives one.
struct Raw {
    stream: DuplexStream,
    decoder: FrameDecoder,
    /// What the device says hello with, which a proof signs.
    hello: ServerHello,
}

impl Raw {
    fn open(to: &Device, from: DeviceId) -> Self {
        let (near, far) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(far);
        let (wire, _) = maxhq_wire::spawn(writer);
        let hello = hello(to.id);
        tokio::spawn(serve(
            to.peers.clone(),
            from,
            reader,
            FrameDecoder::new(),
            wire,
            hello.clone(),
        ));
        Self {
            stream: near,
            decoder: FrameDecoder::new(),
            hello,
        }
    }

    /// A proof signed with `seed`, as `kind`, by the client [`client_hello`]
    /// names.
    fn proof(&self, seed: &[u8; 32], kind: Kind) -> (IdempotencyKey, ClientMessage) {
        let statement = Statement {
            kind,
            device: self.hello.device,
            instance: self.hello.instance,
            client: raw_client(),
            challenge: Challenge::parse(self.hello.challenge.as_deref().unwrap()).unwrap(),
        };
        let proof = prove::sign(seed, &statement).unwrap();
        let key = IdempotencyKey::generate();
        let message = ClientMessage::Prove {
            key,
            public_key: proof.public_key,
            signature: proof.signature,
        };
        (key, message)
    }

    /// Sends a proof signed with `seed`, as `kind`, and gives the answer.
    async fn prove(&mut self, seed: &[u8; 32], kind: Kind) -> ActionOutcome {
        let (key, proof) = self.proof(seed, kind);
        self.send(&proof).await;
        match self.recv().await {
            Some(ServerMessage::ActionResult {
                key: answered,
                outcome,
            }) if answered == key => outcome,
            other => panic!("{other:?}"),
        }
    }

    async fn send(&mut self, message: &impl serde::Serialize) {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message).unwrap();
        self.stream.write_all(&out).await.unwrap();
    }

    /// The next message, or none once the connection closes.
    async fn recv(&mut self) -> Option<ServerMessage> {
        let mut buf = [0; 4096];
        loop {
            if let Some(Frame::Message(json)) = self.decoder.next_frame().unwrap() {
                return Some(serde_json::from_slice(json).unwrap());
            }
            let n = within("a message", self.stream.read(&mut buf))
                .await
                .unwrap();
            if n == 0 {
                return None;
            }
            self.decoder.push(&buf[..n]);
        }
    }

    async fn refused(&mut self) -> ErrorCode {
        match self.recv().await {
            Some(ServerMessage::Error { code, .. }) => code,
            other => panic!("{other:?}"),
        }
    }
}

/// The client ID a raw connection says hello with.
fn raw_client() -> ClientId {
    ClientId::derive("raw peer link")
}

fn client_hello(role: ClientRole) -> ClientMessage {
    ClientMessage::Hello(ClientHello {
        protocol: ProtocolRange::OURS,
        client: ClientInfo {
            name: "maxd".into(),
            version: "0.0.0".into(),
        },
        client_id: raw_client(),
        role,
    })
}

#[tokio::test]
async fn a_peer_link_carries_only_calls_wakes_and_answers() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    b.knows(true, &[(&a, true)]);
    let mut raw = Raw::open(&b, a.id);
    raw.send(&ClientMessage::Sync { since: None }).await;
    assert_eq!(raw.refused().await, ErrorCode::NoHello);
    raw.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(raw.recv().await, Some(ServerMessage::Hello(hello)) if hello.device == b.id));
    assert_eq!(raw.prove(&a.seed(), Kind::Peer).await, ActionOutcome::Done);

    for refused in [
        ClientMessage::Sync { since: None },
        ClientMessage::ForgetCredential {
            key: IdempotencyKey::generate(),
            credential: CredentialId::generate(),
        },
        ClientMessage::SetPeerBook {
            key: IdempotencyKey::generate(),
            book: PeerBook::default(),
        },
        ClientMessage::PeerKey {
            key: IdempotencyKey::generate(),
        },
        ClientMessage::RevokePeer {
            key: IdempotencyKey::generate(),
            device: a.id,
        },
        ClientMessage::FetchBegin {
            key: IdempotencyKey::generate(),
            transfer: TransferId::generate(),
            agent: AgentId::derive("lead"),
            path: "/tmp/example/plan.md".into(),
            anyway: true,
        },
        ClientMessage::ListFolder {
            key: IdempotencyKey::generate(),
            device: b.id,
            path: "/".into(),
            files: true,
            hidden: true,
            filter: None,
        },
        ClientMessage::MakeFolder {
            key: IdempotencyKey::generate(),
            device: b.id,
            path: "/tmp/example/new".into(),
        },
        ClientMessage::Notices {
            key: IdempotencyKey::generate(),
            device: b.id,
            call: maxhq_protocol::notices::NoticeCall::Clear,
        },
        ClientMessage::CursorRules {
            key: IdempotencyKey::generate(),
            device: b.id,
            call: maxhq_protocol::cursor_rules::CursorRuleCall::List,
        },
    ]
    .into_iter()
    .chain(terminal_requests())
    {
        raw.send(&refused).await;
        assert_eq!(raw.refused().await, ErrorCode::NotAllowed, "{refused:?}");
    }
    raw.send(&client_hello(ClientRole::Peer)).await;
    assert_eq!(raw.refused().await, ErrorCode::BadMessage);

    // Keystrokes for a terminal go nowhere: the next answer is the pong.
    let mut typed = Vec::new();
    frame::encode_terminal_input(&mut typed, TerminalId::derive("shell"), b"whoami\r");
    raw.stream.write_all(&typed).await.unwrap();
    raw.send(&ClientMessage::Ping { nonce: 7 }).await;
    assert_eq!(raw.recv().await, Some(ServerMessage::Pong { nonce: 7 }));
    let key = IdempotencyKey::generate();
    raw.send(&ClientMessage::PeerRequest {
        key,
        request: PeerRequest::Call {
            call: call("list_agents"),
        },
    })
    .await;
    match raw.recv().await {
        Some(ServerMessage::ActionResult {
            key: answered,
            outcome: ActionOutcome::Tool { answer },
        }) => {
            assert_eq!(answered, key);
            assert_eq!(answer.text, format!("list_agents from {}", a.id));
        }
        other => panic!("{other:?}"),
    }
}

/// Every request a connection makes of a terminal, for a shell.
fn terminal_requests() -> [ClientMessage; 8] {
    let terminal = TerminalId::derive("shell");
    let key = IdempotencyKey::generate;
    [
        ClientMessage::TerminalOpen {
            key: key(),
            agent: AgentId::derive("lead"),
            kind: TerminalKind::Shell,
            cols: 80,
            rows: 24,
        },
        ClientMessage::TerminalAttach {
            terminal,
            after: None,
        },
        ClientMessage::TerminalDetach { terminal },
        ClientMessage::TerminalResize {
            terminal,
            cols: 100,
            rows: 30,
        },
        ClientMessage::TerminalInterrupt {
            key: key(),
            terminal,
        },
        ClientMessage::TerminalClose {
            key: key(),
            terminal,
            force: true,
        },
        ClientMessage::TerminalRename {
            key: key(),
            terminal,
            title: "mine".into(),
        },
        ClientMessage::TerminalMove {
            key: key(),
            terminal,
        },
    ]
}

/// A connection from `a` to `b` that has said hello and proved `a`'s key.
async fn greeted(a: &Device, b: &Device) -> Raw {
    b.knows(true, &[(a, true)]);
    let mut raw = Raw::open(b, a.id);
    raw.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(raw.recv().await, Some(ServerMessage::Hello(_))));
    assert_eq!(raw.prove(&a.seed(), Kind::Peer).await, ActionOutcome::Done);
    raw
}

/// The answer to a call on `raw`.
async fn call_on(raw: &mut Raw, tool: &str) -> ActionOutcome {
    let key = IdempotencyKey::generate();
    raw.send(&ClientMessage::PeerRequest {
        key,
        request: PeerRequest::Call { call: call(tool) },
    })
    .await;
    match raw.recv().await {
        Some(ServerMessage::ActionResult {
            key: answered,
            outcome,
        }) if answered == key => outcome,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_peer_frame_without_a_proof_starts_no_agent_and_makes_no_link() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    b.knows(true, &[(&a, true)]);
    // Any program on b can write a Peer frame naming a, and say hello.
    let mut forged = Raw::open(&b, a.id);
    forged.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(forged.recv().await, Some(ServerMessage::Hello(_))));

    assert_eq!(
        call_on(&mut forged, "start_agent").await,
        ActionOutcome::Refused {
            reason: serve::UNPROVEN.into()
        }
    );
    // An answer to a request it was never sent goes nowhere.
    forged
        .send(&ClientMessage::PeerAnswer {
            key: IdempotencyKey::generate(),
            outcome: ActionOutcome::Done,
        })
        .await;
    forged.send(&ClientMessage::Ping { nonce: 1 }).await;
    assert_eq!(forged.recv().await, Some(ServerMessage::Pong { nonce: 1 }));
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 0);
    assert!(!b.peers.lock().incoming.contains_key(&a.id));
}

#[tokio::test]
async fn a_large_message_before_the_proof_is_refused_unread() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    b.knows(true, &[(&a, true)]);
    let mut raw = Raw::open(&b, a.id);
    raw.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(raw.recv().await, Some(ServerMessage::Hello(_))));

    let pad = "x".repeat(serve::MAX_UNPROVEN);
    raw.send(&json!({ "type": "ping", "nonce": 1, "pad": pad }))
        .await;
    assert_eq!(raw.refused().await, ErrorCode::BadMessage);
    // A large call from a `maxd` from before proofs still hears why.
    let key = IdempotencyKey::generate();
    let call =
        json!({ "type": "peer_request", "key": key, "request": { "kind": "call", "pad": pad } });
    raw.send(&call).await;
    assert_eq!(
        raw.recv().await,
        Some(ServerMessage::ActionResult {
            key,
            outcome: ActionOutcome::Refused {
                reason: serve::UNPROVEN.into()
            },
        })
    );

    // The link still proves its key, and then reads more.
    assert_eq!(raw.prove(&a.seed(), Kind::Peer).await, ActionOutcome::Done);
    raw.send(&json!({ "type": "ping", "nonce": 2, "pad": pad }))
        .await;
    assert_eq!(raw.recv().await, Some(ServerMessage::Pong { nonce: 2 }));
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_link_this_device_stops_trusting_takes_no_more_calls() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let mut raw = greeted(&a, &b).await;
    b.peers.revoke(a.id).unwrap();
    raw.send(&a_request()).await;
    assert_eq!(raw.recv().await, None);
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn only_a_proof_with_the_devices_own_key_opens_its_link() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let c = Device::new(&net, "c");
    b.knows(true, &[(&a, true), (&c, true)]);
    let mut raw = Raw::open(&b, a.id);
    raw.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(raw.recv().await, Some(ServerMessage::Hello(_))));

    // Another device's key, even one b lets in for that device.
    let unknown = ActionOutcome::Refused {
        reason: format!(
            "Unknown key: it isn't the key this device lets in for device {}.",
            a.id
        ),
    };
    assert_eq!(raw.prove(&c.seed(), Kind::Peer).await, unknown);
    assert_eq!(raw.prove(&[5; 32], Kind::Peer).await, unknown);
    // The right key, signed as another kind of connection.
    let mismatch = ActionOutcome::Refused {
        reason: "Bad proof: the signature doesn't match.".into(),
    };
    assert_eq!(raw.prove(&a.seed(), Kind::App).await, mismatch);
    // The right key, signed for another connection's challenge.
    let other = Raw::open(&b, a.id);
    let (key, proof) = other.proof(&a.seed(), Kind::Peer);
    raw.send(&proof).await;
    assert_eq!(
        raw.recv().await,
        Some(ServerMessage::ActionResult {
            key,
            outcome: mismatch
        })
    );
    assert!(matches!(
        call_on(&mut raw, "list_agents").await,
        ActionOutcome::Refused { .. }
    ));
    assert!(!b.peers.lock().incoming.contains_key(&a.id));

    assert_eq!(raw.prove(&a.seed(), Kind::Peer).await, ActionOutcome::Done);
    assert!(b.peers.lock().incoming.contains_key(&a.id));
    assert!(matches!(
        call_on(&mut raw, "list_agents").await,
        ActionOutcome::Tool { .. }
    ));
    assert_eq!(b.served.calls.load(Ordering::SeqCst), 1);

    // Once b takes a's key out, its proof no longer counts.
    b.peers.revoke(a.id).unwrap();
    let mut again = Raw::open(&b, a.id);
    again.send(&client_hello(ClientRole::Peer)).await;
    assert!(matches!(again.recv().await, Some(ServerMessage::Hello(_))));
    assert_eq!(again.prove(&a.seed(), Kind::Peer).await, unknown);
}

#[tokio::test]
async fn a_device_proves_only_to_a_maxd_that_takes_peer_proofs() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let proofs = Arc::new(AtomicUsize::new(0));
    net.0.old.lock().unwrap().insert(b.id, proofs.clone());
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);

    let answer = within("the call", a.peers.call(b.id, call("list_agents"))).await;
    assert_eq!(answer.unwrap().text, "answered without a proof");
    assert_eq!(proofs.load(Ordering::SeqCst), 0);
}

fn a_request() -> ClientMessage {
    ClientMessage::PeerRequest {
        key: IdempotencyKey::generate(),
        request: PeerRequest::Call { call: call("wait") },
    }
}

/// Writes `count` of a message to a connection in batches, and says how many
/// it wrote before the far side stopped reading for two seconds or closed.
async fn write_until_stalled(
    raw: &mut Raw,
    count: usize,
    message: impl Fn() -> ClientMessage,
) -> usize {
    const BATCH: usize = 1024;
    let mut written = 0;
    while written < count {
        let mut out = Vec::new();
        for _ in 0..BATCH {
            frame::encode_message(&mut out, &message()).unwrap();
        }
        let write = tokio::time::timeout(Duration::from_secs(2), raw.stream.write_all(&out));
        if !matches!(write.await, Ok(Ok(()))) {
            break;
        }
        written += BATCH;
    }
    written
}

#[tokio::test]
async fn a_connection_with_too_many_requests_running_closes() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let gate = Arc::new(Notify::new());
    *b.served.gate.lock().unwrap() = Some(gate.clone());
    let mut raw = greeted(&a, &b).await;

    for _ in 0..MAX_REQUESTS {
        raw.send(&a_request()).await;
    }
    // That many running at once is still fine.
    raw.send(&ClientMessage::Ping { nonce: 7 }).await;
    assert_eq!(raw.recv().await, Some(ServerMessage::Pong { nonce: 7 }));
    assert_eq!(b.served.calls.load(Ordering::SeqCst), MAX_REQUESTS);

    raw.send(&a_request()).await;
    assert_eq!(raw.recv().await, None);
    assert_eq!(b.served.calls.load(Ordering::SeqCst), MAX_REQUESTS);
    gate.notify_waiters();
}

#[tokio::test]
async fn a_finished_request_makes_room_for_the_next() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let mut raw = greeted(&a, &b).await;
    for _ in 0..(MAX_REQUESTS * 2) {
        raw.send(&a_request()).await;
        assert!(matches!(
            raw.recv().await,
            Some(ServerMessage::ActionResult { .. })
        ));
    }
}

#[tokio::test]
async fn answers_a_peer_never_reads_end_up_closing_its_connection() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let mut raw = greeted(&a, &b).await;
    assert!(b.peers.lock().incoming.contains_key(&a.id));
    // Each answer waits for room and keeps its place while it does.
    let writing = async {
        loop {
            let mut out = Vec::new();
            for _ in 0..1024 {
                frame::encode_message(&mut out, &a_request()).unwrap();
            }
            if raw.stream.write_all(&out).await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    };
    let closed = within("the connection to close", async {
        while b.peers.lock().incoming.contains_key(&a.id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    tokio::select! {
        () = writing => {}
        () = closed => {}
    }
}

#[tokio::test]
async fn pings_a_peer_never_reads_stall_its_own_connection() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let mut raw = greeted(&a, &b).await;
    let most = 1_000_000;
    let written = write_until_stalled(&mut raw, most, || ClientMessage::Ping { nonce: 7 }).await;
    assert!(written < most, "it read {written} pings and kept going");
    // The connection stays up, and whatever it held waits for the peer.
    assert!(b.peers.lock().incoming.contains_key(&a.id));
}

#[tokio::test]
async fn a_link_with_too_many_requests_sent_back_closes() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    a.peers.call(b.id, call("list_agents")).await.unwrap();
    assert_eq!(a.outgoing(), 1);

    let gate = Arc::new(Notify::new());
    *a.served.gate.lock().unwrap() = Some(gate.clone());
    let control = b.peers.lock().incoming[&a.id].control.clone();
    let request = || ServerMessage::PeerRequest {
        key: IdempotencyKey::generate(),
        request: PeerRequest::Call { call: call("wait") },
    };
    for _ in 0..MAX_REQUESTS {
        control.send(&request());
    }
    within("the requests to run", async {
        while a.served.calls.load(Ordering::SeqCst) < MAX_REQUESTS {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(a.outgoing(), 1, "that many running at once is still fine");

    control.send(&request());
    within("the link to close", async {
        while a.outgoing() == 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(a.served.calls.load(Ordering::SeqCst), MAX_REQUESTS);
    gate.notify_waiters();
}

#[tokio::test]
async fn control_messages_a_peer_never_reads_close_its_link() {
    // The far end never reads, so the link's queues fill and stay full.
    let (writer, _far) = tokio::io::duplex(64 * 1024);
    let (wire, _) = maxhq_wire::spawn(writer);
    let control = Control::new(wire.clone());
    let big = || ServerMessage::Error {
        code: ErrorCode::BadMessage,
        message: "x".repeat(1024 * 1024),
    };
    let mut sent = 0;
    while !wire.is_closed() {
        assert!(sent < 64, "{sent} big messages queued with nobody reading");
        control.send(&big());
        sent += 1;
        assert!(control.waiting.load(Ordering::Acquire) <= MAX_WAITING);
    }
    // Room for the queue's own budget plus what may wait behind it.
    assert!(
        sent > MAX_WAITING / (1024 * 1024),
        "closed after only {sent}"
    );
    // Once closed, more messages are dropped, not held.
    let held = control.waiting.load(Ordering::Acquire);
    control.send(&big());
    assert_eq!(control.waiting.load(Ordering::Acquire), held);
}

#[tokio::test]
async fn only_a_peer_hello_from_a_known_device_is_taken() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    b.knows(true, &[(&a, true)]);

    let mut app = Raw::open(&b, a.id);
    app.send(&client_hello(ClientRole::App)).await;
    assert_eq!(app.refused().await, ErrorCode::NotAllowed);
    assert_eq!(app.recv().await, None, "the connection closes");

    let mut stranger = Raw::open(&b, DeviceId::derive("stranger"));
    stranger.send(&client_hello(ClientRole::Peer)).await;
    assert_eq!(stranger.refused().await, ErrorCode::NotAllowed);
    assert_eq!(stranger.recv().await, None, "the connection closes");
}

/// Gives a device its notes, which swap with its peers from now on.
fn notes_on(device: &Device, name: &str) -> (crate::notes::Notes, crate::hub::Hub) {
    let hub = crate::notes::tests::hub_of(name);
    let notes = crate::notes::Notes::new(device.db.clone(), hub.clone());
    device.peers.notes_with(notes.clone());
    (notes, hub)
}

/// Waits until `notes` holds the version `hash` of `project`'s notes.
async fn holds(what: &str, notes: &crate::notes::Notes, project: ProjectId, hash: &VersionHash) {
    within(what, async {
        loop {
            let held = notes.hashes(project).await.unwrap();
            if held.contains(hash) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

#[tokio::test]
async fn a_save_reaches_peers_in_the_project_and_no_others() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    let c = Device::new(&net, "c");
    a.knows(true, &[(&b, true), (&c, true)]);
    b.knows(true, &[(&a, true), (&c, true)]);
    c.knows(true, &[(&a, true), (&b, true)]);
    let project = ProjectId::derive("project");
    let (at_a, _) = notes_on(&a, "a");
    let (at_b, hub_b) = notes_on(&b, "b");
    let (at_c, _) = notes_on(&c, "c");
    note_tests::join(&hub_b, project);

    let note = NoteId::generate();
    let (first, _) = at_a
        .save(note_tests::save(project, note, None, "one"))
        .await
        .unwrap();
    holds("the first save on b", &at_b, project, &first.hash).await;

    // A change on b comes back to a.
    let (second, _) = at_b
        .save(note_tests::save(project, note, Some(&first), "two"))
        .await
        .unwrap();
    holds("b's save on a", &at_a, project, &second.hash).await;
    assert_eq!(at_a.read(project, note, None).await.unwrap(), second);

    // c has no workspace in the project, so it keeps none of it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(at_c.hashes(project).await.unwrap().is_empty());
}

#[tokio::test]
async fn notes_swap_when_a_link_comes_up() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    // a can't dial b, so its save waits for b to call.
    a.knows(true, &[(&b, false)]);
    b.knows(true, &[(&a, true)]);
    let project = ProjectId::derive("project");
    let (at_a, _) = notes_on(&a, "a");
    let (at_b, hub_b) = notes_on(&b, "b");
    note_tests::join(&hub_b, project);
    let (saved, _) = at_a
        .save(note_tests::save(project, NoteId::generate(), None, "one"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(at_b.hashes(project).await.unwrap().is_empty());

    within("the call", b.peers.call(a.id, call("list_agents")))
        .await
        .unwrap();
    holds("the save on b", &at_b, project, &saved.hash).await;
}

#[tokio::test]
async fn a_failed_swap_tries_again() {
    let net = Net::default();
    let a = Device::new(&net, "a");
    let b = Device::new(&net, "b");
    a.knows(true, &[(&b, true)]);
    b.knows(true, &[(&a, true)]);
    let project = ProjectId::derive("project");
    let (at_a, _) = notes_on(&a, "a");
    let (at_b, hub_b) = notes_on(&b, "b");
    note_tests::join(&hub_b, project);

    // b can't be reached when a saves.
    let away = net.0.devices.lock().unwrap().remove(&b.id).unwrap();
    let (saved, _) = at_a
        .save(note_tests::save(project, NoteId::generate(), None, "one"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(at_b.hashes(project).await.unwrap().is_empty());
    net.0.devices.lock().unwrap().insert(b.id, away);
    holds("the save on b", &at_b, project, &saved.hash).await;
}

/// What a Mac serves linked devices' model streams with: its runner, on
/// a fake host, behind its switch.
struct Shares {
    runner: Arc<Runner>,
    server: ModelServerId,
    /// Why it refuses, while its switch is off.
    off: Mutex<Option<String>>,
    holds: Mutex<Vec<(DeviceId, Vec<String>)>>,
}

impl Shares {
    fn lets(&self, server: ModelServerId) -> Result<(), String> {
        if server != self.server {
            return Err(NO_MODELS.into());
        }
        self.off.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

impl Models for Shares {
    fn serve(
        &self,
        _from: DeviceId,
        server: ModelServerId,
        socket: DuplexStream,
    ) -> Result<(), String> {
        self.lets(server)?;
        self.runner.clone().serve_linked(socket);
        Ok(())
    }

    fn hold(&self, from: DeviceId, server: ModelServerId, models: &[String]) -> Result<(), String> {
        self.lets(server)?;
        self.holds.lock().unwrap().push((from, models.to_vec()));
        self.runner.hold(models);
        Ok(())
    }
}

/// The key a devbox's local agents reach its linked listener with.
const DEVBOX_KEY: &str = "devbox-linked-key-0123456789";

/// A devbox whose local agents use a Mac's managed models, through the
/// devbox's own listener on 127.0.0.1.
struct Sharing {
    net: Net,
    devbox: Device,
    mac: Device,
    shares: Arc<Shares>,
    host: Arc<runner_tests::Fakes>,
    /// Where a local agent on the devbox sends its turns.
    url: String,
}

impl Sharing {
    /// The two devices, the Mac taking SSH logins when `mac_dials` says
    /// so. Otherwise it holds a link to the devbox, whose agents use its
    /// models.
    async fn new(mac_dials: bool) -> Self {
        let net = Net::default();
        let devbox = Device::new(&net, "devbox");
        let mac = Device::new(&net, "mac");
        let (runner, host, _base) = runner_tests::runner().await;
        let server = ModelServerId::derive("model-server:mac:managed");
        let shares = Arc::new(Shares {
            runner,
            server,
            off: Mutex::default(),
            holds: Mutex::default(),
        });
        mac.peers.models_with(shares.clone());
        devbox.knows(true, &[(&mac, mac_dials)]);
        mac.knows(mac_dials, &[(&devbox, true)]);

        let mut book = devbox.peers.lock().book.clone();
        book.peers[0].model_server = Some(LinkedModels {
            server,
            label: "Mac".into(),
            models: vec![ServerModel {
                id: MODEL.into(),
                label: "Qwen".into(),
                catalog: None,
                context: Some(32_768),
                load: None,
                speed: None,
            }],
        });
        devbox.peers.set_book(book).unwrap();
        let mut book = mac.peers.lock().book.clone();
        book.peers[0].models_from = vec![mac.id];
        mac.peers.set_book(book).unwrap();

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!(
            "http://{}{}{server}/v1/messages?beta=true",
            listener.local_addr().unwrap(),
            linked::PREFIX
        );
        let listen = linked::Linked::new(
            Secret::new(DEVBOX_KEY.into()),
            Arc::new(linked::OverPeers(Some(devbox.peers.clone()))),
        );
        tokio::spawn(listen.serve(listener));
        Self {
            net,
            devbox,
            mac,
            shares,
            host,
            url,
        }
    }

    /// Sends a turn as a local agent on the devbox does, and gives the
    /// status, whether to retry, and the body.
    async fn turn(&self) -> (u16, Option<String>, serde_json::Value) {
        turn_at(self.url.clone()).await
    }

    /// Asks the Mac to keep the model loaded, as the devbox does while
    /// its agent works between requests.
    async fn hold(&self) -> Result<(), String> {
        within(
            "the hold",
            self.devbox
                .peers
                .hold_models(self.mac.id, self.shares.server, vec![MODEL.into()]),
        )
        .await
    }
}

/// Sends a turn to `url` with the devbox's key, and gives the status,
/// whether to retry, and the body.
async fn turn_at(url: String) -> (u16, Option<String>, serde_json::Value) {
    let answer = crate::secrets::http::client(false)
        .unwrap()
        .post(&url)
        .header("x-api-key", DEVBOX_KEY)
        .header("anthropic-version", "2023-06-01")
        .body(runner_tests::turn())
        .send()
        .await
        .unwrap();
    let status = answer.status().as_u16();
    let retry = answer
        .headers()
        .get("x-should-retry")
        .map(|value| value.to_str().unwrap().to_owned());
    let body = serde_json::from_slice(&answer.bytes().await.unwrap()).unwrap();
    (status, retry, body)
}

/// What a refused turn says.
fn reason(body: &serde_json::Value) -> &str {
    body["error"]["message"].as_str().unwrap()
}

#[tokio::test]
async fn a_devbox_agents_turn_crosses_the_link_to_the_macs_model() {
    let sharing = Sharing::new(true).await;
    let (status, _, body) = within("the turn", sharing.turn()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["content"][0]["text"], "hi");
    // The Mac loaded the model for it.
    assert_eq!(sharing.host.starts.load(Ordering::SeqCst), 1);
    let runner = &sharing.shares.runner;
    assert!(matches!(runner.load_of(MODEL), ModelLoad::Loaded { .. }));
    // llama-server got the Mac's own key, which never left the Mac, and
    // nothing of the devbox's.
    let seen = sharing.host.backend.seen();
    let sent = seen.last().unwrap();
    assert_eq!(sent.path, "/v1/messages?beta=true");
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_ne!(KEY, DEVBOX_KEY);
    assert_eq!(sent.header("x-api-key"), None);
    assert!(!sent.body.contains(DEVBOX_KEY));

    // A devbox agent still at work holds the model off its idle unload.
    let used = runner.last_used(MODEL);
    tokio::time::sleep(Duration::from_millis(20)).await;
    sharing.hold().await.unwrap();
    assert_eq!(
        *sharing.shares.holds.lock().unwrap(),
        [(sharing.devbox.id, vec![MODEL.to_owned()])]
    );
    assert!(runner.last_used(MODEL) > used);
    let idle = used + Duration::from_secs(15 * 60);
    assert!(!runner.sweep(idle), "the hold counts as use");
    assert!(matches!(runner.load_of(MODEL), ModelLoad::Loaded { .. }));
    assert!(runner.sweep(idle + Duration::from_secs(60)));
    assert_eq!(runner.load_of(MODEL), ModelLoad::Unloaded);
}

#[tokio::test]
async fn a_mac_that_takes_no_ssh_serves_its_models_over_the_link_it_holds() {
    let sharing = Sharing::new(false).await;
    let (devbox, mac) = (&sharing.devbox, &sharing.mac);
    within("the held link", async {
        while !devbox.peers.lock().incoming.contains_key(&mac.id) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let (status, _, body) = within("the turn", sharing.turn()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["content"][0]["text"], "hi");
    assert_eq!(sharing.host.starts.load(Ordering::SeqCst), 1);
    sharing.hold().await.unwrap();
    assert_eq!(sharing.shares.holds.lock().unwrap().len(), 1);
    // The link stays up while the devbox's agents use the Mac's models.
    mac.clock.advance(IDLE * 3);
    mac.peers.sweep();
    assert_eq!(mac.outgoing(), 1);
}

#[tokio::test]
async fn with_the_macs_switch_off_a_turn_is_refused_with_its_reason() {
    let sharing = Sharing::new(true).await;
    let off = "Mac doesn't let linked devices use its models.";
    *sharing.shares.off.lock().unwrap() = Some(off.into());
    let (status, retry, body) = within("the turn", sharing.turn()).await;
    assert_eq!(status, 502);
    assert_eq!(
        retry.as_deref(),
        Some("false"),
        "Claude Code doesn't try again"
    );
    assert_eq!(reason(&body), off);
    assert_eq!(
        sharing.host.starts.load(Ordering::SeqCst),
        0,
        "nothing loads"
    );
    assert_eq!(sharing.hold().await, Err(off.to_owned()));
    assert!(sharing.shares.holds.lock().unwrap().is_empty());

    // Turned back on, the next turn goes through.
    *sharing.shares.off.lock().unwrap() = None;
    let (status, _, body) = within("the next turn", sharing.turn()).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn a_turn_whose_link_drops_fails_with_a_plain_reason() {
    let sharing = Sharing::new(true).await;
    // The model takes a while to load, so the turn is still out when the
    // link goes.
    *sharing.host.halfway.lock().unwrap() = Some(Duration::from_secs(120));
    let turning = tokio::spawn(turn_at(sharing.url.clone()));
    within("the load to start", async {
        while sharing.host.starts.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let mac = sharing.mac.id;
    let away = sharing.net.0.devices.lock().unwrap().remove(&mac).unwrap();
    sharing.net.cut();
    let (status, retry, body) = within("the failed turn", turning).await.unwrap();
    assert_eq!(status, 502);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(reason(&body), format!("Lost the link to device {mac}."));

    // With the Mac still away, the next turn fails soon, and says why.
    let started = Instant::now();
    let (status, retry, body) = within("the next turn", sharing.turn()).await;
    assert_eq!(status, 502);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(reason(&body), format!("Can't reach device {mac}."));
    assert!(started.elapsed() < STREAM_WITHIN + Duration::from_secs(5));
    drop(away);
}
