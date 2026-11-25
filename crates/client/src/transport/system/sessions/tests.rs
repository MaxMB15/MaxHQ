// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The russh logins against an in-process russh server, whose sessions
//! answer a command with `ran <command>` and a line on stderr, and the
//! plans they log in by.

use std::sync::atomic::{AtomicUsize, Ordering};

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::server::{self, Auth, ChannelOpenHandle, PublicKeyOffer, VerifiedPublicKey};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use tokio::io::AsyncReadExt;

use super::*;

/// Where the server is.
const DEVBOX: &str = "devbox.example";
const PORT: u16 = 2222;
/// Its name in `known_hosts`.
const NAME: &str = "[devbox.example]:2222";

fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

/// A `known_hosts` line for `key` under `name`.
fn line(name: &str, key: &PrivateKey) -> String {
    format!("{name} {}\n", key.public_key().to_openssh().unwrap())
}

/// One session on the far side, which lets `dev` in with `allowed`.
struct Session {
    allowed: PublicKey,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
}

impl Session {
    fn check(&self, user: &str, key: &PublicKey) -> Auth {
        if user == "dev" && key.key_data() == self.allowed.key_data() {
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

impl server::Handler for Session {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        offer: PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.check(user, offer.public_key))
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(self.check(user, key.public_key))
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// Says what it ran, and fails `false`.
    async fn exec_request(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        session.channel_success(id)?;
        session.data(id, format!("ran {command}\n").into_bytes())?;
        session.extended_data(id, 1, b"from stderr\n".to_vec())?;
        session.exit_status_request(id, u32::from(command == "false"))?;
        session.eof(id)?;
        session.close(id)?;
        self.channels.remove(&id);
        Ok(())
    }
}

/// Logins to one server over pipes, with your files in a folder of their
/// own.
struct Rig {
    masters: Masters,
    dir: PathBuf,
    host_key: PrivateKey,
    /// How many times a login reached the server.
    connections: Arc<AtomicUsize>,
}

impl Rig {
    /// A server with host key 1 that lets in key 2, which is in
    /// `~/id_ed25519`.
    fn new() -> Self {
        Self::signing_in_with(&key(2))
    }

    /// The same server, with `key` in `~/id_ed25519`.
    fn signing_in_with(key_in_file: &PrivateKey) -> Self {
        let host_key = key(1);
        let dir = std::env::temp_dir().join(format!("maxhq-sessions-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("id_ed25519"),
            key_in_file.to_openssh(LineEnding::LF).unwrap().as_bytes(),
        )
        .unwrap();
        let config = Arc::new(server::Config {
            methods: MethodSet::from(&[MethodKind::PublicKey][..]),
            keys: vec![host_key.clone()],
            inactivity_timeout: None,
            ..Default::default()
        });
        let allowed = key(2).public_key().clone();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&connections);
        let connect: Connect = Arc::new(move |host, port| {
            let config = Arc::clone(&config);
            let allowed = allowed.clone();
            let counted = Arc::clone(&counted);
            let host = host.to_owned();
            Box::pin(async move {
                if host != DEVBOX || port != PORT {
                    return Err(io::Error::from(io::ErrorKind::ConnectionRefused));
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let (ours, theirs) = tokio::io::duplex(64 * 1024);
                let session = Session {
                    allowed,
                    channels: HashMap::new(),
                };
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, theirs, session).await {
                        let _ = running.await;
                    }
                });
                Ok(Box::new(ours) as Box<dyn Io>)
            })
        });
        let env = Env {
            home: Some(dir.clone()),
            user: Some("someone".into()),
            ..Env::default()
        };
        Self {
            masters: Masters::with(connect, env),
            dir,
            host_key,
            connections,
        }
    }

    /// What `ssh -G devbox` would say, with `strict` for
    /// `StrictHostKeyChecking`.
    fn config(strict: &str) -> SshConfig {
        SshConfig {
            user: Some("dev".into()),
            host_name: DEVBOX.into(),
            port: Some(PORT),
            strict_host_key_checking: Some(strict.into()),
            identity_files: vec!["~/id_ed25519".into()],
            known_hosts_files: vec!["~/known_hosts".into()],
            ..SshConfig::default()
        }
    }

    fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }

    /// Keeps the server's real key in `known_hosts`.
    fn trust(&self) {
        std::fs::write(self.known_hosts(), line(NAME, &self.host_key)).unwrap();
    }

    async fn dial(&self, strict: &str) -> Result<Conn, DialError> {
        let config = Self::config(strict);
        self.masters
            .dial("devbox", Some(&config), None, "maxd proxy")
            .await
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Everything the far side sends before it closes.
async fn read_all(mut conn: Conn) -> String {
    let mut text = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        conn.reader.read_to_string(&mut text),
    )
    .await
    .expect("the channel ends")
    .unwrap();
    text
}

fn reason(result: Result<Conn, DialError>) -> String {
    match result {
        Ok(_) => panic!("the login went through"),
        Err(err) => err.reason,
    }
}

#[tokio::test]
async fn a_login_runs_the_proxy_command_and_streams_it() {
    let rig = Rig::new();
    rig.trust();
    let conn = rig.dial("true").await.unwrap();
    assert_eq!(read_all(conn).await, "ran maxd proxy\n");
}

#[tokio::test]
async fn run_gathers_what_a_command_printed_and_how_it_ended() {
    let rig = Rig::new();
    rig.trust();
    let config = Rig::config("true");
    let output = rig
        .masters
        .run("devbox", Some(&config), "uname")
        .await
        .unwrap();
    assert!(output.ok);
    assert_eq!(output.stdout, "ran uname\n");
    assert_eq!(output.stderr, "from stderr\n");

    let failed = rig
        .masters
        .run("devbox", Some(&config), "false")
        .await
        .unwrap();
    assert!(!failed.ok);
}

#[tokio::test]
async fn later_connections_reuse_the_login_until_its_forgotten() {
    let rig = Rig::new();
    rig.trust();
    let first = rig.dial("true").await.unwrap();
    let second = rig.dial("true").await.unwrap();
    assert_eq!(read_all(first).await, "ran maxd proxy\n");
    assert_eq!(read_all(second).await, "ran maxd proxy\n");
    let config = Rig::config("true");
    rig.masters
        .run("devbox", Some(&config), "uname")
        .await
        .unwrap();
    assert_eq!(rig.connections(), 1);

    rig.masters.forget("devbox").await;
    let third = rig.dial("true").await.unwrap();
    assert_eq!(read_all(third).await, "ran maxd proxy\n");
    assert_eq!(rig.connections(), 2);
}

#[tokio::test]
async fn an_unknown_host_key_is_refused_when_checking_is_strict() {
    let rig = Rig::new();
    let refused = reason(rig.dial("true").await);
    assert!(refused.contains("isn't trusted"), "{refused}");
    assert!(!rig.known_hosts().exists());
}

#[tokio::test]
async fn accept_new_keeps_a_new_host_key() {
    let rig = Rig::new();
    let conn = rig.dial("accept-new").await.unwrap();
    assert_eq!(read_all(conn).await, "ran maxd proxy\n");
    let kept = std::fs::read_to_string(rig.known_hosts()).unwrap();
    assert_eq!(kept.trim(), line(NAME, &rig.host_key).trim());

    // And a strict login reads it back.
    rig.masters.forget("devbox").await;
    rig.dial("true").await.unwrap();
}

#[tokio::test]
async fn a_changed_host_key_is_refused_even_with_accept_new() {
    let rig = Rig::new();
    std::fs::write(rig.known_hosts(), line(NAME, &key(9))).unwrap();
    let refused = reason(rig.dial("accept-new").await);
    assert!(refused.contains("isn't trusted"), "{refused}");
}

#[tokio::test]
async fn a_revoked_host_key_is_refused_even_when_its_known() {
    let rig = Rig::new();
    let revoked = format!(
        "@revoked * {}",
        line(NAME, &rig.host_key).split_once(' ').unwrap().1
    );
    std::fs::write(
        rig.known_hosts(),
        format!("{}{revoked}", line(NAME, &rig.host_key)),
    )
    .unwrap();
    let refused = reason(rig.dial("no").await);
    assert!(refused.contains("revoked"), "{refused}");
}

#[tokio::test]
async fn checking_off_takes_any_host_key() {
    let rig = Rig::new();
    let conn = rig.dial("false").await.unwrap();
    assert_eq!(read_all(conn).await, "ran maxd proxy\n");
    assert!(!rig.known_hosts().exists());
}

#[tokio::test]
async fn a_key_the_server_doesnt_take_is_refused() {
    let rig = Rig::signing_in_with(&key(3));
    rig.trust();
    let refused = reason(rig.dial("true").await);
    assert_eq!(refused, "SSH sign-in was refused");
}

#[tokio::test]
async fn a_pinned_login_uses_only_the_pin() {
    let rig = Rig::new();
    let known_hosts = rig.dir.join("pinned_known_hosts");
    std::fs::write(&known_hosts, line("maxhq-devbox", &rig.host_key)).unwrap();
    let pin = PinnedSsh {
        host: DEVBOX.into(),
        port: PORT,
        user: "dev".into(),
        key_file: rig.dir.join("id_ed25519"),
        known_hosts,
        alias: "maxhq-devbox".into(),
    };
    // The config's files would refuse it: nothing is in `~/known_hosts`.
    let conn = rig
        .masters
        .dial("devbox", None, Some(&pin), "maxd proxy --peer x")
        .await
        .unwrap();
    assert_eq!(read_all(conn).await, "ran maxd proxy --peer x\n");
}

#[tokio::test]
async fn hosts_that_arent_ssh_hosts_are_refused() {
    let rig = Rig::new();
    let config = Rig::config("true");
    let refused = rig
        .masters
        .dial("-oProxyCommand=x", Some(&config), None, "maxd proxy")
        .await;
    assert_eq!(reason(refused), "that isn't an SSH host name");
    assert_eq!(rig.connections(), 0);
}

/// Your files on a Windows machine.
fn windows_env() -> Env {
    Env {
        home: Some(PathBuf::from(r"C:\Users\dev")),
        user: Some("dev".into()),
        auth_sock: None,
        program_data: Some(PathBuf::from(r"C:\ProgramData")),
    }
}

#[test]
fn a_plan_follows_what_the_windows_ssh_prints() {
    let config = SshConfig::parse(
        "user dev\n\
         hostname devbox.example\n\
         port 2222\n\
         connecttimeout 5\n\
         stricthostkeychecking accept-new\n\
         identitiesonly yes\n\
         identityagent none\n\
         identityfile ~/.ssh/id_ecdsa\n\
         identityfile ~/.ssh/id_%r_%h\n\
         identityfile ~/.ssh/id_%C\n\
         userknownhostsfile ~/.ssh/known_hosts ~/.ssh/known_hosts2\n\
         globalknownhostsfile __PROGRAMDATA__/ssh/ssh_known_hosts\n",
    );
    let plan = Plan::from_config("devbox", &config, &windows_env()).unwrap();
    let path = |text: &str| PathBuf::from(text);
    assert_eq!(
        plan,
        Plan {
            host: "devbox.example".into(),
            port: 2222,
            user: "dev".into(),
            // `%C` is a hash this doesn't make, so that file is left out.
            key_files: vec![
                path(r"C:\Users\dev/.ssh/id_ecdsa"),
                path(r"C:\Users\dev/.ssh/id_dev_devbox.example"),
            ],
            agent: None,
            identities_only: true,
            policy: Policy::AcceptNew,
            name: "[devbox.example]:2222".into(),
            known_hosts: vec![
                path(r"C:\Users\dev/.ssh/known_hosts"),
                path(r"C:\Users\dev/.ssh/known_hosts2"),
            ],
            global_known_hosts: vec![path(r"C:\ProgramData/ssh/ssh_known_hosts")],
            connect_timeout: Duration::from_secs(5),
        }
    );
}

#[test]
fn a_plan_finds_the_agent_and_the_login() {
    let env = Env {
        auth_sock: Some("/tmp/agent.sock".into()),
        ..windows_env()
    };
    let config = SshConfig {
        host_name: "devbox.example".into(),
        ..SshConfig::default()
    };
    let plan = Plan::from_config("ops@devbox", &config, &env).unwrap();
    assert_eq!(plan.user, "ops");
    assert_eq!(plan.port, 22);
    assert_eq!(plan.name, "devbox.example");
    assert_eq!(plan.agent.as_deref(), Some("/tmp/agent.sock"));
    assert_eq!(plan.policy, Policy::Strict);
    assert_eq!(plan.connect_timeout, CONNECT_TIMEOUT);

    let named = SshConfig {
        identity_agent: Some("~/agent".into()),
        host_key_alias: Some("box".into()),
        ..config.clone()
    };
    let plan = Plan::from_config("devbox", &named, &env).unwrap();
    assert_eq!(plan.user, "dev");
    assert_eq!(plan.name, "box");
    assert_eq!(plan.agent.as_deref(), Some(r"C:\Users\dev/agent"));
}

#[test]
fn jump_hosts_and_proxy_commands_are_refused_with_a_reason() {
    let jump = SshConfig {
        host_name: "devbox.example".into(),
        proxy_jump: vec!["bastion".into()],
        ..SshConfig::default()
    };
    let refused = Plan::from_config("devbox", &jump, &windows_env()).unwrap_err();
    assert!(refused.reason.contains("ProxyJump"), "{}", refused.reason);

    let command = SshConfig {
        host_name: "devbox.example".into(),
        proxy_command: Some("ssh -W %h:%p bastion".into()),
        ..SshConfig::default()
    };
    let refused = Plan::from_config("devbox", &command, &windows_env()).unwrap_err();
    assert!(
        refused.reason.contains("ProxyCommand"),
        "{}",
        refused.reason
    );
}

#[test]
fn without_ssh_the_defaults_stand_in() {
    let config = fallback("ops@devbox.example:2200");
    assert_eq!(config.user.as_deref(), Some("ops"));
    assert_eq!(config.host_name, "devbox.example");
    assert_eq!(config.port, Some(2200));
    assert!(config
        .identity_files
        .contains(&"~/.ssh/id_ed25519".to_owned()));
    assert_eq!(config.known_hosts_files[0], "~/.ssh/known_hosts");
    let plan = Plan::from_config("ops@devbox.example:2200", &config, &windows_env()).unwrap();
    assert_eq!(plan.name, "[devbox.example]:2200");
    assert_eq!(plan.key_files.len(), DEFAULT_KEYS.len());
}

#[test]
fn tokens_ssh_cant_fill_drop_the_path() {
    let env = Env {
        home: None,
        ..windows_env()
    };
    let tokens = Tokens {
        env: &env,
        host: "devbox",
        user: "dev",
    };
    assert_eq!(tokens.expand("~/.ssh/id"), None);
    assert_eq!(
        tokens.expand("C:/keys/%r-100%%"),
        Some(PathBuf::from("C:/keys/dev-100%"))
    );
    assert_eq!(tokens.expand("C:/keys/%"), None);
}

#[test]
fn host_key_checks_follow_the_config() {
    assert_eq!(Policy::from_config(Some("true")), Policy::Strict);
    assert_eq!(Policy::from_config(Some("ask")), Policy::Strict);
    assert_eq!(Policy::from_config(None), Policy::Strict);
    assert_eq!(Policy::from_config(Some("accept-new")), Policy::AcceptNew);
    assert_eq!(Policy::from_config(Some("false")), Policy::Accept);

    let shown = key(1).public_key().clone();
    let other = key(2).public_key().clone();
    let keys = |known: &[&PublicKey], revoked: &[&PublicKey]| HostKeys {
        known: known.iter().map(|key| (*key).clone()).collect(),
        revoked: revoked.iter().map(|key| (*key).clone()).collect(),
    };
    let none = keys(&[], &[]);
    assert_eq!(none.verdict(Policy::Strict, &shown), Verdict::Unknown);
    assert_eq!(none.verdict(Policy::AcceptNew, &shown), Verdict::Learn);
    assert_eq!(none.verdict(Policy::Accept, &shown), Verdict::Accept);
    let known = keys(&[&other, &shown], &[]);
    assert_eq!(known.verdict(Policy::Strict, &shown), Verdict::Known);
    let changed = keys(&[&other], &[]);
    assert_eq!(changed.verdict(Policy::AcceptNew, &shown), Verdict::Changed);
    assert_eq!(changed.verdict(Policy::Accept, &shown), Verdict::Accept);
    let revoked = keys(&[&shown], &[&shown]);
    assert_eq!(revoked.verdict(Policy::Accept, &shown), Verdict::Revoked);
    for verdict in [
        Verdict::Revoked,
        Verdict::Changed,
        Verdict::Unknown,
        Verdict::Certificate,
    ] {
        assert!(!verdict.trusts());
    }
}

#[test]
fn the_host_shows_a_key_known_hosts_has_first() {
    let ecdsa = PublicKey::from_openssh(
        "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBMFCbMNeoNiJCJlHqjYRa3en6HDiNQMK/gEOaC92ddat6+RwjGqmzcpGwflo6Zq+6RrJ2bAOKQYmPiAbyKC0edA=",
    );
    let keys = HostKeys {
        known: ecdsa.into_iter().collect(),
        revoked: Vec::new(),
    };
    let order = keys.preferred();
    assert_eq!(
        order[0],
        Algorithm::Ecdsa {
            curve: russh::keys::EcdsaCurve::NistP256
        }
    );
    assert_eq!(order.len(), Preferred::DEFAULT.key.len());
    assert_eq!(
        HostKeys::default().preferred(),
        Preferred::DEFAULT.key.to_vec()
    );
}
