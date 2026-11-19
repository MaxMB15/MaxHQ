// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Runs the built `maxd` binary against a socket, home folder and data
//! folder in a temp dir. Nothing here touches the real socket or installs a
//! service.

use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use maxhq_app::merge::Merge;
use maxhq_client::pinned::{PeerDial, PeerStream};
use maxhq_client::proof::{Prover, Proving, Verdict};
use maxhq_client::{LinkEvent, LinkSpec, TerminalSink, Transport};
use maxhq_core::prove::{self, Challenge, Kind, Statement};
use maxhq_hold::{Detach, Dirs, Event, Framing, Holds, Launch, TermLaunch, WriteOutcome};
use maxhq_protocol::agents::ClientRole;
use maxhq_protocol::frame::{self, Frame, FrameDecoder};
use maxhq_protocol::ids::{AgentId, ClientId, IdempotencyKey, LinkId, SessionId, TerminalId};
use maxhq_protocol::link::{DeviceLink, LinkState, Route};
use maxhq_protocol::message::{ActionOutcome, ClientHello, ClientInfo, ServerHello};
use maxhq_protocol::model::Device;
use maxhq_protocol::state::{Entity, EntityKey, Replica};
use maxhq_protocol::terminal::{TerminalKind, TerminalStatus};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::{ClientMessage, ProtocolRange, ServerMessage};

const BIN: &str = env!("CARGO_BIN_EXE_maxd");

/// How long a test waits for `maxd` before failing.
const WAIT: Duration = Duration::from_secs(10);

/// The seed of the app key these tests prove with.
const APP_SEED: [u8; 32] = [7; 32];

/// A temp folder holding one `maxd`'s world, removed when dropped. Its
/// name is short, since macOS allows socket paths of about 104 bytes.
struct World(PathBuf);

impl World {
    fn new() -> Self {
        loop {
            let id = uuid::Uuid::now_v7().simple().to_string();
            let path = std::env::temp_dir().join(format!("maxd-{}", &id[20..]));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("couldn't make {}: {err}", path.display()),
            }
        }
    }

    fn socket(&self) -> PathBuf {
        self.0.join("maxd.sock")
    }

    /// `maxd` with every path it uses pointed into this folder, and no
    /// observers, so it sees only this device.
    fn maxd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("MAXD_SOCKET", self.socket())
            .env("HOME", self.0.join("home"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("MAXD_NO_OBSERVERS", "1")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null());
        command
    }

    /// Starts `maxd run` and waits until its socket answers.
    fn start(&self) -> Running {
        self.run(self.maxd(&["run"]).stderr(Stdio::null()))
    }

    /// Starts `maxd run` logging all it can to `log`.
    fn start_logging(&self, log: &Path) -> Running {
        let log = File::options().create(true).append(true).open(log).unwrap();
        self.run(self.maxd(&["run"]).env("RUST_LOG", "trace").stderr(log))
    }

    fn run(&self, command: &mut Command) -> Running {
        let child = command.stdout(Stdio::null()).spawn().expect("maxd runs");
        let mut running = Running(child);
        let start = Instant::now();
        while UnixStream::connect(self.socket()).is_err() {
            if let Some(status) = running.0.try_wait().unwrap() {
                panic!("maxd run exited early with {status}");
            }
            assert!(start.elapsed() < WAIT, "maxd run never opened its socket");
            thread::sleep(Duration::from_millis(20));
        }
        running
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `maxd run` this test started, killed when dropped.
struct Running(Child);

impl Running {
    /// Asks it to stop with SIGTERM, as launchd and systemd do, and waits.
    fn stop(mut self) -> std::process::ExitStatus {
        let pid = self.0.id().to_string();
        let sent = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
        assert!(sent.success());
        self.wait()
    }

    /// Kills it without warning, as a crash would, and waits.
    fn crash(mut self) {
        self.0.kill().unwrap();
        self.wait();
    }

    fn wait(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(start.elapsed() < WAIT, "maxd didn't stop");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Speaks the protocol over any pair of byte streams.
struct Client<R, W> {
    reader: R,
    writer: W,
    decoder: FrameDecoder,
    /// Every byte maxd sent, for tests that look for what it mustn't.
    heard: Vec<u8>,
}

/// A message or a terminal's output, as a client hears it.
#[derive(Debug)]
enum Heard {
    Message(ServerMessage),
    Output { offset: u64, bytes: Vec<u8> },
}

impl Client<UnixStream, UnixStream> {
    fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        Self::new(stream.try_clone().unwrap(), stream)
    }
}

impl<R: Read, W: Write> Client<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            decoder: FrameDecoder::new(),
            heard: Vec::new(),
        }
    }

    fn send(&mut self, message: &ClientMessage) {
        let mut out = Vec::new();
        frame::encode_message(&mut out, message).unwrap();
        self.writer.write_all(&out).unwrap();
        self.writer.flush().unwrap();
    }

    fn recv(&mut self) -> ServerMessage {
        loop {
            if let Heard::Message(message) = self.hear() {
                return message;
            }
        }
    }

    /// The next message or terminal output. Other frames pass.
    fn hear(&mut self) -> Heard {
        let mut buf = [0; 4096];
        loop {
            match self.decoder.next_frame().unwrap() {
                Some(Frame::Message(json)) => {
                    return Heard::Message(serde_json::from_slice(json).unwrap());
                }
                Some(Frame::TerminalOutput { offset, bytes, .. }) => {
                    let bytes = bytes.to_vec();
                    return Heard::Output { offset, bytes };
                }
                Some(_) => continue,
                None => {}
            }
            let n = self.reader.read(&mut buf).unwrap();
            assert_ne!(n, 0, "maxd closed the connection");
            self.heard.extend_from_slice(&buf[..n]);
            self.decoder.push(&buf[..n]);
        }
    }

    /// Attaches to `terminal` after the byte at `after`, and returns where
    /// its output starts. A `maxd` that just started holds the attach until
    /// the terminals the last one left are back, so it asks only once.
    fn attach(&mut self, terminal: TerminalId, after: Option<u64>) -> u64 {
        self.send(&ClientMessage::TerminalAttach { terminal, after });
        match self.answer() {
            ServerMessage::TerminalAttached {
                terminal: got,
                from,
                reset,
            } if got == terminal => {
                assert!(!reset, "an attach after {after:?} started over");
                from
            }
            other => panic!("expected the attach, got {other:?}"),
        }
    }

    /// Reads a terminal's output from `next` into `text` until it holds
    /// `until`. Returns where the next chunk starts.
    fn read_until(&mut self, next: u64, text: &mut Vec<u8>, until: &str) -> u64 {
        let mut state = Replica::default();
        self.follow(&mut state, text, next, |_, text| {
            has(text, until.as_bytes())
        })
    }

    /// Hears the state into `replica` and a terminal's output from `next`
    /// into `text` until `done`, checking each chunk starts where the last
    /// ended. Returns where the next chunk starts.
    fn follow(
        &mut self,
        replica: &mut Replica,
        text: &mut Vec<u8>,
        mut next: u64,
        done: impl Fn(&Replica, &[u8]) -> bool,
    ) -> u64 {
        while !done(replica, text) {
            match self.hear() {
                Heard::Output { offset, bytes } => {
                    assert_eq!(offset, next, "the output has a gap or repeats");
                    next += u64::try_from(bytes.len()).unwrap();
                    text.extend(bytes);
                }
                Heard::Message(ServerMessage::TerminalAttached { from, .. }) => {
                    panic!("the output started over at {from}")
                }
                Heard::Message(ServerMessage::Snapshot(snapshot)) => {
                    replica.apply_snapshot(snapshot);
                }
                Heard::Message(ServerMessage::Delta(delta)) => {
                    let _ = replica.apply_delta(delta);
                }
                Heard::Message(_) => {}
            }
        }
        next
    }

    /// Types `bytes` into `terminal`.
    fn type_into(&mut self, terminal: TerminalId, bytes: &[u8]) {
        let mut out = Vec::new();
        frame::encode_terminal_input(&mut out, terminal, bytes);
        self.writer.write_all(&out).unwrap();
        self.writer.flush().unwrap();
    }

    /// The next message that isn't a delta. A synced client gets one
    /// whenever maxd's state changes, which can come before an answer.
    fn answer(&mut self) -> ServerMessage {
        loop {
            match self.recv() {
                ServerMessage::Delta(_) => {}
                other => return other,
            }
        }
    }

    /// Says hello as the app and proves it with the test key, which a
    /// `maxd` that trusts no key yet takes as its first.
    fn hello(&mut self) -> ServerHello {
        let client_id = ClientId::derive("maxd-cli-test");
        self.send(&ClientMessage::Hello(ClientHello {
            protocol: ProtocolRange::OURS,
            client: ClientInfo {
                name: "maxd-cli-test".into(),
                version: "0".into(),
            },
            client_id,
            role: ClientRole::App,
        }));
        let hello = match self.recv() {
            ServerMessage::Hello(hello) => hello,
            other => panic!("expected a hello, got {other:?}"),
        };
        let challenge = hello.challenge.as_deref().and_then(Challenge::parse);
        let statement = Statement {
            kind: Kind::App,
            device: hello.device,
            instance: hello.instance,
            client: client_id,
            challenge: challenge.expect("a challenge in the hello"),
        };
        let proof = prove::sign(&APP_SEED, &statement).expect("a proof");
        let proved = self.act(ClientMessage::Prove {
            key: IdempotencyKey::generate(),
            public_key: proof.public_key,
            signature: proof.signature,
        });
        if proved != ActionOutcome::Done {
            let enrolled = self.act(ClientMessage::EnrollAppKey {
                key: IdempotencyKey::generate(),
                label: "maxd-cli-test".into(),
            });
            assert_eq!(enrolled, ActionOutcome::Done, "after {proved:?}");
        }
        hello
    }

    /// Asks for an action and gives its outcome.
    fn act(&mut self, message: ClientMessage) -> ActionOutcome {
        let key = message.action_key().expect("an action");
        self.send(&message);
        loop {
            if let ServerMessage::ActionResult { key: got, outcome } = self.recv()
                && got == key
            {
                return outcome;
            }
        }
    }

    /// Syncs from nothing and returns this device as maxd describes it.
    fn device(&mut self, hello: &ServerHello) -> Device {
        self.send(&ClientMessage::Sync { since: None });
        let ServerMessage::Snapshot(snapshot) = self.recv() else {
            panic!("expected a snapshot");
        };
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot);
        match replica.get(&EntityKey::Device(hello.device)) {
            Some(Entity::Device(device)) => device.clone(),
            other => panic!("expected this device, got {other:?}"),
        }
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The public half of the app key with `seed`, as the app sends it.
fn app_key(seed: &[u8; 32]) -> String {
    use base64::Engine;
    use ring::signature::KeyPair;
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pair.public_key())
}

#[test]
fn setup_s_app_key_counts_while_the_device_trusts_none() {
    use std::os::unix::fs::PermissionsExt;
    const SSH: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample";
    let world = World::new();
    let data = if cfg!(target_os = "macos") {
        world.0.join("home/Library/Application Support/MaxHQ")
    } else {
        world.0.join("data/maxhq")
    };
    let file = data.join("phones/app-keys.json");
    let authorized = world.0.join("home/.ssh/authorized_keys");
    let authorize = |app: &str, label: &str| {
        let args = ["authorize-key", SSH, "--app", app, label];
        world.maxd(&args).output().unwrap()
    };

    // A bad app key stops it before the SSH key goes in.
    let output = authorize("not-a-key", "MaxHQ");
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(!authorized.exists() && !file.exists());

    let output = authorize(&app_key(&APP_SEED), "MaxHQ on a Mac");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fs::read_to_string(&authorized).unwrap().contains(SSH));
    let mode = fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let kept = fs::read(&file).unwrap();
    let keys: serde_json::Value = serde_json::from_slice(&kept).unwrap();
    assert_eq!(keys.as_array().map(Vec::len), Some(1));
    assert_eq!(keys[0]["public_key"], app_key(&APP_SEED));
    assert_eq!(keys[0]["label"], "MaxHQ on a Mac");
    assert_eq!(keys[0]["origin"], "setup");
    assert_eq!(keys[0]["proved_at"], serde_json::Value::Null);

    // Once it trusts one, another waits for the app to ask, and setup
    // still succeeds.
    let output = authorize(&app_key(&[8; 32]), "Elsewhere");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("already trusts an app key"));
    assert_eq!(fs::read(&file).unwrap(), kept);

    // The app proves with setup's key without asking for anything.
    let _maxd = world.start();
    Client::connect(&world.socket()).hello();
    let keys: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(keys.as_array().map(Vec::len), Some(1));
    assert_ne!(keys[0]["proved_at"], serde_json::Value::Null);
}

/// A `Project` keeps what a device can speak to, so this compares the rest.
#[test]
fn the_app_s_view_keeps_all_a_real_maxd_reports() {
    let world = World::new();
    let _maxd = world.start();
    let mut client = Client::connect(&world.socket());
    let hello = client.hello();
    client.send(&ClientMessage::Sync { since: None });
    let ServerMessage::Snapshot(snapshot) = client.recv() else {
        panic!("expected a snapshot");
    };
    let mut replica = Replica::default();
    replica.apply_snapshot(snapshot);
    let entities: Vec<Entity> = replica.entities().cloned().collect();
    assert!(entities
        .iter()
        .any(|entity| matches!(entity, Entity::Device(_))));

    let spec = LinkSpec {
        id: LinkId::derive("real"),
        name: "real".into(),
        route: Route::Ssh {
            host: "real".into(),
        },
        fallbacks: Vec::new(),
        last_good: BTreeMap::new(),
    };
    let mut merge = Merge::new();
    merge.add_link(&spec);
    merge.link_event(
        spec.id,
        LinkEvent::Link(Box::new(DeviceLink {
            routes: Vec::new(),
            id: spec.id,
            name: spec.name.clone(),
            device: Some(hello.device),
            route: spec.route.clone(),
            state: LinkState::Connected {
                since: Timestamp(0),
            },
            network: None,
            peer: None,
            via: Vec::new(),
            capabilities: None,
            protocol: None,
            protocol_mismatch: None,
            waiting: Vec::new(),
            claim: None,
        })),
    );
    merge.link_event(spec.id, LinkEvent::Synced(entities.clone()));
    for entity in &entities {
        assert_eq!(merge.store().get(&entity.key()), Some(entity));
    }
}

#[test]
fn version_names_the_protocol() {
    let output = World::new().maxd(&["--version"]).output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with(&format!("maxd {} (protocol ", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn unknown_arguments_print_the_usage() {
    let world = World::new();
    for args in [&[][..], &["serve"], &["service", "restart"]] {
        let output = world.maxd(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("maxd service install"));
    }
}

#[test]
fn the_document_text_child_prints_a_documents_text() {
    let world = World::new();
    let notebook = world.0.join("a.ipynb");
    fs::write(
        &notebook,
        r#"{"cells":[{"cell_type":"code","source":["print('hi')\n"],
            "outputs":[{"output_type":"stream","text":["hi\n"]}]}]}"#,
    )
    .unwrap();
    let output = world
        .maxd(&["__document-text", notebook.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("## Cell 1 (code)"), "{text}");
    assert!(text.contains("print('hi')"), "{text}");

    let broken = world.0.join("b.docx");
    fs::write(&broken, "not a zip").unwrap();
    let output = world
        .maxd(&["__document-text", broken.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn run_serves_this_device_and_keeps_its_id() {
    let world = World::new();
    let maxd = world.start();
    let mut client = Client::connect(&world.socket());
    let first = client.hello();
    let device = client.device(&first);
    assert_eq!(device.id, first.device);
    assert_eq!(device.os, std::env::consts::OS);
    assert_eq!(device.maxd_version, env!("CARGO_PKG_VERSION"));
    client.send(&ClientMessage::Ping { nonce: 3 });
    assert_eq!(client.answer(), ServerMessage::Pong { nonce: 3 });

    // SIGTERM stops it cleanly and takes the socket away.
    assert!(maxd.stop().success());
    assert!(!world.socket().exists());

    // The next run is the same device, on the same boot, as a new instance.
    let _maxd = world.start();
    let second = Client::connect(&world.socket()).hello();
    assert_eq!(second.device, first.device);
    assert_eq!(second.boot_id, first.boot_id);
    assert_ne!(second.instance, first.instance);
}

#[test]
fn a_second_run_says_the_first_holds_the_socket() {
    let world = World::new();
    let first = world.start();
    let output = world.maxd(&["run"]).output().unwrap();
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains("already running") && message.contains(&first.0.id().to_string()),
        "{message}"
    );
    // The first one is untouched.
    Client::connect(&world.socket()).hello();
}

#[test]
fn a_socket_left_by_a_crash_is_replaced() {
    let world = World::new();
    world.start().crash();
    assert!(world.socket().exists(), "a crash leaves the socket behind");
    let _maxd = world.start();
    Client::connect(&world.socket()).hello();
}

#[test]
fn proxy_carries_the_protocol_over_stdin_and_stdout() {
    let world = World::new();
    let _maxd = world.start();
    let mut proxy = world
        .maxd(&["proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = proxy.stdin.take().unwrap();
    let stdout = proxy.stdout.take().unwrap();
    let mut client = Client::new(stdout, stdin);
    let hello = client.hello();
    assert_eq!(client.device(&hello).id, hello.device);

    // Closing stdin ends the connection, and the proxy with it.
    drop(client);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = proxy.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < WAIT, "the proxy didn't exit");
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
}

#[test]
fn proxy_without_maxd_says_where_it_looked() {
    let world = World::new();
    let output = world.maxd(&["proxy"]).output().unwrap();
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains(&world.socket().display().to_string()),
        "{message}"
    );
}

#[test]
fn hold_without_one_folder_prints_its_usage() {
    let world = World::new();
    for args in [&["hold"][..], &["hold", "a", "b"]] {
        let output = world.maxd(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("maxd hold"), "{args:?}");
    }
}

/// A holder this test started, stopped when dropped if it still runs.
struct Held(PathBuf);

impl Drop for Held {
    fn drop(&mut self) {
        // It counts as this test's only while it holds the lock in this
        // test's folder, so nothing else is signalled.
        let Ok(lock) = File::open(self.0.join("holder.lock")) else {
            return;
        };
        if !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)) {
            return;
        }
        let Ok(holder) = fs::read(self.0.join("holder.json")) else {
            return;
        };
        let holder: serde_json::Value = serde_json::from_slice(&holder).unwrap_or_default();
        if let Some(pid) = holder["holder_pid"].as_u64() {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
    }
}

#[test]
fn hold_keeps_an_agent_running_for_maxd() {
    let world = World::new();
    let dirs = Dirs {
        sessions: world.0.join("hold"),
        sockets: world.0.join("holders"),
    };
    let holds = Holds::new(dirs.clone())
        .with_holder(BIN, vec!["hold".into()])
        .with_detach(Detach::Setsid);
    let launch = Launch {
        session: SessionId::generate(),
        // NixOS has no /bin/cat, so take the one on PATH.
        program: std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join("cat"))
            .find(|file| file.is_file())
            .expect("cat on PATH"),
        args: Vec::new(),
        env: Vec::new(),
        env_remove: Vec::new(),
        secret_env: Vec::new(),
        run_as: None,
        cwd: world.0.clone(),
        framing: Framing::ClaudeStreamJson,
    };
    let _held = Held(dirs.sessions.join(launch.session.to_string()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        holds.spawn(&launch).await.unwrap();
        assert_eq!(holds.list().unwrap(), [launch.session]);
        let mut connection = holds.connect(launch.session, None).await.unwrap();
        let said = r#"{"type":"user"}"#;
        let outcome = connection.writer.write(said.into(), None).await.unwrap();
        assert_eq!(outcome, WriteOutcome::Written);
        match tokio::time::timeout(WAIT, connection.events.recv()).await {
            Ok(Some(Event::Line(line))) => assert_eq!((line.seq, &*line.text), (1, said)),
            other => panic!("expected the line back, got {other:?}"),
        }
        connection.writer.stop(WAIT).await.unwrap();
        loop {
            match tokio::time::timeout(WAIT, connection.events.recv()).await {
                Ok(Some(Event::Line(_))) => {}
                Ok(Some(Event::Exited(exit))) => {
                    assert_eq!(exit.code, Some(0), "{exit:?}");
                    break;
                }
                other => panic!("expected the exit, got {other:?}"),
            }
        }
        connection.writer.forget().await.unwrap();
        assert!(holds.list().unwrap().is_empty());
    });
}

/// Whether `haystack` holds `needle`.
fn has(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The folder a `maxd` in `world` keeps its data in.
fn data_dir(world: &World) -> PathBuf {
    if cfg!(target_os = "macos") {
        world.0.join("home/Library/Application Support/MaxHQ")
    } else {
        world.0.join("data/maxhq")
    }
}

/// Every file under `dir`, with what it holds.
fn every_file(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => dirs.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Ok(bytes) = fs::read(&path) {
                        found.push((path, bytes));
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// `secret` as it could turn up: itself, and the middle of its base64 at
/// each of the three places it can start, in both alphabets.
fn secret_forms(secret: &str) -> Vec<Vec<u8>> {
    use base64::Engine;
    let mut forms = vec![secret.as_bytes().to_vec()];
    for engine in [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::URL_SAFE,
    ] {
        for shift in 0..3 {
            let encoded = engine.encode(format!("{}{secret}", "x".repeat(shift)));
            // The first and last four characters depend on what's around.
            forms.push(encoded.as_bytes()[4..encoded.len() - 4].to_vec());
        }
    }
    forms
}

/// Starts a terminal's holder in `world`'s folders, as a `maxd` there
/// would, running `script` in `sh`, and notes it as `kind` for an agent
/// this `maxd` doesn't have, so the next `maxd` there publishes it. A
/// terminal opens only for an agent `maxd` runs, which needs a vendor's
/// CLI, so the test starts the holder itself.
fn hold_terminal(
    world: &World,
    kind: TerminalKind,
    moved: bool,
    script: &str,
) -> (TerminalId, Held) {
    let terminals = maxhq_hold::Terminals::new(Dirs {
        sessions: data_dir(world).join("terminals"),
        sockets: world.0.join("holders"),
    })
    .with_holder(BIN, vec!["hold".into()])
    .with_detach(Detach::Setsid)
    .with_linger(Duration::from_secs(30));
    let launch = TermLaunch {
        terminal: TerminalId::generate(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env: vec![
            ("TERM".into(), "xterm-256color".into()),
            ("HOME".into(), world.0.clone().into()),
        ],
        cwd: world.0.clone(),
        cols: 80,
        rows: 24,
        run: matches!(kind, TerminalKind::Run { .. }),
        nonce: None,
    };
    let held = Held(terminals.dirs().sessions.join(launch.terminal.to_string()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(terminals.spawn(&launch)).unwrap();
    let note = serde_json::json!({
        "agent": AgentId::derive("maxd-cli-test"),
        "kind": kind,
        "cwd": "~",
        "started_at": Timestamp::now(),
        "opener": {"by": "app"},
        "moved": moved,
    });
    terminals.save_note(launch.terminal, &note).unwrap();
    (launch.terminal, held)
}

/// Waits until `done`, or fails saying `what` never happened.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    wait_up_to(WAIT, what, done);
}

/// Waits `limit` at most until `done`, or fails saying `what` never
/// happened.
fn wait_up_to(limit: Duration, what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "{what} never happened");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Dials the world's socket for an app link, in place of SSH.
struct Dial(PathBuf);

impl PeerDial for Dial {
    fn dial(&self, _host: &str) -> BoxFuture<'static, io::Result<PeerStream>> {
        let socket = self.0.clone();
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(socket).await?;
            let (reader, writer) = stream.into_split();
            let stream: PeerStream = (Box::new(reader), Box::new(writer));
            Ok(stream)
        })
    }
}

/// Proves an app link with the test key, asking a device that doesn't
/// know it yet to trust it, as the app does.
struct AppKey;

impl Prover for AppKey {
    fn kind(&self) -> Kind {
        Kind::App
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        Box::pin(async move {
            let outcome = match proving.prove(&APP_SEED, false).await {
                Ok(ActionOutcome::Refused { .. }) => proving.enroll("MaxHQ").await,
                outcome => outcome,
            };
            match outcome {
                Ok(ActionOutcome::Done) => Verdict::Proven,
                Ok(other) => Verdict::Refused(format!("{other:?}")),
                Err(_) => Verdict::Lost,
            }
        })
    }
}

/// Where an app link hands a terminal's output: the text, and each place
/// it had a gap, repeated or started over. A panic here would only end
/// the link's task, so the test reads them after.
#[derive(Default)]
struct Screen(Mutex<(Vec<u8>, Vec<String>)>);

impl Screen {
    fn shows(&self, text: &str) -> bool {
        has(&self.0.lock().unwrap().0, text.as_bytes())
    }
}

impl TerminalSink for Screen {
    fn output(&self, _terminal: TerminalId, offset: u64, bytes: &[u8]) {
        let mut screen = self.0.lock().unwrap();
        let next = u64::try_from(screen.0.len()).unwrap();
        if offset != next {
            screen
                .1
                .push(format!("{} bytes at {offset}, after {next}", bytes.len()));
        }
        screen.0.extend_from_slice(bytes);
    }

    fn reset(&self, _terminal: TerminalId, from: u64) {
        let mut screen = self.0.lock().unwrap();
        screen.1.push(format!("started over at {from}"));
    }
}

#[test]
fn a_terminal_outlives_its_maxd_and_goes_on_from_the_last_byte_a_client_has() {
    let world = World::new();
    // Three bursts of 40 numbered lines. After each the shell leaves a
    // `wrote` file and waits for a `go` one, so the test knows what it
    // wrote while no maxd ran.
    let script = "i=0; for burst in 1 2 3; do end=$((burst * 40)); \
                  while [ $i -lt $end ]; do echo line $i; i=$((i+1)); done; \
                  : > wrote$burst; while [ ! -e go$burst ]; do sleep 0.05; done; done; \
                  exec sleep 600";
    let (terminal, held) = hold_terminal(&world, TerminalKind::Shell, false, script);
    let burst = |n: u32| {
        fs::write(world.0.join(format!("go{}", n - 1)), "").unwrap();
        let wrote = world.0.join(format!("wrote{n}"));
        wait_for(&format!("burst {n}"), || wrote.exists());
    };
    let mut text = Vec::new();

    let maxd = world.start();
    let mut client = Client::connect(&world.socket());
    client.hello();
    assert_eq!(client.attach(terminal, None), 0);
    let mut next = client.read_until(0, &mut text, "line 39\r\n");

    // A crash, then a stop as launchd or systemd asks for one. Each time
    // the shell writes on while no maxd runs, and the next one sends it
    // from the byte after the last one the client has.
    maxd.crash();
    burst(2);
    let maxd = world.start();
    let mut client = Client::connect(&world.socket());
    client.hello();
    assert_eq!(client.attach(terminal, Some(next - 1)), next);
    next = client.read_until(next, &mut text, "line 79\r\n");
    maxd.stop();
    burst(3);
    let _maxd = world.start();
    let mut client = Client::connect(&world.socket());
    client.hello();
    assert_eq!(client.attach(terminal, Some(next - 1)), next);
    client.read_until(next, &mut text, "line 119\r\n");

    let text = String::from_utf8(text).unwrap();
    let lines: Vec<&str> = text.split_terminator("\r\n").collect();
    let all: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    assert_eq!(lines, all);

    let close = ClientMessage::TerminalClose {
        key: IdempotencyKey::generate(),
        terminal,
        force: true,
    };
    assert_eq!(client.act(close), ActionOutcome::Done);
    wait_for("the closed terminal's folder going", || !held.0.exists());
}

/// The app's own link, not this file's client: it notes where a
/// terminal's output got to, and its reconnect attaches again from there.
#[test]
fn the_app_s_link_goes_on_after_the_last_byte_it_showed_when_maxd_comes_back() {
    let world = World::new();
    let script = "i=0; for burst in 1 2; do end=$((burst * 40)); \
                  while [ $i -lt $end ]; do echo line $i; i=$((i+1)); done; \
                  : > wrote$burst; while [ ! -e go$burst ]; do sleep 0.05; done; done; \
                  exec sleep 600";
    let (terminal, held) = hold_terminal(&world, TerminalKind::Shell, false, script);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _inside = runtime.enter();

    let maxd = world.start();
    let app = maxhq_client::Client::app(Transport::Streams(Arc::new(Dial(world.socket()))))
        .proving(Arc::new(AppKey));
    let (link, mut events) = app.link(LinkSpec {
        id: LinkId::derive("real"),
        name: "real".into(),
        route: Route::Ssh {
            host: "real".into(),
        },
        fallbacks: Vec::new(),
        last_good: BTreeMap::new(),
    });
    runtime.spawn(async move { while events.recv().await.is_some() {} });
    let screen = Arc::new(Screen::default());
    let sink: Arc<dyn TerminalSink> = screen.clone();
    runtime
        .block_on(link.attach_terminal(terminal, None, sink))
        .unwrap();
    wait_up_to(WAIT * 3, "the first burst", || screen.shows("line 39\r\n"));

    // The shell writes on while no maxd runs. The link tries again a
    // second or more after it lost the connection, then up to three
    // times as long as the last try each time.
    maxd.crash();
    fs::write(world.0.join("go1"), "").unwrap();
    wait_for("burst 2", || world.0.join("wrote2").exists());
    let _maxd = world.start();
    wait_up_to(WAIT * 6, "the second burst", || screen.shows("line 79\r\n"));

    let (text, wrong) = screen.0.lock().unwrap().clone();
    assert_eq!(wrong, Vec::<String>::new());
    let text = String::from_utf8(text).unwrap();
    let lines: Vec<&str> = text.split_terminator("\r\n").collect();
    let all: Vec<String> = (0..80).map(|i| format!("line {i}")).collect();
    assert_eq!(lines, all);

    let close = ClientMessage::TerminalClose {
        key: IdempotencyKey::generate(),
        terminal,
        force: true,
    };
    assert_eq!(
        runtime.block_on(link.act(close)).unwrap(),
        ActionOutcome::Done
    );
    wait_for("the closed terminal's folder going", || !held.0.exists());
}

#[test]
fn a_password_typed_into_a_terminal_is_kept_nowhere() {
    let world = World::new();
    let secret = format!("canary-{}", uuid::Uuid::now_v7().simple());
    let script = "stty -echo; printf 'Password: '; read secret; stty echo; echo; \
                  echo \"got ${#secret}\"";
    let kind = TerminalKind::Run {
        command: "./deploy.sh".into(),
        private: false,
    };
    // Moved to the drawer, so maxd keeps it, and its folder, once it ends.
    let (terminal, held) = hold_terminal(&world, kind, true, script);
    let log = world.0.join("maxd.log");
    let maxd = world.start_logging(&log);
    let mut client = Client::connect(&world.socket());
    client.hello();
    let mut text = Vec::new();
    let next = client.attach(terminal, None);
    let next = client.read_until(next, &mut text, "Password: ");

    // maxd shows the shell waits for a secret, as its holder sees.
    let shown = |replica: &Replica| match replica.get(&EntityKey::Terminal(terminal)) {
        Some(Entity::Terminal(shown)) => Some(shown.clone()),
        _ => None,
    };
    let mut replica = Replica::default();
    client.send(&ClientMessage::Sync { since: None });
    let next = client.follow(&mut replica, &mut text, next, |replica, _| {
        shown(replica).is_some_and(|shown| shown.password_prompt)
    });

    client.type_into(terminal, format!("{secret}\r").as_bytes());
    let said = format!("got {}\r\n", secret.len());
    client.follow(&mut replica, &mut text, next, |replica, text| {
        has(text, said.as_bytes())
            && shown(replica).is_some_and(|shown| {
                !shown.password_prompt
                    && matches!(shown.status, TerminalStatus::Exited { code: Some(0), .. })
            })
    });

    // The run's transcript marks where the secret went.
    let exit = held.0.join("exit.json");
    wait_for("the run's end kept", || {
        fs::read(&exit)
            .is_ok_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
    });
    let transcript = fs::read_to_string(&exit).unwrap();
    assert!(transcript.contains("🔒 sent"), "{transcript}");

    maxd.stop();
    let logged = fs::read_to_string(&log).unwrap();
    assert!(
        logged.contains("DEBUG") || logged.contains("TRACE"),
        "{logged}"
    );
    let forms = secret_forms(&secret);
    for (path, bytes) in every_file(&world.0) {
        for form in &forms {
            assert!(!has(&bytes, form), "{} has the secret", path.display());
        }
    }
    for form in &forms {
        assert!(!has(&client.heard, form), "maxd sent the secret back");
    }
}
