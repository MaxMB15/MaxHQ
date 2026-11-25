// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The writable Incus client for container workspaces. It talks to
//! [`SOCKET`] and leaves the observer's read-only `observe/incus/api.rs` as
//! it is.
//!
//! It only ever names an instance through [`InstanceName`], which can't be
//! built for a name outside `mhq-`. Each write reads the instance first and
//! refuses one without `user.maxhq.managed=true`, except `create`.

pub(crate) mod http;
pub mod moves;
pub(crate) mod socket;
mod ws;

// The fake serves on a Unix socket, as Incus does.
#[cfg(all(test, unix))]
pub mod fake;
#[cfg(all(test, unix))]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::stream::{self, BoxStream, StreamExt};
use hyper::body::Bytes;
use hyper::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::host::ids;
use crate::recipe::{ExecOutput, AGENT, AGENT_HOME, AGENT_UID};
use ws::{opcode, IncusSocket};

/// The Incus daemon's socket.
pub const SOCKET: &str = "/var/lib/incus/unix.socket";

/// What every MaxHQ instance name starts with.
pub const PREFIX: &str = "mhq-";

/// The instance key that marks an instance as MaxHQ's to change.
pub const MANAGED_KEY: &str = "user.maxhq.managed";

/// The image server the `images:` remote names.
pub const IMAGE_SERVER: &str = "https://images.linuxcontainers.org";

/// The longest part after [`PREFIX`].
const MAX_SUFFIX: usize = 59;

/// How long an instance create may take, which includes the image
/// download the first time.
const CREATE_LIMIT: Duration = Duration::from_secs(15 * 60);

/// How long a command run through exec may take. The recipe's installs
/// are the slowest.
const EXEC_LIMIT: Duration = Duration::from_secs(30 * 60);

/// How long a delete or a forced stop may take.
const CHANGE_LIMIT: Duration = Duration::from_secs(5 * 60);

/// Where the guest listens inside a container, which the `maxhq-guest`
/// proxy device connects to.
const GUEST_LISTEN: &str = maxhq_core::paths::GUEST_HOST_SOCKET;

/// The proxy device that carries the guest's socket to the host.
const GUEST_DEVICE: &str = "maxhq-guest";

/// The expiry Incus reads as never.
pub const NEVER: &str = "0001-01-01T00:00:00Z";

/// The pause before the first reconnect of the event stream. It doubles up
/// to [`LAST_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(1);

/// The longest pause between reconnects of the event stream.
const LAST_RETRY: Duration = Duration::from_secs(30);

/// `mhq-`, then 1 to 59 of a to z, 0 to 9 and `-`, not ending in `-`.
/// Can't be built for any other name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceName(String);

/// Why a name isn't an [`InstanceName`].
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0:?} isn't a MaxHQ instance name, which is mhq- and 1 to 59 of a-z, 0-9 and -")]
pub struct NameError(pub String);

impl InstanceName {
    /// The name, if it's one MaxHQ may manage. Incus refuses a name that
    /// ends in `-`, so this does too.
    pub fn new(name: &str) -> Result<Self, NameError> {
        let suffix = name
            .strip_prefix(PREFIX)
            .ok_or_else(|| NameError(name.to_owned()))?;
        let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if suffix.is_empty()
            || suffix.len() > MAX_SUFFIX
            || !suffix.chars().all(allowed)
            || suffix.ends_with('-')
        {
            return Err(NameError(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    /// The name as Incus knows it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a new instance is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceSpec {
    /// The image alias, Debian 13 in M3.
    pub image: String,
    /// The instance keys, including every `user.maxhq.*` key.
    pub config: BTreeMap<String, String>,
    /// The storage pool for the root disk.
    pub pool: String,
    /// The network for the NIC.
    pub network: String,
    /// Where the `maxhq-guest` proxy device listens on the host.
    pub guest_socket: PathBuf,
}

/// A change of an instance's running state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateAction {
    /// Starts it.
    Start,
    /// Stops it.
    Stop,
    /// Stops and starts it. Nothing restarts a container yet.
    #[allow(dead_code)]
    Restart,
}

impl StateAction {
    /// The action as Incus names it.
    fn as_str(self) -> &'static str {
        match self {
            StateAction::Start => "start",
            StateAction::Stop => "stop",
            StateAction::Restart => "restart",
        }
    }
}

/// An instance lifecycle event from Incus's event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    /// The instance, as Incus named it, which may be no MaxHQ instance.
    pub instance: String,
    /// Incus's action, such as `instance-started`.
    pub action: String,
}

/// Why an Incus call failed.
#[derive(Debug, Error)]
pub enum IncusError {
    /// This build has no Incus client yet. The client this build has
    /// never gives it.
    #[allow(dead_code)]
    #[error("this maxd can't manage Incus instances yet")]
    NotYet,
    /// The instance lacks `user.maxhq.managed=true`, so MaxHQ won't touch it.
    #[error("{0} isn't managed by MaxHQ, so maxd won't change it")]
    Unmanaged(InstanceName),
    /// A move's write to a container the old scripts made, without the
    /// record and approval it needs.
    #[error("maxd won't change {name}: {why}")]
    Guarded {
        /// The container.
        name: String,
        /// Why not.
        why: String,
    },
    /// Incus answered with an error.
    #[error("Incus said {status}: {message}")]
    Api {
        /// The HTTP status.
        status: u16,
        /// Incus's message.
        message: String,
    },
    /// The socket failed.
    #[error("couldn't reach Incus: {0}")]
    Io(#[from] std::io::Error),
}

impl IncusError {
    /// True when Incus said the thing asked for isn't there.
    pub fn is_not_found(&self) -> bool {
        matches!(self, IncusError::Api { status: 404, .. })
    }
}

/// One instance, as Incus reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Instance {
    /// The instance name, which may be no MaxHQ instance.
    pub name: String,
    /// Such as `Running`, `Stopped` or `Frozen`.
    #[serde(default)]
    pub status: String,
    /// The instance's own keys, without its profiles'.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    /// The instance's own devices, without its profiles'.
    #[serde(default)]
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
}

impl Instance {
    /// The pool its root disk is on, when the instance has its own.
    pub fn pool(&self) -> Option<&str> {
        self.devices
            .get("root")
            .and_then(|root| root.get("pool"))
            .map(String::as_str)
    }

    /// The network its NIC is on, when the instance has its own.
    pub fn network(&self) -> Option<&str> {
        self.devices
            .get("eth0")
            .and_then(|nic| nic.get("network"))
            .map(String::as_str)
    }

    /// A key's value, unless it's unset or empty.
    pub fn key(&self, key: &str) -> Option<&str> {
        self.config
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// True when the instance carries `user.maxhq.managed=true` on itself.
    pub fn is_managed(&self) -> bool {
        self.key(MANAGED_KEY) == Some("true")
    }

    /// True while the container runs.
    pub fn is_running(&self) -> bool {
        self.status == "Running"
    }
}

/// One snapshot of an instance, as Incus reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Snapshot {
    /// The snapshot's own name, without its instance's.
    pub name: String,
    /// When it was taken, in RFC 3339.
    #[serde(default)]
    pub created_at: String,
    /// When Incus deletes it, in RFC 3339. Incus sends the year 1 for a
    /// snapshot that never expires.
    #[serde(default)]
    pub expires_at: String,
}

/// One network, as Incus reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Network {
    /// The network's name.
    pub name: String,
    /// Such as `bridge`, `physical` or `loopback`.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// True for a network Incus made and runs.
    #[serde(default)]
    pub managed: bool,
    /// `Created` once it's up.
    #[serde(default)]
    pub status: String,
    /// Its keys, such as `ipv4.address`.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

impl Network {
    /// True for a bridge Incus manages, which is what a workspace NIC
    /// attaches to.
    pub fn is_managed_bridge(&self) -> bool {
        self.managed && self.kind == "bridge"
    }

    /// The bridge's IPv4 address and prefix, such as `10.100.2.1/24`.
    pub fn ipv4(&self) -> Option<&str> {
        address(self.config.get("ipv4.address"))
    }

    /// The bridge's IPv6 address and prefix.
    pub fn ipv6(&self) -> Option<&str> {
        address(self.config.get("ipv6.address"))
    }
}

/// An address key's value, unless it's unset, `none` or `auto`.
fn address(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .filter(|value| !value.is_empty() && *value != "none" && *value != "auto")
}

/// One storage pool, as Incus reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Pool {
    /// The pool's name.
    pub name: String,
    /// Such as `btrfs` or `dir`.
    #[serde(default)]
    pub driver: String,
    /// `Created` once it's set up.
    #[serde(default)]
    pub status: String,
    /// Its keys, such as `source`.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

/// A client of the Incus daemon on this device.
#[derive(Clone, Debug)]
pub struct Incus {
    socket: PathBuf,
}

impl Incus {
    /// A client of the daemon at `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// The socket this client talks to.
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// Whether Incus answers at all.
    pub async fn ping(&self) -> Result<(), IncusError> {
        http::call(&self.socket, Method::GET, "/1.0", None).await?;
        Ok(())
    }

    /// Every instance in the default project, MaxHQ's or not.
    pub async fn instances(&self) -> Result<Vec<Instance>, IncusError> {
        let envelope = http::call(
            &self.socket,
            Method::GET,
            "/1.0/instances?recursion=1",
            None,
        )
        .await?;
        serde_json::from_value(envelope.metadata).map_err(|e| http::broken(e).into())
    }

    /// One instance, or None when there's none by that name.
    pub async fn instance(&self, name: &InstanceName) -> Result<Option<Instance>, IncusError> {
        match http::call(&self.socket, Method::GET, &instance_uri(name, ""), None).await {
            Ok(envelope) => serde_json::from_value(envelope.metadata)
                .map(Some)
                .map_err(|e| http::broken(e).into()),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// What a running instance has used: its CPU time in nanoseconds and
    /// its memory in bytes. None when there's no instance by that name.
    pub async fn usage(&self, name: &InstanceName) -> Result<Option<(u64, u64)>, IncusError> {
        match http::call(
            &self.socket,
            Method::GET,
            &instance_uri(name, "/state"),
            None,
        )
        .await
        {
            Ok(envelope) => {
                let read = |section: &str| {
                    envelope.metadata[section]["usage"]
                        .as_u64()
                        .unwrap_or_default()
                };
                Ok(Some((read("cpu"), read("memory"))))
            }
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Every network Incus knows, its own and the host's.
    pub async fn networks(&self) -> Result<Vec<Network>, IncusError> {
        let envelope =
            http::call(&self.socket, Method::GET, "/1.0/networks?recursion=1", None).await?;
        serde_json::from_value(envelope.metadata).map_err(|e| http::broken(e).into())
    }

    /// Every storage pool.
    pub async fn pools(&self) -> Result<Vec<Pool>, IncusError> {
        let envelope = http::call(
            &self.socket,
            Method::GET,
            "/1.0/storage-pools?recursion=1",
            None,
        )
        .await?;
        serde_json::from_value(envelope.metadata).map_err(|e| http::broken(e).into())
    }

    /// Makes a storage pool with `driver` and `config`, for the host
    /// spec's pool.
    pub async fn create_pool(
        &self,
        name: &str,
        driver: &str,
        config: BTreeMap<String, String>,
    ) -> Result<(), IncusError> {
        let body = json!({ "name": name, "driver": driver, "config": config });
        self.post("/1.0/storage-pools", &body).await
    }

    /// Makes a bridge with `config`, for the host spec's bridge.
    pub async fn create_bridge(
        &self,
        name: &str,
        config: BTreeMap<String, String>,
    ) -> Result<(), IncusError> {
        let body = json!({ "name": name, "type": "bridge", "config": config });
        self.post("/1.0/networks", &body).await
    }

    /// Sends a JSON body that Incus may take a while over, such as a pool
    /// that formats its disk.
    async fn post(&self, uri: &str, body: &Value) -> Result<(), IncusError> {
        let headers = [("content-type", "application/json".to_owned())];
        let bytes = Bytes::from(serde_json::to_vec(body).map_err(http::broken)?);
        let reply = http::send(
            &self.socket,
            Method::POST,
            uri,
            &headers,
            bytes,
            CHANGE_LIMIT,
        )
        .await?;
        http::envelope(&reply)?;
        Ok(())
    }

    /// Makes an instance. It doesn't start it.
    ///
    /// The instance gets no profiles, `user.maxhq.managed=true` whatever
    /// the spec says, a root disk on the spec's pool, a NIC on its network
    /// and the `maxhq-guest` proxy device. The proxy's host socket belongs
    /// to whoever owns the folder it sits in, which is this maxd.
    pub async fn create(&self, name: &InstanceName, spec: InstanceSpec) -> Result<(), IncusError> {
        let body = create_body(name, spec)?;
        http::run(
            &self.socket,
            Method::POST,
            "/1.0/instances",
            Some(&body),
            CREATE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Starts, stops or restarts an instance, waiting up to `timeout`.
    pub async fn set_state(
        &self,
        name: &InstanceName,
        action: StateAction,
        timeout: Duration,
    ) -> Result<(), IncusError> {
        self.managed(name).await?;
        self.change_state(name, action.as_str(), timeout, false)
            .await
    }

    /// Stops an instance at once, without waiting for it to shut down.
    pub async fn force_stop(&self, name: &InstanceName) -> Result<(), IncusError> {
        self.managed(name).await?;
        self.change_state(name, "stop", CHANGE_LIMIT, true).await
    }

    /// Deletes a stopped instance.
    pub async fn delete(&self, name: &InstanceName) -> Result<(), IncusError> {
        self.managed(name).await?;
        http::run(
            &self.socket,
            Method::DELETE,
            &instance_uri(name, ""),
            None,
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Sets instance keys, leaving the others as they are.
    pub async fn set_config(
        &self,
        name: &InstanceName,
        keys: BTreeMap<String, String>,
    ) -> Result<(), IncusError> {
        self.managed(name).await?;
        let body = json!({ "config": keys });
        http::run(
            &self.socket,
            Method::PATCH,
            &instance_uri(name, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Points a stopped instance's `maxhq-guest` proxy at `socket`, as a
    /// copy needs before its first start.
    pub async fn set_guest_socket(
        &self,
        name: &InstanceName,
        socket: &std::path::Path,
    ) -> Result<(), IncusError> {
        self.managed(name).await?;
        let body = json!({ "devices": { GUEST_DEVICE: guest_device(socket)? } });
        http::run(
            &self.socket,
            Method::PATCH,
            &instance_uri(name, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Adds a device unless the instance has one named `device`, and says
    /// whether it did. One that's there stays as it is, since replacing a
    /// disk on a running container pulls it out from under what reads it.
    pub async fn add_device(
        &self,
        name: &InstanceName,
        device: &str,
        keys: BTreeMap<String, String>,
    ) -> Result<bool, IncusError> {
        let instance = self.managed(name).await?;
        if instance.devices.contains_key(device) {
            return Ok(false);
        }
        let body = json!({ "devices": { device: keys } });
        http::run(
            &self.socket,
            Method::PATCH,
            &instance_uri(name, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(true)
    }

    /// Runs a command in an instance as `as_user`, or root, and collects
    /// what it printed.
    ///
    /// The agent user, uid 1000, gets its own `HOME`, `USER` and `LOGNAME`
    /// and starts in its home folder. Without `stdin` the command reads
    /// nothing and Incus records what it prints. With `stdin`, which is how
    /// a secret reaches a command, the call streams over WebSockets.
    pub async fn exec(
        &self,
        name: &InstanceName,
        cmd: &[&str],
        as_user: Option<u32>,
        stdin: Option<Vec<u8>>,
    ) -> Result<ExecOutput, IncusError> {
        self.managed(name).await?;
        let body = exec_body(cmd, as_user, stdin.is_some());
        match stdin {
            None => self.exec_recorded(name, &body).await,
            Some(stdin) => self.exec_streamed(name, &body, stdin, None).await,
        }
    }

    /// Runs a command as [`Self::exec`] does with `stdin`, and hands
    /// `stderr` each piece of its stderr as it comes, for a command that
    /// reports progress there.
    pub async fn exec_watched(
        &self,
        name: &InstanceName,
        cmd: &[&str],
        as_user: Option<u32>,
        stdin: Vec<u8>,
        stderr: &StderrWatch<'_>,
    ) -> Result<ExecOutput, IncusError> {
        self.managed(name).await?;
        let body = exec_body(cmd, as_user, true);
        self.exec_streamed(name, &body, stdin, Some(stderr)).await
    }

    /// Writes a file into an instance. Only for the guest binary and its
    /// unit, per A-14, and an app a lease installs in a test container.
    pub async fn push_file(
        &self,
        name: &InstanceName,
        path: &str,
        bytes: Vec<u8>,
        mode: u32,
    ) -> Result<(), IncusError> {
        self.push(name, path, Push::File, bytes, mode).await
    }

    /// Adds `bytes` to the end of a file in an instance, making it with
    /// `mode` when it isn't there, so a large file goes in pieces.
    pub async fn append_file(
        &self,
        name: &InstanceName,
        path: &str,
        bytes: Vec<u8>,
        mode: u32,
    ) -> Result<(), IncusError> {
        self.push(name, path, Push::Append, bytes, mode).await
    }

    /// Makes a folder in an instance. Its parent must be there.
    pub async fn make_folder(
        &self,
        name: &InstanceName,
        path: &str,
        mode: u32,
    ) -> Result<(), IncusError> {
        self.push(name, path, Push::Folder, Vec::new(), mode).await
    }

    /// An instance's snapshots, in the order Incus lists them.
    pub async fn snapshots(&self, name: &InstanceName) -> Result<Vec<Snapshot>, IncusError> {
        let uri = instance_uri(name, "/snapshots?recursion=1");
        let envelope = http::call(&self.socket, Method::GET, &uri, None).await?;
        serde_json::from_value(envelope.metadata).map_err(|e| http::broken(e).into())
    }

    /// Deletes one of an instance's snapshots.
    // Nothing deletes one yet: Incus expires its own, and maxd's named
    // ones stay until you delete them.
    #[allow(dead_code)]
    pub async fn delete_snapshot(
        &self,
        name: &InstanceName,
        snapshot: &str,
    ) -> Result<(), IncusError> {
        self.managed(name).await?;
        let rest = format!("/snapshots/{}", http::escape(snapshot));
        http::run(
            &self.socket,
            Method::DELETE,
            &instance_uri(name, &rest),
            None,
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Copies an instance, or one of its snapshots when `snapshot` names
    /// one, to a new stopped instance `to`. Both are MaxHQ's. The copy
    /// keeps the original's `maxhq-guest` socket until
    /// [`Incus::set_guest_socket`] gives it its own.
    pub async fn copy(
        &self,
        from: &InstanceName,
        snapshot: Option<&str>,
        to: &InstanceName,
    ) -> Result<(), IncusError> {
        self.managed(from).await?;
        let source = match snapshot {
            Some(snapshot) => format!("{from}/{snapshot}"),
            None => from.as_str().to_owned(),
        };
        let body = json!({
            "name": to.as_str(),
            "source": { "type": "copy", "source": source, "instance_only": true },
        });
        http::run(
            &self.socket,
            Method::POST,
            "/1.0/instances",
            Some(&body),
            CREATE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Takes a snapshot of an instance's disk, without its memory. It never
    /// expires, whatever the instance's `snapshots.expiry` says: Incus
    /// reads the year 1 as never.
    pub async fn snapshot(&self, name: &InstanceName, snapshot: &str) -> Result<(), IncusError> {
        self.managed(name).await?;
        let body = json!({ "name": snapshot, "stateful": false, "expires_at": NEVER });
        http::run(
            &self.socket,
            Method::POST,
            &instance_uri(name, "/snapshots"),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Takes an instance's disk, keys and devices back to a snapshot. Stop
    /// it first, so nothing it runs sees the change.
    pub async fn restore(&self, name: &InstanceName, snapshot: &str) -> Result<(), IncusError> {
        self.managed(name).await?;
        let body = json!({ "restore": snapshot });
        http::run(
            &self.socket,
            Method::PUT,
            &instance_uri(name, ""),
            Some(&body),
            CHANGE_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// Instance lifecycle events as they happen. When the stream drops, as
    /// it does when Incus restarts, it connects again after a pause that
    /// grows while connections keep failing. Events in a gap are lost, so
    /// a caller that cares rereads the instances now and then.
    pub fn lifecycle(&self) -> BoxStream<'static, Lifecycle> {
        let watch = Watch {
            socket: self.socket.clone(),
            events: None,
            pause: FIRST_RETRY,
        };
        stream::unfold(watch, |mut watch| async move {
            let event = watch.next().await;
            Some((event, watch))
        })
        .boxed()
    }

    /// Whether a storage pool can be used now. A locked pool can't.
    ///
    /// Incus has no field for a locked pool, so this checks that the pool
    /// exists, that its source folder is there on the host, and that Incus
    /// can read the pool's usage, which fails when its disk isn't mounted.
    pub async fn pool_available(&self, pool: &str) -> Result<bool, IncusError> {
        let uri = format!("/1.0/storage-pools/{}", http::escape(pool));
        let pool = match http::call(&self.socket, Method::GET, &uri, None).await {
            Ok(envelope) => serde_json::from_value::<Pool>(envelope.metadata)
                .map_err(|e| IncusError::from(http::broken(e)))?,
            Err(error) if error.is_not_found() => return Ok(false),
            Err(error) => return Err(error),
        };
        if pool.status != "Created" || !source_present(&pool) {
            return Ok(false);
        }
        let resources = format!("{uri}/resources");
        match http::call(&self.socket, Method::GET, &resources, None).await {
            Ok(_) => Ok(true),
            Err(IncusError::Api { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// One file push, owned by root.
    async fn push(
        &self,
        name: &InstanceName,
        path: &str,
        push: Push,
        bytes: Vec<u8>,
        mode: u32,
    ) -> Result<(), IncusError> {
        self.managed(name).await?;
        let uri = format!(
            "{}?path={}",
            instance_uri(name, "/files"),
            http::escape(path)
        );
        let headers = file_headers(push, mode);
        let reply = http::send(
            &self.socket,
            Method::POST,
            &uri,
            &headers,
            Bytes::from(bytes),
            EXEC_LIMIT,
        )
        .await?;
        http::envelope(&reply)?;
        Ok(())
    }

    /// Reads an instance and fails unless it's MaxHQ's to change.
    async fn managed(&self, name: &InstanceName) -> Result<Instance, IncusError> {
        let Some(instance) = self.instance(name).await? else {
            return Err(IncusError::Api {
                status: 404,
                message: format!("there's no instance named {name}"),
            });
        };
        if !instance.is_managed() {
            return Err(IncusError::Unmanaged(name.clone()));
        }
        Ok(instance)
    }

    /// Sends a state change and waits for it.
    async fn change_state(
        &self,
        name: &InstanceName,
        action: &str,
        timeout: Duration,
        force: bool,
    ) -> Result<(), IncusError> {
        let seconds = timeout.as_secs().max(1);
        let body = json!({ "action": action, "timeout": seconds, "force": force });
        http::run(
            &self.socket,
            Method::PUT,
            &instance_uri(name, "/state"),
            Some(&body),
            timeout + Duration::from_secs(30),
        )
        .await?;
        Ok(())
    }

    /// Runs a command that reads nothing. Incus records what it prints to
    /// log files, which this reads and then deletes.
    async fn exec_recorded(
        &self,
        name: &InstanceName,
        body: &Value,
    ) -> Result<ExecOutput, IncusError> {
        let uri = instance_uri(name, "/exec");
        let envelope = http::call(&self.socket, Method::POST, &uri, Some(body)).await?;
        let operation = http::started(&envelope)?;
        let finished = http::finished(&self.socket, &operation.id, EXEC_LIMIT).await?;
        let metadata = exec_ended(finished)?.metadata;
        let status = return_code(&metadata);
        let mut output = ExecOutput {
            status,
            ..ExecOutput::default()
        };
        let logs = metadata.get("output").cloned().unwrap_or_default();
        let mut failed = None;
        for (fd, target) in [("1", &mut output.stdout), ("2", &mut output.stderr)] {
            let Some(path) = logs.get(fd).and_then(Value::as_str) else {
                continue;
            };
            let reply = http::send(
                &self.socket,
                Method::GET,
                path,
                &[],
                Bytes::new(),
                http::TIMEOUT,
            )
            .await;
            // The log goes whatever the read did. One past the reply limit
            // is the last thing to leave on the host's disk.
            if let Err(error) = http::call(&self.socket, Method::DELETE, path, None).await {
                tracing::debug!(%error, path, "couldn't delete an exec log");
            }
            match reply {
                Ok(reply) if reply.status.is_success() => *target = reply.body.to_vec(),
                Ok(_) => {}
                Err(error) => failed = failed.or(Some(error)),
            }
        }
        match failed {
            Some(error) => Err(error),
            None => Ok(output),
        }
    }

    /// Runs a command with `stdin`, over one WebSocket per stream.
    async fn exec_streamed(
        &self,
        name: &InstanceName,
        body: &Value,
        stdin: Vec<u8>,
        watch: Option<&StderrWatch<'_>>,
    ) -> Result<ExecOutput, IncusError> {
        let envelope = http::call(
            &self.socket,
            Method::POST,
            &instance_uri(name, "/exec"),
            Some(body),
        )
        .await?;
        let operation = http::started(&envelope)?;
        let fds = &operation.metadata["fds"];
        let secret = |fd: &str| {
            fds.get(fd)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| http::broken(format!("Incus gave no secret for exec stream {fd}")))
        };
        let mut sockets = Vec::new();
        for fd in ["0", "1", "2", "control"] {
            let uri = format!(
                "/1.0/operations/{}/websocket?secret={}",
                http::escape(&operation.id),
                http::escape(&secret(fd)?)
            );
            sockets.push(ws::open(&self.socket, &uri).await?);
        }
        let control = sockets.pop();
        let mut stderr_socket = sockets.pop().ok_or_else(|| http::broken("no stderr"))?;
        let mut stdout_socket = sockets.pop().ok_or_else(|| http::broken("no stdout"))?;
        let mut stdin_socket = sockets.pop().ok_or_else(|| http::broken("no stdin"))?;

        let write = async {
            stdin_socket.send_message(opcode::BINARY, &stdin).await?;
            // An empty text message is Incus's end of stream.
            stdin_socket.send_message(opcode::TEXT, &[]).await?;
            stdin_socket.close().await
        };
        let (written, stdout, stderr) = tokio::join!(
            write,
            drain(&mut stdout_socket, None),
            drain(&mut stderr_socket, watch)
        );
        if let Err(error) = written {
            tracing::debug!(%error, "the exec's stdin closed early");
        }
        let finished = exec_ended(http::finished(&self.socket, &operation.id, EXEC_LIMIT).await?)?;
        drop(control);
        Ok(ExecOutput {
            status: return_code(&finished.metadata),
            stdout: stdout?,
            stderr: stderr?,
        })
    }
}

/// What hears an exec's stderr as it comes.
pub type StderrWatch<'a> = dyn Fn(&[u8]) + Send + Sync + 'a;

/// The most one exec stream keeps. A command in a container prints what it
/// likes, so the rest is read and dropped and this device's memory holds.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Reads an exec stream until it ends, handing each piece to `watch`.
async fn drain(
    socket: &mut IncusSocket,
    watch: Option<&StderrWatch<'_>>,
) -> Result<Vec<u8>, IncusError> {
    let mut out = Vec::new();
    while let Some(message) = socket.next().await? {
        if message.text {
            break;
        }
        if let Some(watch) = watch {
            watch(&message.payload);
        }
        keep_output(&mut out, &message.payload, MAX_OUTPUT);
    }
    Ok(out)
}

/// Adds `piece` to `out`, up to `limit` bytes in all.
fn keep_output(out: &mut Vec<u8>, piece: &[u8], limit: usize) {
    let room = limit.saturating_sub(out.len());
    if room > 0 && piece.len() > room {
        tracing::warn!(limit, "an exec printed too much, so the rest is dropped");
    }
    out.extend_from_slice(&piece[..room.min(piece.len())]);
}

/// An exec's operation once it ended, or its error if the command didn't
/// run. Incus marks an exec that exits 126 or 127 as failed, with "Command
/// not executable" or "Command not found", even when the command ran and
/// the code is its own, as `env` gives for a program that isn't there.
/// Such an operation still carries the code and the output, so it counts
/// as an exit.
fn exec_ended(operation: http::Operation) -> Result<http::Operation, IncusError> {
    if matches!(return_code(&operation.metadata), 126 | 127) {
        return Ok(operation);
    }
    operation.succeeded()
}

/// The exit status an exec operation reports.
fn return_code(metadata: &Value) -> i32 {
    metadata
        .get("return")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
        .unwrap_or(-1)
}

/// An instance's URI, with `rest` after it.
fn instance_uri(name: &InstanceName, rest: &str) -> String {
    format!("/1.0/instances/{}{rest}", name.as_str())
}

/// The body of an instance create.
fn create_body(name: &InstanceName, spec: InstanceSpec) -> Result<Value, IncusError> {
    let devices = devices(&spec.pool, &spec.network, &spec.guest_socket)?;
    let mut config = spec.config;
    config.insert(MANAGED_KEY.to_owned(), "true".to_owned());
    let alias = spec
        .image
        .strip_prefix("images:")
        .unwrap_or(&spec.image)
        .to_owned();
    Ok(json!({
        "name": name.as_str(),
        "type": "container",
        "source": {
            "type": "image",
            "alias": alias,
            "server": IMAGE_SERVER,
            "protocol": "simplestreams",
            "mode": "pull",
        },
        "profiles": [],
        "config": config,
        "devices": devices,
    }))
}

/// The devices MaxHQ gives its own instances: a root disk on `pool`, a
/// NIC on `network` and the `maxhq-guest` proxy.
fn devices(pool: &str, network: &str, guest_socket: &Path) -> Result<Value, IncusError> {
    Ok(json!({
        "root": { "type": "disk", "path": "/", "pool": pool },
        "eth0": { "type": "nic", "name": "eth0", "network": network },
        GUEST_DEVICE: guest_device(guest_socket)?,
    }))
}

/// The `maxhq-guest` proxy device: it listens on `socket` on the host,
/// which belongs to whoever owns the socket's folder.
fn guest_device(socket: &std::path::Path) -> Result<Value, IncusError> {
    let folder = socket
        .parent()
        .ok_or_else(|| http::broken("the guest socket has no folder"))?;
    let owner = std::fs::metadata(folder)?;
    let (uid, gid) = (ids::owner(&owner), ids::group(&owner));
    Ok(json!({
        "type": "proxy",
        "bind": "host",
        "listen": format!("unix:{}", socket.display()),
        "connect": format!("unix:{GUEST_LISTEN}"),
        "uid": uid.to_string(),
        "gid": gid.to_string(),
        "mode": "0600",
    }))
}

/// The body of an exec. `streamed` asks for WebSockets, for a command that
/// reads stdin.
fn exec_body(cmd: &[&str], as_user: Option<u32>, streamed: bool) -> Value {
    let uid = as_user.unwrap_or(0);
    let mut environment = BTreeMap::new();
    let mut cwd = "/root";
    if uid == AGENT_UID {
        environment.insert("HOME", AGENT_HOME);
        environment.insert("USER", AGENT);
        environment.insert("LOGNAME", AGENT);
        cwd = AGENT_HOME;
    } else if uid != 0 {
        cwd = "/";
    }
    json!({
        "command": cmd,
        "environment": environment,
        "wait-for-websocket": streamed,
        "record-output": !streamed,
        "interactive": false,
        "user": uid,
        "group": uid,
        "cwd": cwd,
    })
}

/// What a file push makes.
#[derive(Clone, Copy)]
enum Push {
    /// A file, replacing any there.
    File,
    /// More of a file.
    Append,
    /// A folder.
    Folder,
}

/// The headers of a file push.
fn file_headers(push: Push, mode: u32) -> Vec<(&'static str, String)> {
    let (kind, write) = match push {
        Push::File => ("file", "overwrite"),
        Push::Append => ("file", "append"),
        Push::Folder => ("directory", "overwrite"),
    };
    vec![
        ("content-type", "application/octet-stream".to_owned()),
        ("x-incus-type", kind.to_owned()),
        ("x-incus-mode", format!("{mode:04o}")),
        ("x-incus-uid", "0".to_owned()),
        ("x-incus-gid", "0".to_owned()),
        ("x-incus-write", write.to_owned()),
    ]
}

/// False when a pool's source is a host folder that isn't there, as when
/// the disk under it is locked. A source maxd can't look at counts as
/// there, and the usage check decides.
fn source_present(pool: &Pool) -> bool {
    let Some(source) = pool.config.get("source").filter(|s| s.starts_with('/')) else {
        return true;
    };
    match std::fs::metadata(Path::new(source)) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// The event stream as it reconnects.
struct Watch {
    socket: PathBuf,
    events: Option<IncusSocket>,
    pause: Duration,
}

impl Watch {
    /// The next lifecycle event, waiting through reconnects.
    async fn next(&mut self) -> Lifecycle {
        loop {
            let Some(events) = self.events.as_mut() else {
                match ws::open(&self.socket, "/1.0/events?type=lifecycle").await {
                    Ok(events) => {
                        tracing::debug!("reading Incus lifecycle events");
                        self.events = Some(events);
                    }
                    Err(error) => {
                        tracing::debug!(%error, "can't open the Incus event stream");
                        self.back_off().await;
                    }
                }
                continue;
            };
            match events.next().await {
                Ok(Some(message)) => {
                    if let Some(event) = parse_event(&message.payload) {
                        self.pause = FIRST_RETRY;
                        return event;
                    }
                }
                Ok(None) => {
                    self.events = None;
                    self.back_off().await;
                }
                Err(error) => {
                    tracing::debug!(%error, "the Incus event stream broke");
                    self.events = None;
                    self.back_off().await;
                }
            }
        }
    }

    /// Waits before the next connect, longer each time.
    async fn back_off(&mut self) {
        tokio::time::sleep(self.pause).await;
        self.pause = (self.pause * 2).min(LAST_RETRY);
    }
}

/// One event message.
#[derive(Deserialize)]
struct EventMessage {
    #[serde(rename = "type")]
    kind: String,
    metadata: Option<EventMetadata>,
}

/// A lifecycle event's details.
#[derive(Deserialize)]
struct EventMetadata {
    action: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    name: String,
}

/// Reads a lifecycle event about an instance. None for anything else.
fn parse_event(message: &[u8]) -> Option<Lifecycle> {
    let message: EventMessage = serde_json::from_slice(message).ok()?;
    let metadata = message.metadata?;
    if message.kind != "lifecycle" || !metadata.action.starts_with("instance-") {
        return None;
    }
    let instance = if metadata.name.is_empty() {
        let name = metadata.source.strip_prefix("/1.0/instances/")?;
        name.split(['/', '?']).next()?.to_owned()
    } else {
        metadata.name
    };
    Some(Lifecycle {
        instance,
        action: metadata.action,
    })
}
