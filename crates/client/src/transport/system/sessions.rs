// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Logins through russh, for Windows, where openssh can't run: one per SSH
//! host, which later connections to it reuse the way openssh reuses a
//! ControlMaster. A host's options come from `ssh -G`, as they do for
//! openssh. A login signs in with your agent and then your key files,
//! checks the host key against your `known_hosts` the way your config
//! asks, and closes once it's gone unused for [`IDLE`].
//!
//! Not yet: `ProxyJump` and `ProxyCommand`, which it refuses with a reason
//! the app shows, host certificates, keys behind a passphrase, and RSA keys
//! from a file. RSA keys work through the agent.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use russh::client::{self, Handle, Msg};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::agent::AgentIdentity;
use russh::keys::PublicKeyOrCertificate;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{ChannelMsg, ChannelWriteHalf, Disconnect, Preferred};
use tokio::task::JoinHandle;
use tracing::debug;

use super::RUN_TIMEOUT;
use crate::pinned::PinnedSsh;
use crate::setup::Output;
use crate::ssh_config::{self, is_safe_destination, split_hop, SshConfig};
use crate::transport::channel::{self, ssh_failure, REASONS};
use crate::transport::reach::{self, Connect, Io};
use crate::transport::{Conn, DialError, Rest, CONNECT_TIMEOUT, LOGIN_TIMEOUT};

/// How long a login stays up once nothing uses it, as long as a master.
const IDLE: Duration = Duration::from_secs(600);

/// How often a login is checked for being idle.
const REAP_EVERY: Duration = Duration::from_secs(60);

/// How often a login asks the far side whether it's still there.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// Where Windows' own OpenSSH agent listens.
const WINDOWS_AGENT: &str = r"\\.\pipe\openssh-ssh-agent";

/// The key files ssh tries when its config names none, in its order.
const DEFAULT_KEYS: [&str; 3] = ["id_rsa", "id_ecdsa", "id_ed25519"];

/// The logins, one per host.
pub(in crate::transport) struct Masters {
    /// Logins by the host as the route names it.
    logins: Arc<Mutex<HashMap<String, Arc<Login>>>>,
    /// Reaches a host's port. Tests swap in pipes.
    connect: Connect,
    /// Where your files are.
    env: Env,
}

impl Masters {
    /// No logins yet. `_control_dir` is where openssh would keep masters,
    /// which russh has no use for.
    pub fn new(_control_dir: Option<PathBuf>) -> Self {
        Self::with(reach::tcp(), Env::current())
    }

    /// Reaches hosts through `connect`, with `env` for your files.
    fn with(connect: Connect, env: Env) -> Self {
        Self {
            logins: Arc::default(),
            connect,
            env,
        }
    }

    /// Runs `command`, which starts `maxd proxy`, on a host over its
    /// login, logging in first if there's none yet or it died.
    pub async fn dial(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        pin: Option<&PinnedSsh>,
        command: &str,
    ) -> Result<Conn, DialError> {
        if !is_safe_destination(host) {
            return Err(DialError::refused("that isn't an SSH host name"));
        }
        let login = self.login(host, config, pin).await?;
        let channel = match open_channel(&login).await {
            Ok(channel) => channel,
            Err(err) => {
                debug!(%err, "couldn't open an SSH channel");
                self.forget(host).await;
                return Err(DialError::down("the SSH connection dropped"));
            }
        };
        channel
            .exec(true, command)
            .await
            .map_err(|err| ssh_failure(&err))?;
        let piped = channel::pipe(channel);
        Ok(Conn {
            reader: piped.reader,
            writer: piped.writer,
            rest: Rest {
                stderr: Some(piped.stderr),
                _keep: Some(Box::new(Open {
                    pump: piped.pump,
                    closer: Some(piped.closer),
                    login,
                })),
            },
        })
    }

    /// Runs `command` on a host over its login and waits up to
    /// [`RUN_TIMEOUT`] for it. The host must be one [`is_safe_destination`]
    /// passes.
    pub async fn run(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        command: &str,
    ) -> io::Result<Output> {
        let login = self
            .login(host, config, None)
            .await
            .map_err(|err| io::Error::other(err.reason))?;
        match tokio::time::timeout(RUN_TIMEOUT, collect(&login, command)).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(err)) => {
                debug!(%err, "couldn't run a command over ssh");
                self.forget(host).await;
                Err(io::Error::other("the SSH connection dropped"))
            }
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "it took too long")),
        }
    }

    /// Drops a host's login. It closes once nothing still uses it.
    pub async fn forget(&self, host: &str) {
        let removed = self
            .logins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(host);
        drop(removed);
    }

    /// The host's login, if it's still up, or a new one.
    async fn login(
        &self,
        host: &str,
        config: Option<&SshConfig>,
        pin: Option<&PinnedSsh>,
    ) -> Result<Arc<Login>, DialError> {
        if let Some(login) = self.existing(host) {
            return Ok(login);
        }
        let plan = match (pin, config) {
            (Some(pin), _) => Plan::pinned(pin),
            (None, Some(config)) => Plan::from_config(host, config, &self.env)?,
            (None, None) => {
                let config = match ssh_config::resolve(host).await {
                    Ok(config) => config,
                    Err(err) => {
                        debug!(%err, "ssh -G didn't answer, so using ssh's defaults");
                        fallback(host)
                    }
                };
                Plan::from_config(host, &config, &self.env)?
            }
        };
        let stream = reach::open_within(
            &self.connect,
            &plan.host,
            plan.port,
            &REASONS,
            plan.connect_timeout,
        )
        .await?;
        let handle = match tokio::time::timeout(LOGIN_TIMEOUT, log_in(stream, &plan)).await {
            Ok(handle) => handle?,
            Err(_) => return Err(DialError::refused("SSH login didn't finish")),
        };
        let login = Arc::new(Login {
            handle: Some(handle),
            used: Mutex::new(Instant::now()),
        });
        self.logins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host.to_owned(), Arc::clone(&login));
        reap(
            Arc::downgrade(&self.logins),
            host.to_owned(),
            Arc::downgrade(&login),
        );
        Ok(login)
    }

    /// The host's login, unless it's gone.
    fn existing(&self, host: &str) -> Option<Arc<Login>> {
        let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
        let login = logins.get(host)?;
        if login.handle().is_closed() {
            logins.remove(host);
            return None;
        }
        Some(Arc::clone(login))
    }
}

/// Closes a login once only the book holds it and it's been idle for
/// [`IDLE`], or once it's closed, unless the book has a newer one by then.
fn reap(logins: Weak<Mutex<HashMap<String, Arc<Login>>>>, host: String, login: Weak<Login>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(REAP_EVERY).await;
            let Some(logins) = logins.upgrade() else {
                return;
            };
            let mut logins = logins.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(held) = logins.get(&host) else {
                return;
            };
            if !std::ptr::eq(Arc::as_ptr(held), login.as_ptr()) {
                return;
            }
            // Every other holder clones it under this lock, so the count
            // can't grow while it's read.
            let unused = Arc::strong_count(held) == 1 && held.idle() >= IDLE;
            if unused || held.handle().is_closed() {
                logins.remove(&host);
                return;
            }
        }
    });
}

/// One login, which channels share.
struct Login {
    /// Only `None` while it drops.
    handle: Option<Handle<Checker>>,
    /// When a channel on it last closed.
    used: Mutex<Instant>,
}

impl Login {
    fn handle(&self) -> &Handle<Checker> {
        self.handle.as_ref().expect("a login keeps its handle")
    }

    /// Marks it used just now.
    fn touch(&self) {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    /// How long since a channel on it last closed.
    fn idle(&self) -> Duration {
        self.used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }
}

impl Drop for Login {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

/// One `maxd proxy` channel. Dropping it closes the channel and leaves the
/// login to the next one.
struct Open {
    pump: JoinHandle<()>,
    closer: Option<ChannelWriteHalf<Msg>>,
    login: Arc<Login>,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.pump.abort();
        self.login.touch();
        if let Some(closer) = self.closer.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = closer.close().await;
            });
        }
    }
}

/// A session channel on `login`, within [`CONNECT_TIMEOUT`], since a login
/// whose network went away only notices at its next keepalive.
async fn open_channel(login: &Login) -> Result<russh::Channel<Msg>, russh::Error> {
    match tokio::time::timeout(CONNECT_TIMEOUT, login.handle().channel_open_session()).await {
        Ok(channel) => channel,
        Err(_) => Err(russh::Error::ConnectionTimeout),
    }
}

/// Runs `command` on `login` with no input, and gathers what it printed and
/// how it ended.
async fn collect(login: &Login, command: &str) -> Result<Output, russh::Error> {
    let mut channel = open_channel(login).await?;
    channel.exec(true, command).await?;
    channel.eof().await?;
    let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), None);
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    login.touch();
    Ok(Output {
        ok: status == Some(0),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Where a login looks for your files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Env {
    /// Your home folder, for `~` and `%d`.
    home: Option<PathBuf>,
    /// Your login on this machine, for `%u`.
    user: Option<String>,
    /// `SSH_AUTH_SOCK`.
    auth_sock: Option<String>,
    /// `%ProgramData%`, where Windows keeps ssh's own files.
    program_data: Option<PathBuf>,
}

impl Env {
    fn current() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            home: maxhq_os::home::home_dir(),
            user: maxhq_os::home::user_name(),
            auth_sock: var("SSH_AUTH_SOCK"),
            program_data: var("ProgramData").map(PathBuf::from),
        }
    }
}

/// What ssh does with a host key it doesn't know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    /// Refuses it, as `yes` and `ask` do, since nobody can be asked.
    Strict,
    /// Keeps it and goes on, as `accept-new` does.
    AcceptNew,
    /// Goes on, as `no` does.
    Accept,
}

impl Policy {
    /// The policy for a `StrictHostKeyChecking` value.
    fn from_config(value: Option<&str>) -> Self {
        match value {
            Some("accept-new") => Self::AcceptNew,
            Some("no" | "off" | "false") => Self::Accept,
            _ => Self::Strict,
        }
    }
}

/// Everything one login needs, from the host's config or its pin.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    host: String,
    port: u16,
    user: String,
    /// The key files to offer, in order.
    key_files: Vec<PathBuf>,
    /// The agent to ask before the files.
    agent: Option<String>,
    /// Offer only the keys the key files name, even from the agent.
    identities_only: bool,
    policy: Policy,
    /// The name host keys are kept under.
    name: String,
    /// Your `known_hosts` files. A new key goes in the first.
    known_hosts: Vec<PathBuf>,
    /// The machine's.
    global_known_hosts: Vec<PathBuf>,
    connect_timeout: Duration,
}

impl Plan {
    /// A pin's login: its key, its `known_hosts` and alias, and nothing
    /// else.
    fn pinned(pin: &PinnedSsh) -> Self {
        Self {
            host: pin.host.clone(),
            port: pin.port,
            user: pin.user.clone(),
            key_files: vec![pin.key_file.clone()],
            agent: None,
            identities_only: true,
            policy: Policy::Strict,
            name: pin.alias.clone(),
            known_hosts: vec![pin.known_hosts.clone()],
            global_known_hosts: Vec::new(),
            connect_timeout: CONNECT_TIMEOUT,
        }
    }

    /// The login `ssh -G` describes for the route host `route`.
    fn from_config(route: &str, config: &SshConfig, env: &Env) -> Result<Self, DialError> {
        if !config.proxy_jump.is_empty() {
            return Err(DialError::refused(
                "that host goes through ProxyJump, which MaxHQ can't follow on Windows yet",
            ));
        }
        if config.proxy_command.is_some() {
            return Err(DialError::refused(
                "that host has a ProxyCommand, which MaxHQ can't run on Windows yet",
            ));
        }
        let (destination, _) = split_hop(route);
        let route_user = destination
            .rsplit_once('@')
            .map(|(user, _)| user.to_owned());
        let Some(user) = config
            .user
            .clone()
            .or(route_user)
            .or_else(|| env.user.clone())
        else {
            return Err(DialError::refused("there's no SSH login for that host"));
        };
        let host = if config.host_name.is_empty() {
            destination
                .rsplit_once('@')
                .map_or(destination.as_str(), |(_, host)| host)
                .to_owned()
        } else {
            config.host_name.clone()
        };
        let tokens = Tokens {
            env,
            host: &host,
            user: &user,
        };
        let paths = |files: &[String]| -> Vec<PathBuf> {
            files
                .iter()
                .filter_map(|file| tokens.expand(file))
                .collect()
        };
        let agent = match config.identity_agent.as_deref() {
            Some(none) if none.eq_ignore_ascii_case("none") => None,
            Some("SSH_AUTH_SOCK") | None => env.auth_sock.clone().or_else(default_agent),
            Some(path) => tokens
                .expand(path)
                .map(|path| path.to_string_lossy().into_owned()),
        };
        Ok(Self {
            name: SshConfig {
                host_name: host.clone(),
                ..config.clone()
            }
            .known_hosts_name(),
            port: config.port.unwrap_or(22),
            key_files: paths(&config.identity_files),
            agent,
            identities_only: config.identities_only,
            policy: Policy::from_config(config.strict_host_key_checking.as_deref()),
            known_hosts: paths(&config.known_hosts_files),
            global_known_hosts: paths(&config.global_known_hosts_files),
            connect_timeout: config
                .connect_timeout
                .map_or(CONNECT_TIMEOUT, Duration::from_secs),
            host,
            user,
        })
    }
}

/// What ssh's defaults would say about `route` when `ssh -G` can't, as
/// when no ssh is installed.
fn fallback(route: &str) -> SshConfig {
    let (destination, port) = split_hop(route);
    let (user, host) = match destination.rsplit_once('@') {
        Some((user, host)) => (Some(user.to_owned()), host.to_owned()),
        None => (None, destination),
    };
    let global = if cfg!(windows) {
        "__PROGRAMDATA__/ssh/ssh_known_hosts"
    } else {
        "/etc/ssh/ssh_known_hosts"
    };
    SshConfig {
        user,
        host_name: host,
        port,
        identity_files: DEFAULT_KEYS
            .iter()
            .map(|name| format!("~/.ssh/{name}"))
            .collect(),
        known_hosts_files: vec!["~/.ssh/known_hosts".into(), "~/.ssh/known_hosts2".into()],
        global_known_hosts_files: vec![global.into()],
        ..SshConfig::default()
    }
}

/// The agent ssh asks when nothing names one.
fn default_agent() -> Option<String> {
    cfg!(windows).then(|| WINDOWS_AGENT.to_owned())
}

/// What the `~` and `%` tokens in a path stand for.
struct Tokens<'a> {
    env: &'a Env,
    host: &'a str,
    user: &'a str,
}

impl Tokens<'_> {
    /// `path` with its tokens filled in, or none when it has one this
    /// can't fill.
    fn expand(&self, path: &str) -> Option<PathBuf> {
        let home = || self.env.home.as_ref().map(|home| home.to_string_lossy());
        let mut text = String::new();
        let mut rest = path;
        if rest == "~" || rest.starts_with("~/") || rest.starts_with("~\\") {
            text.push_str(&home()?);
            rest = &rest[1..];
        } else if let Some(after) = rest.strip_prefix("__PROGRAMDATA__") {
            text.push_str(&self.env.program_data.as_ref()?.to_string_lossy());
            rest = after;
        }
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                text.push(c);
                continue;
            }
            match chars.next()? {
                '%' => text.push('%'),
                'd' => text.push_str(&home()?),
                'u' => text.push_str(self.env.user.as_deref()?),
                'h' => text.push_str(self.host),
                'r' => text.push_str(self.user),
                _ => return None,
            }
        }
        Some(PathBuf::from(text))
    }
}

/// What a login knows of the host's keys before it connects.
#[derive(Debug, Default)]
struct HostKeys {
    /// The keys `known_hosts` keeps for the host.
    known: Vec<PublicKey>,
    /// The keys marked `@revoked` for any host.
    revoked: Vec<PublicKey>,
}

impl HostKeys {
    /// The keys `plan`'s files keep for its host.
    fn read(plan: &Plan) -> Result<Self, DialError> {
        let mut keys = Self::default();
        for file in plan.known_hosts.iter().chain(&plan.global_known_hosts) {
            match russh::keys::known_hosts::known_host_keys_path(&plan.name, 22, file) {
                Ok(found) => keys.known.extend(found.into_iter().map(|(_, key)| key)),
                Err(err) => {
                    debug!(%err, file = %file.display(), "a known_hosts file doesn't read");
                    return Err(DialError::refused(
                        "a known_hosts line for this host doesn't read",
                    ));
                }
            }
            keys.revoked.extend(revoked(file));
        }
        Ok(keys)
    }

    /// What to do with the key the host shows.
    fn verdict(&self, policy: Policy, key: &PublicKey) -> Verdict {
        let same = |other: &PublicKey| other.key_data() == key.key_data();
        if self.revoked.iter().any(same) {
            Verdict::Revoked
        } else if self.known.iter().any(same) {
            Verdict::Known
        } else if policy == Policy::Accept {
            Verdict::Accept
        } else if self
            .known
            .iter()
            .any(|known| known.algorithm() == key.algorithm())
        {
            Verdict::Changed
        } else if policy == Policy::AcceptNew {
            Verdict::Learn
        } else {
            Verdict::Unknown
        }
    }

    /// The host key algorithms to ask for, the ones `known_hosts` has for
    /// the host first, so the host shows a key it can be checked by.
    fn preferred(&self) -> Vec<Algorithm> {
        let rsa = |algorithm: &Algorithm| matches!(algorithm, Algorithm::Rsa { .. });
        let recorded = |algorithm: &Algorithm| {
            self.known.iter().any(|key| {
                let known = key.algorithm();
                known == *algorithm || rsa(&known) && rsa(algorithm)
            })
        };
        let (mut first, rest): (Vec<_>, Vec<_>) = Preferred::DEFAULT
            .key
            .iter()
            .cloned()
            .partition(|algorithm| recorded(algorithm));
        first.extend(rest);
        first
    }
}

/// The keys in a `known_hosts` file's `@revoked` lines.
fn revoked(file: &Path) -> Vec<PublicKey> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "@revoked").then_some(())?;
            let _hosts = words.next()?;
            let _kind = words.next()?;
            russh::keys::parse_public_key_base64(words.next()?).ok()
        })
        .collect()
}

/// What a host key check decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Known,
    /// Unknown, and kept, as `accept-new` asks.
    Learn,
    /// Not checked, as `no` asks.
    Accept,
    Revoked,
    /// Another key of the same kind is kept for the host.
    Changed,
    Unknown,
    /// The host showed a certificate, which this can't check yet.
    Certificate,
}

impl Verdict {
    /// Whether the login goes on.
    fn trusts(self) -> bool {
        matches!(self, Self::Known | Self::Learn | Self::Accept)
    }

    /// Why the login stopped.
    fn refusal(self) -> DialError {
        DialError::refused(match self {
            Self::Revoked => "the device's host key is revoked in known_hosts",
            Self::Certificate => "the device shows a host certificate, which MaxHQ can't check yet",
            _ => "the host key isn't trusted, so ssh to it once by hand",
        })
    }
}

/// Checks the host key and keeps what it decided.
struct Checker {
    keys: HostKeys,
    policy: Policy,
    /// Where a new key goes, and the name it goes under.
    learn: Option<(PathBuf, String)>,
    verdict: Arc<Mutex<Option<Verdict>>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            *self.verdict.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(Verdict::Certificate);
            return Ok(false);
        };
        let verdict = self.keys.verdict(self.policy, key);
        if verdict == Verdict::Learn
            && let Some((file, name)) = &self.learn
            && let Err(err) = russh::keys::known_hosts::learn_known_hosts_path(name, 22, key, file)
        {
            debug!(%err, "couldn't keep the new host key");
        }
        *self.verdict.lock().unwrap_or_else(PoisonError::into_inner) = Some(verdict);
        Ok(verdict.trusts())
    }
}

/// Logs in over `stream` as `plan` says.
async fn log_in(stream: Box<dyn Io>, plan: &Plan) -> Result<Handle<Checker>, DialError> {
    let keys = HostKeys::read(plan)?;
    let config = Arc::new(client::Config {
        preferred: Preferred {
            key: Cow::Owned(keys.preferred()),
            ..Preferred::DEFAULT
        },
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: 3,
        inactivity_timeout: None,
        ..client::Config::default()
    });
    let verdict = Arc::new(Mutex::new(None));
    let checker = Checker {
        keys,
        policy: plan.policy,
        learn: plan
            .known_hosts
            .first()
            .map(|file| (file.clone(), plan.name.clone())),
        verdict: Arc::clone(&verdict),
    };
    let mut handle = match client::connect_stream(config, stream, checker).await {
        Ok(handle) => handle,
        Err(err) => {
            let verdict = *verdict.lock().unwrap_or_else(PoisonError::into_inner);
            return Err(match verdict {
                Some(verdict) if !verdict.trusts() => verdict.refusal(),
                _ => ssh_failure(&err),
            });
        }
    };
    sign_in(&mut handle, plan).await?;
    Ok(handle)
}

/// A key file's public half, and its private half when it opens without a
/// passphrase.
struct KeyFile {
    public: PublicKey,
    private: Option<PrivateKey>,
}

/// The key files that read, in order.
fn key_files(files: &[PathBuf]) -> Vec<KeyFile> {
    files
        .iter()
        .filter_map(|file| match russh::keys::load_secret_key(file, None) {
            Ok(private) => Some(KeyFile {
                public: private.public_key().clone(),
                private: Some(private),
            }),
            Err(err) => {
                // One behind a passphrase still names its key for the agent.
                let mut public = file.clone().into_os_string();
                public.push(".pub");
                let public = russh::keys::load_public_key(PathBuf::from(public)).ok()?;
                debug!(%err, file = %file.display(), "a key file doesn't open by itself");
                Some(KeyFile {
                    public,
                    private: None,
                })
            }
        })
        .collect()
}

/// An agent of either kind.
type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// Reaches the agent at `path`: a named pipe on Windows, a socket on Unix.
async fn agent(path: &str) -> Result<Agent, russh::keys::Error> {
    #[cfg(windows)]
    let agent = AgentClient::connect_named_pipe(path).await?.dynamic();
    #[cfg(unix)]
    let agent = AgentClient::connect_uds(path).await?.dynamic();
    Ok(agent)
}

/// The hash an RSA key signs with on this host, or none for other keys.
async fn rsa_hash(handle: &Handle<Checker>, key: &PublicKey) -> Option<HashAlg> {
    if !key.algorithm().is_rsa() {
        return None;
    }
    handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten()
}

/// Signs in with the agent's keys and then the key files.
async fn sign_in(handle: &mut Handle<Checker>, plan: &Plan) -> Result<(), DialError> {
    let files = key_files(&plan.key_files);
    let named = |key: &PublicKey| {
        files
            .iter()
            .any(|file| file.public.key_data() == key.key_data())
    };
    if let Some(path) = &plan.agent {
        match agent(path).await {
            Ok(mut agent) => {
                let identities = agent.request_identities().await.unwrap_or_else(|err| {
                    debug!(%err, "the SSH agent didn't list its keys");
                    Vec::new()
                });
                for identity in identities {
                    let AgentIdentity::PublicKey { key, .. } = identity else {
                        continue;
                    };
                    if plan.identities_only && !named(&key) {
                        continue;
                    }
                    let hash = rsa_hash(handle, &key).await;
                    match handle
                        .authenticate_publickey_with(&plan.user, key, hash, &mut agent)
                        .await
                    {
                        Ok(result) if result.success() => return Ok(()),
                        Ok(_) => {}
                        Err(err) => debug!(%err, "the SSH agent didn't sign"),
                    }
                }
            }
            Err(err) => debug!(%err, "no SSH agent"),
        }
    }
    for file in files {
        let Some(private) = file.private else {
            continue;
        };
        let hash = rsa_hash(handle, &file.public).await;
        let key = PrivateKeyWithHashAlg::new(Arc::new(private), hash);
        match handle.authenticate_publickey(&plan.user, key).await {
            Ok(result) if result.success() => return Ok(()),
            Ok(_) => {}
            Err(err) if handle.is_closed() => return Err(ssh_failure(&err)),
            Err(err) => debug!(%err, "a key file didn't sign"),
        }
    }
    Err(DialError::refused("SSH sign-in was refused"))
}

#[cfg(test)]
mod tests;
