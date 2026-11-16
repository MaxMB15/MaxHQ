// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's SSH server, which editors reach over a host tunnel stream.
//! Each `TunnelOpen` for [`TunnelTarget::Ssh`] gets a russh session of its
//! own on an in-memory pipe, so nothing in the container listens for SSH.
//!
//! A session logs in as user `agent` with one of the keys the host pushed,
//! and with nothing else. Its channels run commands and shells as `agent`,
//! as a login shell in the work folder, with a pty for a channel that asks
//! for one. They reach ports on loopback only, and take no agent, X11 or
//! remote forwarding and no subsystem.
//!
//! [`TunnelTarget::Ssh`]: maxhq_protocol::guest::TunnelTarget::Ssh

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_hold::pty::{self, controlling_tty, winsize, Master};
use maxhq_hold::RunAs;
use maxhq_protocol::guest::MAX_SSH_KEYS;
use ring::rand::{SecureRandom, SystemRandom};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, PublicKeyOffer, VerifiedPublicKey};
use russh::{Channel, ChannelId, ChannelMsg, ChannelOpenFailure, MethodKind, MethodSet, Sig};
use rustix::process::{Pid, Signal};
use rustix::termios::Winsize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, warn};
use zeroize::Zeroizing;

use super::link::{Users, AGENT};
use super::worktree::PATH;

/// The most channels one session has open at once.
const MAX_CHANNELS: usize = 256;

/// The most variables a channel takes from the client.
const MAX_ENV: usize = 64;

/// The most chunks of input a channel holds for its process.
const INPUT_QUEUE: usize = 16;

/// Bytes read from a process or a port at a time.
const CHUNK: usize = 32 * 1024;

/// What the in-memory pipe under a session holds each way.
pub(crate) const PIPE: usize = 64 * 1024;

/// How long a pty's last output has to arrive after its shell exits.
/// Something the shell left running may hold the pty open for good.
const PTY_DRAIN: Duration = Duration::from_millis(200);

/// How often the server checks that a quiet client is still there.
const KEEPALIVE: Duration = Duration::from_secs(60);

/// How a channel's process starts.
pub(crate) struct Login {
    /// Finds user `agent`. None runs as the guest's own user, for tests.
    pub(crate) users: Users,
    /// The folder processes start in.
    pub(crate) work: PathBuf,
    /// The shell for a user whose record names none.
    pub(crate) shell: PathBuf,
}

impl Login {
    /// Processes as `agent` in the container's work folder.
    pub(crate) fn container() -> Self {
        Self {
            users: |user| RunAs::lookup(user).map(Some),
            work: PathBuf::from(super::files::WORK_DIR),
            shell: PathBuf::from("/bin/bash"),
        }
    }
}

/// The keys a session may log in with.
type Keys = Arc<Mutex<Vec<PublicKey>>>;

/// The server: its host key, the keys the host pushed, and how channels
/// start their processes.
pub(crate) struct Ssh {
    config: Arc<server::Config>,
    host_key: String,
    keys: Keys,
    login: Arc<Login>,
}

impl Ssh {
    /// A server with `host_key` that starts processes the way `login`
    /// says.
    pub(crate) fn new(host_key: PrivateKey, login: Login) -> Result<Self, String> {
        let public = host_key
            .public_key()
            .to_openssh()
            .map_err(|err| format!("the host key didn't encode: {err}"))?;
        let config = server::Config {
            methods: MethodSet::from(&[MethodKind::PublicKey][..]),
            keys: vec![host_key],
            // The stream underneath ends with the host link, so a quiet
            // session stays until its client goes.
            inactivity_timeout: None,
            keepalive_interval: Some(KEEPALIVE),
            ..Default::default()
        };
        Ok(Self {
            config: Arc::new(config),
            host_key: public,
            keys: Arc::default(),
            login: Arc::new(login),
        })
    }

    /// The host key's public half, as an OpenSSH line.
    pub(crate) fn host_key(&self) -> &str {
        &self.host_key
    }

    /// Takes the keys the host pushed in place of any before. Lines that
    /// aren't ed25519 keys are skipped, and past [`MAX_SSH_KEYS`] are
    /// dropped.
    pub(crate) fn authorize(&self, lines: &[String]) {
        let keys: Vec<PublicKey> = lines
            .iter()
            .take(MAX_SSH_KEYS)
            .filter(|line| line.len() <= crate::sshkey::MAX_LINE)
            .filter_map(|line| PublicKey::from_openssh(line).ok())
            .filter(|key| key.algorithm() == Algorithm::Ed25519)
            .collect();
        *lock(&self.keys) = keys;
    }

    /// Forgets the pushed keys, as the host link that pushed them is gone.
    /// Sessions already logged in carry on.
    pub(crate) fn forget(&self) {
        lock(&self.keys).clear();
    }

    /// Serves one session on `socket` until it ends.
    pub(crate) fn session(&self, socket: DuplexStream) {
        let config = self.config.clone();
        let handler = Session {
            keys: self.keys.clone(),
            login: self.login.clone(),
            channels: HashMap::new(),
            slots: Arc::new(Semaphore::new(MAX_CHANNELS)),
        };
        tokio::spawn(async move {
            let ended = match server::run_stream(config, socket, handler).await {
                Ok(running) => running.await,
                Err(err) => Err(err),
            };
            if let Err(err) = ended {
                debug!("an SSH session ended: {err}");
            }
        });
    }
}

/// The host key at `path`, made there first if there's none or the one
/// there doesn't read. The file is root's alone.
pub(crate) fn host_key(path: &Path) -> io::Result<PrivateKey> {
    match fs::read_to_string(path) {
        Ok(text) => match PrivateKey::from_openssh(Zeroizing::new(text).as_bytes()) {
            Ok(key) if key.algorithm() == Algorithm::Ed25519 && !key.is_encrypted() => {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
                return Ok(key);
            }
            _ => warn!("the SSH host key didn't read, so the guest makes a new one"),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let key = new_key()?;
    let text = key
        .to_openssh(LineEnding::LF)
        .map_err(|err| io::Error::other(format!("the host key didn't encode: {err}")))?;
    write_atomic(path, text.as_bytes(), Atomic::PRIVATE)?;
    Ok(key)
}

/// A new ed25519 key from the system's random source.
pub(crate) fn new_key() -> io::Result<PrivateKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(seed.as_mut())
        .map_err(|_| io::Error::other("the system has no random bytes"))?;
    Ok(PrivateKey::from(Ed25519Keypair::from_seed(&seed)))
}

/// One session's handler. It answers the client's requests, and each
/// channel's task acts on them.
struct Session {
    keys: Keys,
    login: Arc<Login>,
    /// What each session channel has asked for.
    channels: HashMap<ChannelId, Asked>,
    /// One per open channel.
    slots: Arc<Semaphore>,
}

impl Session {
    /// Lets `user` in with `key` only when it's `agent` with a pushed key.
    fn check(&self, user: &str, key: &PublicKey) -> Auth {
        let known = lock(&self.keys)
            .iter()
            .any(|ours| ours.key_data() == key.key_data());
        if user == AGENT && known {
            Auth::Accept
        } else {
            Auth::reject()
        }
    }

    /// Answers a channel request.
    fn reply(
        channel: ChannelId,
        ok: bool,
        session: &mut server::Session,
    ) -> Result<(), russh::Error> {
        if ok {
            session.channel_success(channel)
        } else {
            session.channel_failure(channel)
        }
    }

    /// Takes a channel's slot, or turns the channel away when they're all
    /// taken.
    async fn slot(
        &self,
        reply: ChannelOpenHandle,
    ) -> Option<(OwnedSemaphorePermit, ChannelOpenHandle)> {
        match self.slots.clone().try_acquire_owned() {
            Ok(slot) => Some((slot, reply)),
            Err(_) => {
                reply.reject(ChannelOpenFailure::ResourceShortage).await;
                None
            }
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
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let Some((slot, reply)) = self.slot(reply).await else {
            return Ok(());
        };
        self.channels.insert(channel.id(), Asked::default());
        reply.accept().await;
        tokio::spawn(run_session(channel, self.login.clone(), slot));
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let Some(addr) = loopback(host_to_connect, port_to_connect) else {
            debug!("a forward to {host_to_connect}:{port_to_connect} is refused");
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let Some((slot, reply)) = self.slot(reply).await else {
            return Ok(());
        };
        tokio::spawn(async move {
            match TcpStream::connect(addr).await {
                Ok(tcp) => {
                    reply.accept().await;
                    forward(channel, tcp).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
            drop(slot);
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let ok = self.channels.get_mut(&channel).is_some_and(Asked::pty);
        Self::reply(channel, ok, session)
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let ok = self
            .channels
            .get_mut(&channel)
            .is_some_and(|asked| asked.env(variable_name, variable_value));
        Self::reply(channel, ok, session)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let ok = self.channels.get_mut(&channel).is_some_and(Asked::start);
        Self::reply(channel, ok, session)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let ok = self.channels.get_mut(&channel).is_some_and(Asked::start);
        Self::reply(channel, ok, session)
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        debug!("the {name} subsystem is refused");
        Self::reply(channel, false, session)
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single_connection: bool,
        _x11_auth_protocol: &str,
        _x11_auth_cookie: &str,
        _x11_screen_number: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        Self::reply(channel, false, session)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let ok = self.channels.contains_key(&channel);
        Self::reply(channel, ok, session)
    }
}

/// What a session channel has asked for before its process starts. The
/// handler answers with it and the channel's task acts with its own copy,
/// and both see the same requests in the same order, so they agree.
#[derive(Default)]
struct Asked {
    pty: bool,
    env: usize,
    started: bool,
}

impl Asked {
    /// A pty, once, before the process starts.
    fn pty(&mut self) -> bool {
        let ok = !self.started && !self.pty;
        self.pty |= ok;
        ok
    }

    /// A variable from the allowlist, before the process starts.
    fn env(&mut self, name: &str, value: &str) -> bool {
        let ok = !self.started && self.env < MAX_ENV && allowed(name, value);
        self.env += usize::from(ok);
        ok
    }

    /// The command or shell, once.
    fn start(&mut self) -> bool {
        let ok = !self.started;
        self.started = true;
        ok
    }
}

/// Whether a client may set `name`: `LANG`, `TERM` and the `LC_*` and
/// `VSCODE_*` families, with plain names and no zero bytes.
fn allowed(name: &str, value: &str) -> bool {
    let plain = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    let listed =
        matches!(name, "LANG" | "TERM") || name.starts_with("LC_") || name.starts_with("VSCODE_");
    plain && listed && !value.contains('\0')
}

/// Where a forward to `host` and `port` goes, when that's loopback.
/// `localhost` is taken as 127.0.0.1 without asking a resolver.
fn loopback(host: &str, port: u32) -> Option<SocketAddr> {
    let port = u16::try_from(port).ok().filter(|port| *port != 0)?;
    let ip = match host {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        host => host.parse().ok()?,
    };
    let ok = match ip {
        IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST,
        IpAddr::V6(ip) => ip.is_loopback(),
    };
    ok.then_some(SocketAddr::new(ip, port))
}

/// Joins a forward's channel to its port until both ends close.
async fn forward(channel: Channel<Msg>, mut tcp: TcpStream) {
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional_with_sizes(&mut stream, &mut tcp, CHUNK, CHUNK).await;
}

/// The pty a channel asked for.
struct PtyAsk {
    term: String,
    size: Winsize,
}

/// Runs one session channel: takes its requests until it starts a
/// command or shell, runs that, and reports how it exited.
async fn run_session(mut channel: Channel<Msg>, login: Arc<Login>, _slot: OwnedSemaphorePermit) {
    let mut asked = Asked::default();
    let mut env = Vec::new();
    let mut pty = None;
    let command = loop {
        let Some(message) = channel.wait().await else {
            return;
        };
        match message {
            // Each guard records the request, as the handler answered it.
            ChannelMsg::RequestPty {
                term,
                col_width,
                row_height,
                pix_width,
                pix_height,
                ..
            } if asked.pty() => {
                let size = winsize(col_width, row_height, pix_width, pix_height);
                pty = Some(PtyAsk { term, size });
            }
            ChannelMsg::SetEnv {
                variable_name,
                variable_value,
                ..
            } if asked.env(&variable_name, &variable_value) => {
                env.push((variable_name, variable_value));
            }
            ChannelMsg::Exec { command, .. } if asked.start() => break Some(command),
            ChannelMsg::RequestShell { .. } if asked.start() => break None,
            ChannelMsg::WindowChange {
                col_width,
                row_height,
                pix_width,
                pix_height,
            } => {
                if let Some(pty) = &mut pty {
                    pty.size = winsize(col_width, row_height, pix_width, pix_height);
                }
            }
            ChannelMsg::Close => return,
            _ => {}
        }
    };
    let made = tokio::task::spawn_blocking(move || prepare(&login, command, env, pty))
        .await
        .unwrap_or_else(|err| Err(format!("the command didn't start: {err}")));
    let started = made.and_then(|(command, io)| {
        let mut command = tokio::process::Command::from(command);
        let child = command
            .spawn()
            .map_err(|err| format!("the command didn't start: {err}"))?;
        // This closes the guest's copy of the pty's terminal end, so the
        // master reads its end once the process's copies close.
        drop(command);
        Ok((child, io))
    });
    match started {
        Ok((child, io)) => run(channel, child, io).await,
        Err(message) => {
            warn!("an SSH channel's process didn't start: {message}");
            let _ = channel
                .extended_data(1, format!("maxd: {message}\r\n").as_bytes())
                .await;
            finish(&channel, 127).await;
        }
    }
}

/// How a process's input and output are joined.
enum Io {
    Pipes,
    Pty(Arc<Master>),
}

/// The process for a channel, ready to spawn: `command`, or the login
/// shell when there's none, as `agent`.
fn prepare(
    login: &Login,
    command: Option<Vec<u8>>,
    env: Vec<(String, String)>,
    pty: Option<PtyAsk>,
) -> Result<(std::process::Command, Io), String> {
    let run_as =
        (login.users)(AGENT).map_err(|err| format!("couldn't find user {AGENT}: {err}"))?;
    if run_as.as_ref().is_some_and(|user| user.uid == 0) {
        return Err(format!("user {AGENT} is root"));
    }
    if run_as.is_none() && rustix::process::geteuid().is_root() {
        return Err("commands don't run as root".to_owned());
    }
    let user = match &run_as {
        Some(run_as) => Some(run_as.login(&login.work)?),
        None => None,
    };
    let shell = user
        .as_ref()
        .and_then(|user| user.shell())
        .unwrap_or(&login.shell)
        .to_owned();
    let mut process = std::process::Command::new(&shell);
    let mut dash = OsString::from("-");
    dash.push(shell.file_name().unwrap_or(shell.as_os_str()));
    process.arg0(dash);
    if let Some(command) = command {
        if command.contains(&0) {
            return Err("the command has a zero byte".to_owned());
        }
        process.arg("-c").arg(OsString::from_vec(command));
    }

    process.env_clear();
    match &user {
        Some(user) => {
            process.envs(user.env().iter().map(|(key, value)| (key, value)));
        }
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                process.env("HOME", home);
            }
        }
    }
    process.env("SHELL", &shell).env("PATH", PATH);
    process.envs(env);

    let io = match pty {
        Some(ask) => {
            let (master, slave) =
                pty::open(ask.size).map_err(|err| format!("couldn't open a pty: {err}"))?;
            process.env("TERM", ask.term);
            let copy = |fd: &OwnedFd| {
                fd.try_clone()
                    .map(Stdio::from)
                    .map_err(|err| format!("couldn't open a pty: {err}"))
            };
            process
                .stdin(copy(&slave)?)
                .stdout(copy(&slave)?)
                .stderr(Stdio::from(slave));
            controlling_tty(&mut process);
            let master =
                Master::new(master).map_err(|err| format!("couldn't open a pty: {err}"))?;
            Io::Pty(Arc::new(master))
        }
        None => {
            process
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            new_session(&mut process);
            Io::Pipes
        }
    };

    match user {
        Some(user) => user.apply(&mut process)?,
        None => {
            process.current_dir(&login.work);
        }
    }
    Ok((process, io))
}

/// Has the forked process lead a session of its own, so a signal for the
/// channel reaches everything it starts.
#[allow(unsafe_code)]
fn new_session(process: &mut std::process::Command) {
    // SAFETY: `setsid` is a plain system call, which is safe in a forked
    // process.
    unsafe {
        process.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }
}

/// Runs a started process for `channel`: input from the client goes to
/// it, its output goes to the client within the client's window, and its
/// exit ends the channel.
async fn run(mut channel: Channel<Msg>, mut child: Child, io: Io) {
    let pid = child.id().and_then(|id| Pid::from_raw(id.cast_signed()));
    let (input, queue) = mpsc::channel::<Vec<u8>>(INPUT_QUEUE);
    let mut outputs = Vec::new();
    match &io {
        Io::Pipes => {
            if let Some(stdin) = child.stdin.take() {
                tokio::spawn(feed_pipe(stdin, queue));
            }
            if let Some(stdout) = child.stdout.take() {
                outputs.push(tokio::spawn(copy(stdout, channel.make_writer())));
            }
            if let Some(stderr) = child.stderr.take() {
                outputs.push(tokio::spawn(copy(stderr, channel.make_writer_ext(Some(1)))));
            }
        }
        Io::Pty(master) => {
            tokio::spawn(feed_pty(master.clone(), queue));
            outputs.push(tokio::spawn(copy_pty(
                master.clone(),
                channel.make_writer(),
            )));
        }
    }
    let mut input = Some(input);
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => {
                    if let Some(sender) = &input
                        && sender.send(data.to_vec()).await.is_err()
                    {
                        input = None;
                    }
                }
                Some(ChannelMsg::Eof) => input = None,
                Some(ChannelMsg::WindowChange { col_width, row_height, pix_width, pix_height }) => {
                    if let Io::Pty(master) = &io {
                        master.resize(winsize(col_width, row_height, pix_width, pix_height));
                    }
                }
                Some(ChannelMsg::Signal { signal }) => {
                    if let (Some(pid), Some(signal)) = (pid, signal_of(&signal)) {
                        let _ = rustix::process::kill_process_group(pid, signal);
                    }
                }
                Some(ChannelMsg::Close) | None => {
                    // The client is gone, so the process hears a hang-up.
                    if let Some(pid) = pid {
                        let _ = rustix::process::kill_process_group(pid, Signal::HUP);
                    }
                    return;
                }
                Some(_) => {}
            },
        }
    };
    drop(input);
    for output in outputs {
        match &io {
            Io::Pipes => {
                let _ = output.await;
            }
            Io::Pty(_) => {
                let abort = output.abort_handle();
                if tokio::time::timeout(PTY_DRAIN, output).await.is_err() {
                    abort.abort();
                }
            }
        }
    }
    let code = match status {
        Ok(status) => exit_code(status),
        Err(err) => {
            warn!("couldn't wait for an SSH channel's process: {err}");
            255
        }
    };
    finish(&channel, code).await;
}

/// Ends `channel` with exit status `code`.
async fn finish(channel: &Channel<Msg>, code: u32) {
    let _ = channel.exit_status(code).await;
    let _ = channel.eof().await;
    let _ = channel.close().await;
}

/// A process's exit code, or 128 and the signal that ended it, as a shell
/// reports one.
fn exit_code(status: ExitStatus) -> u32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code.cast_unsigned(),
        (None, Some(signal)) => 128 + signal.cast_unsigned(),
        (None, None) => 255,
    }
}

/// The signal a client named, among those a process group may get.
fn signal_of(signal: &Sig) -> Option<Signal> {
    Some(match signal {
        Sig::ABRT => Signal::ABORT,
        Sig::ALRM => Signal::ALARM,
        Sig::FPE => Signal::FPE,
        Sig::HUP => Signal::HUP,
        Sig::ILL => Signal::ILL,
        Sig::INT => Signal::INT,
        Sig::KILL => Signal::KILL,
        Sig::PIPE => Signal::PIPE,
        Sig::QUIT => Signal::QUIT,
        Sig::SEGV => Signal::SEGV,
        Sig::TERM => Signal::TERM,
        Sig::USR1 => Signal::USR1,
        Sig::Custom(_) => return None,
    })
}

/// Copies a process's output to the client.
async fn copy(mut from: impl AsyncRead + Unpin, mut to: impl AsyncWrite + Unpin) {
    let mut buf = vec![0; CHUNK];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buf[..read]).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Copies a pty's output to the client.
async fn copy_pty(from: Arc<Master>, mut to: impl AsyncWrite + Unpin) {
    let mut buf = vec![0; CHUNK];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buf[..read]).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Writes the client's input to a process's standard input, which closes
/// when the input ends.
async fn feed_pipe(mut stdin: ChildStdin, mut queue: mpsc::Receiver<Vec<u8>>) {
    while let Some(data) = queue.recv().await {
        if stdin.write_all(&data).await.is_err() {
            return;
        }
    }
}

/// Writes the client's input to a pty.
async fn feed_pty(master: Arc<Master>, mut queue: mpsc::Receiver<Vec<u8>>) {
    while let Some(data) = queue.recv().await {
        if master.write_all(&data).await.is_err() {
            return;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use russh::client;
    use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
    use tokio::net::TcpListener;

    use crate::testing::TempDir;

    use super::*;

    /// How long a test waits for the server.
    const WAIT: Duration = Duration::from_secs(20);

    /// A client that trusts only the server's own host key.
    struct Client {
        host_key: PublicKey,
    }

    impl client::Handler for Client {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            key: &PublicKeyOrCertificate,
        ) -> Result<bool, Self::Error> {
            Ok(matches!(key, PublicKeyOrCertificate::PublicKey { key, .. }
                if key.key_data() == self.host_key.key_data()))
        }
    }

    /// A server whose processes run as this test's own user in `work`,
    /// with `key` pushed.
    fn server(work: &Path, key: &PrivateKey) -> Ssh {
        let login = Login {
            users: |_| Ok(None),
            work: work.to_owned(),
            shell: PathBuf::from("/bin/sh"),
        };
        let ssh = Ssh::new(new_key().unwrap(), login).unwrap();
        ssh.authorize(&[key.public_key().to_openssh().unwrap()]);
        ssh
    }

    /// A client session over an in-memory pipe to `ssh`.
    async fn connect(ssh: &Ssh) -> client::Handle<Client> {
        let (ours, theirs) = tokio::io::duplex(PIPE);
        ssh.session(theirs);
        let host_key = PublicKey::from_openssh(ssh.host_key()).unwrap();
        let config = Arc::new(client::Config::default());
        client::connect_stream(config, ours, Client { host_key })
            .await
            .unwrap()
    }

    /// Logs in as `user` with `key`, and says whether that worked.
    async fn log_in(session: &mut client::Handle<Client>, user: &str, key: &PrivateKey) -> bool {
        let key = PrivateKeyWithHashAlg::new(Arc::new(key.clone()), None);
        session
            .authenticate_publickey(user, key)
            .await
            .unwrap()
            .success()
    }

    /// A session logged in as `agent`.
    async fn logged_in(ssh: &Ssh, key: &PrivateKey) -> client::Handle<Client> {
        let mut session = connect(ssh).await;
        assert!(log_in(&mut session, AGENT, key).await);
        session
    }

    /// What a channel printed, what it printed on stderr, and its exit
    /// status, once it closes.
    #[derive(Debug, Default)]
    struct Ran {
        out: String,
        err: String,
        status: Option<u32>,
    }

    async fn ran(channel: &mut Channel<client::Msg>) -> Ran {
        let mut ran = Ran::default();
        let read = async {
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => ran.out.push_str(&String::from_utf8_lossy(&data)),
                    ChannelMsg::ExtendedData { data, .. } => {
                        ran.err.push_str(&String::from_utf8_lossy(&data));
                    }
                    ChannelMsg::ExitStatus { exit_status } => ran.status = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
        };
        tokio::time::timeout(WAIT, read)
            .await
            .expect("the channel closes");
        ran
    }

    /// The answer to the last request with a reply.
    async fn answer(channel: &mut Channel<client::Msg>) -> bool {
        loop {
            match tokio::time::timeout(WAIT, channel.wait()).await.unwrap() {
                Some(ChannelMsg::Success) => return true,
                Some(ChannelMsg::Failure) | None => return false,
                Some(_) => {}
            }
        }
    }

    /// Whether this test runs as root, where the server won't run
    /// commands as its own user.
    fn as_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    #[tokio::test]
    async fn only_agent_with_a_pushed_key_logs_in() {
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);

        let mut session = connect(&ssh).await;
        assert!(!log_in(&mut session, "root", &key).await);
        assert!(!log_in(&mut session, AGENT, &new_key().unwrap()).await);
        let refused = session.authenticate_password(AGENT, "agent").await.unwrap();
        assert!(!refused.success());
        assert!(log_in(&mut session, AGENT, &key).await);

        // Keys go with the link that pushed them.
        ssh.forget();
        let mut session = connect(&ssh).await;
        assert!(!log_in(&mut session, AGENT, &key).await);
    }

    #[test]
    fn pushed_keys_must_be_ed25519_and_are_bounded() {
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let line = key.public_key().to_openssh().unwrap();
        let mut lines = vec!["ssh-rsa AAAA".to_owned(), "x".repeat(4096)];
        lines.extend(std::iter::repeat_n(line, MAX_SSH_KEYS + 5));
        ssh.authorize(&lines);
        assert_eq!(lock(&ssh.keys).len(), MAX_SSH_KEYS - 2);
    }

    #[tokio::test]
    async fn exec_runs_a_login_shell_in_the_work_folder_with_listed_variables() {
        if as_root() {
            return;
        }
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let mut channel = session.channel_open_session().await.unwrap();
        channel.set_env(true, "LANG", "C.UTF-8").await.unwrap();
        assert!(answer(&mut channel).await);
        // A login shell's profile may set LANG itself, as NixOS's does, so
        // the script reads a listed variable that no profile sets.
        channel
            .set_env(true, "LC_MAXHQ_TEST", "C.UTF-8")
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        channel
            .set_env(true, "VSCODE_AGENT_FOLDER", "/tmp/v")
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        channel
            .set_env(true, "LD_PRELOAD", "/tmp/evil.so")
            .await
            .unwrap();
        assert!(!answer(&mut channel).await);
        channel.set_env(true, "PATH", "/tmp").await.unwrap();
        assert!(!answer(&mut channel).await);
        let script = "echo $0; pwd; echo $LC_MAXHQ_TEST $VSCODE_AGENT_FOLDER; \
             echo ${LD_PRELOAD-unset} $PATH; echo oops >&2; exit 3";
        channel.exec(true, script).await.unwrap();
        assert!(answer(&mut channel).await);
        let ran = ran(&mut channel).await;

        let work = work.path().canonicalize().unwrap();
        let lines: Vec<&str> = ran.out.lines().collect();
        assert_eq!(lines[0], "-sh", "{ran:?}");
        assert_eq!(Path::new(lines[1]).canonicalize().unwrap(), work);
        assert_eq!(lines[2], "C.UTF-8 /tmp/v");
        // A login shell's profile may add to maxd's PATH, as Ubuntu's adds
        // /snap/bin, or replace it, as NixOS's does, but the PATH the client
        // asked for never gets in.
        let (preload, path) = lines[3].split_once(' ').unwrap();
        assert_eq!(preload, "unset");
        assert!(!path.is_empty());
        assert!(path.split(':').all(|dir| dir != "/tmp"), "{path}");
        assert_eq!(ran.err, "oops\n");
        assert_eq!(ran.status, Some(3));
    }

    #[tokio::test]
    async fn a_channel_runs_one_command() {
        if as_root() {
            return;
        }
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let mut channel = session.channel_open_session().await.unwrap();
        channel
            .exec(true, "read line; echo got $line")
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        channel.exec(true, "echo again").await.unwrap();
        assert!(!answer(&mut channel).await);
        channel
            .request_pty(true, "xterm", 80, 24, 0, 0, &[])
            .await
            .unwrap();
        assert!(!answer(&mut channel).await);
        channel.data_bytes(&b"hello\n"[..]).await.unwrap();
        channel.eof().await.unwrap();
        let ran = ran(&mut channel).await;
        assert_eq!(ran.out, "got hello\n");
        assert_eq!(ran.status, Some(0));
    }

    #[tokio::test]
    async fn a_pty_takes_the_size_the_client_last_gave() {
        if as_root() {
            return;
        }
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let mut channel = session.channel_open_session().await.unwrap();
        channel
            .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        channel.window_change(100, 40, 0, 0).await.unwrap();
        channel
            .exec(true, "test -t 0 && echo $TERM; stty size")
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        let ran = ran(&mut channel).await;
        let lines: Vec<&str> = ran.out.lines().map(str::trim).collect();
        assert_eq!(lines, ["xterm-256color", "40 100"], "{ran:?}");
        assert_eq!(ran.status, Some(0));
    }

    #[tokio::test]
    async fn a_signal_reaches_the_command() {
        if as_root() {
            return;
        }
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let mut channel = session.channel_open_session().await.unwrap();
        channel
            .exec(true, "echo ready; exec sleep 30")
            .await
            .unwrap();
        assert!(answer(&mut channel).await);
        loop {
            match tokio::time::timeout(WAIT, channel.wait()).await.unwrap() {
                Some(ChannelMsg::Data { .. }) => break,
                Some(_) => {}
                None => panic!("the channel closed"),
            }
        }
        channel.signal(Sig::TERM).await.unwrap();
        let ran = ran(&mut channel).await;
        assert_eq!(ran.status, Some(128 + 15));
    }

    #[tokio::test]
    async fn the_sftp_subsystem_and_forwarding_are_refused() {
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let mut channel = session.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();
        assert!(!answer(&mut channel).await);
        channel.agent_forward(true).await.unwrap();
        assert!(!answer(&mut channel).await);
        channel
            .request_x11(true, false, "MIT-MAGIC-COOKIE-1", "00", 0)
            .await
            .unwrap();
        assert!(!answer(&mut channel).await);
        assert!(session.tcpip_forward("127.0.0.1", 0).await.is_err());
    }

    #[tokio::test]
    async fn direct_tcpip_reaches_loopback_only() {
        let work = TempDir::new();
        let key = new_key().unwrap();
        let ssh = server(work.path(), &key);
        let session = logged_in(&ssh, &key).await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4];
            socket.read_exact(&mut buf).await.unwrap();
            socket.write_all(&buf).await.unwrap();
        });
        let channel = session
            .channel_open_direct_tcpip("127.0.0.1", u32::from(port), "127.0.0.1", 40_000)
            .await
            .unwrap();
        let mut stream = channel.into_stream();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0; 4];
        tokio::time::timeout(WAIT, stream.read_exact(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf, b"ping");

        for host in ["10.0.0.1", "example.com", "0.0.0.0", "127.0.0.2"] {
            let refused = session
                .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 40_000)
                .await;
            assert!(refused.is_err(), "{host}");
        }
    }

    #[test]
    fn only_listed_variables_pass() {
        for name in ["LANG", "TERM", "LC_ALL", "LC_CTYPE", "VSCODE_IPC_HOOK_CLI"] {
            assert!(allowed(name, "x"), "{name}");
        }
        for name in [
            "",
            "PATH",
            "LD_PRELOAD",
            "HOME",
            "LANGUAGE",
            "LC-ALL",
            "LC_A=B",
            "BASH_ENV",
        ] {
            assert!(!allowed(name, "x"), "{name}");
        }
        assert!(!allowed("LANG", "C\0x"));
    }

    #[test]
    fn forwards_go_to_loopback_ports() {
        let local = |port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        assert_eq!(loopback("127.0.0.1", 22), Some(local(22)));
        assert_eq!(loopback("localhost", 8080), Some(local(8080)));
        assert_eq!(
            loopback("::1", 5173),
            Some(SocketAddr::new(
                IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                5173
            ))
        );
        for (host, port) in [
            ("127.0.0.1", 0),
            ("127.0.0.1", 70_000),
            ("10.0.0.1", 22),
            ("192.168.1.1", 22),
            ("example.com", 80),
            ("::", 22),
            ("", 22),
        ] {
            assert_eq!(loopback(host, port), None, "{host}:{port}");
        }
    }

    #[test]
    fn the_host_key_is_made_once_and_kept_private() {
        let dir = TempDir::new();
        let path = dir.path().join("host_ed25519");
        let made = host_key(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let again = host_key(&path).unwrap();
        assert_eq!(made.public_key(), again.public_key());

        fs::write(&path, "not a key").unwrap();
        let fresh = host_key(&path).unwrap();
        assert_ne!(fresh.public_key(), made.public_key());
        assert_eq!(fresh.algorithm(), Algorithm::Ed25519);
    }
}
