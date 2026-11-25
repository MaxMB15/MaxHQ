// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest role itself: the folders and sockets it serves on, what the
//! link calls for transfers and tunnels, and the helper subcommands.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use maxhq_core::paths::{GUEST_DATA_DIR, GUEST_RUNTIME_DIR};
use maxhq_hold::{Detach, Dirs, Holds};
use maxhq_protocol::guest::{
    GuestToHost, HostToGuest, Placement, AGENT_SOCKET, HOLD_DIR, HOST_SOCKET,
};
use maxhq_protocol::ids::{AgentId, BootId, SessionId, StreamId, TransferId};
use maxhq_protocol::model::Vendor;
use maxhq_wire::Closed;
use tokio::net::UnixListener;

use super::{agent, catalog, helper};
use super::{files, holds, lease, link, ports, ssh, Outbox};
use crate::guestlink::wire::SendError;
use crate::session::clock::BoxFuture;
use crate::session::vendors::{History, Vendors, Wired};

/// The folder that holds the guest's SSH host key, under its data folder.
const SSH_DIR: &str = "ssh";

/// The host key's file in [`SSH_DIR`].
const HOST_KEY: &str = "host_ed25519";

/// Where the kernel names this boot.
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// Where the kernel names this container, which Incus sets to the
/// instance's name.
const HOSTNAME: &str = "/proc/sys/kernel/hostname";

/// Where the guest takes transfer traffic.
pub(crate) trait Files: Send + Sync + 'static {
    /// `UploadBegin`, `UploadCancel`, `FetchBegin`, `FetchFiles` or
    /// `TransferCredit` from the host. Answers go out on `outbox`.
    fn message(&self, message: HostToGuest, outbox: &Outbox);
    /// Upload bytes from the host.
    fn chunk(&self, transfer: TransferId, offset: u64, data: Vec<u8>, outbox: &Outbox);
    /// The host link dropped, so transfers on it end.
    fn link_down(&self);
    /// A holder for `session` started a CLI with `args`, which may name
    /// agents' attachment folders.
    fn held(&self, session: SessionId, args: Vec<String>) -> BoxFuture<()>;
    /// The host forgot `session`, so the attachment folders only it named
    /// go.
    fn forgotten(&self, session: SessionId) -> BoxFuture<()>;
    /// `KeepPicture` from the host: a picture from `agent`'s tool result
    /// to write into its folder. It doesn't wait.
    fn keep_picture(&self, agent: AgentId, media_type: String, data: String);
    /// `Place` from the host: moves the upload staged as `staged` into the
    /// checkout at `cwd`.
    fn place(
        &self,
        staged: TransferId,
        cwd: String,
        overwrite: bool,
    ) -> BoxFuture<Result<Placement, String>>;
}

/// Where the guest takes port and tunnel traffic.
pub(crate) trait Tunnels: Send + Sync + 'static {
    /// Host link `link` is up, and talks through `outbox`.
    fn link_up(&self, link: u64, outbox: &Outbox);
    /// `WatchPorts`, `TunnelOpen`, `TunnelCredit` or `TunnelClose` from the
    /// host. Answers go out on `outbox`.
    fn message(&self, message: HostToGuest, outbox: &Outbox);
    /// Bytes on a tunnel stream from the host.
    fn data(&self, stream: StreamId, data: Vec<u8>, outbox: &Outbox);
    /// Host link `link` dropped, so its streams end.
    fn link_down(&self, link: u64);
}

impl files::Out for Outbox {
    fn message(&self, message: GuestToHost) -> BoxFuture<Result<(), Closed>> {
        let outbox = self.clone();
        Box::pin(async move { outbox.send(&message).await.map_err(closed) })
    }

    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>> {
        let outbox = self.clone();
        Box::pin(async move {
            outbox
                .transfer_chunks(transfer, offset, &data)
                .await
                .map_err(closed)
        })
    }
}

/// A send to the host that failed, as a transfer sees it: the link is gone
/// for the transfer either way.
fn closed(err: SendError) -> Closed {
    if let SendError::Encode(err) = err {
        tracing::warn!(%err, "a transfer message for the host didn't encode");
    }
    Closed
}

impl Files for files::Files {
    fn message(&self, message: HostToGuest, outbox: &Outbox) {
        files::Files::message(self, message, Arc::new(outbox.clone()));
    }

    fn chunk(&self, transfer: TransferId, offset: u64, data: Vec<u8>, _outbox: &Outbox) {
        files::Files::chunk(self, transfer, offset, data);
    }

    fn link_down(&self) {
        files::Files::link_down(self);
    }

    fn held(&self, session: SessionId, args: Vec<String>) -> BoxFuture<()> {
        let files = self.clone();
        Box::pin(async move { files::Files::held(&files, session, args).await })
    }

    fn forgotten(&self, session: SessionId) -> BoxFuture<()> {
        let files = self.clone();
        Box::pin(async move { files::Files::forgotten(&files, session).await })
    }

    fn keep_picture(&self, agent: AgentId, media_type: String, data: String) {
        files::Files::keep_picture(self, agent, media_type, data);
    }

    fn place(
        &self,
        staged: TransferId,
        cwd: String,
        overwrite: bool,
    ) -> BoxFuture<Result<Placement, String>> {
        let files = self.clone();
        Box::pin(async move { files::Files::place(&files, staged, cwd, overwrite).await })
    }
}

/// `maxd guest`: runs the guest role. A `test_host` guest also answers a
/// lease's requests, running each program as the agent user.
pub fn run(test_host: bool) -> ExitCode {
    crate::init_logging();
    let runtime = match crate::multi_thread_runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("couldn't start the runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(test_host)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("the guest stopped: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Sets up the folders and sockets, then serves for good.
async fn serve(test_host: bool) -> io::Result<()> {
    let runtime = Path::new(GUEST_RUNTIME_DIR);
    make_dir(runtime, 0o755)?;
    let sockets = runtime.join("holders");
    make_dir(&sockets, 0o700)?;
    make_dir(Path::new(GUEST_DATA_DIR), 0o755)?;
    make_dir(Path::new(HOLD_DIR), 0o700)?;
    let ssh_dir = Path::new(GUEST_DATA_DIR).join(SSH_DIR);
    make_dir(&ssh_dir, 0o700)?;
    let host_key = ssh::host_key(&ssh_dir.join(HOST_KEY))?;
    let ssh = ssh::Ssh::new(host_key, ssh::Login::container()).map_err(io::Error::other)?;

    // Each holder runs in a system scope of its own, so it outlives this
    // service's restarts and upgrades.
    let holds = Holds::new(Dirs {
        sessions: PathBuf::from(HOLD_DIR),
        sockets,
    })
    .with_detach(Detach::SystemScope);
    let holds = Arc::new(holds::GuestHolds::new(holds));

    let instance = read_trimmed(HOSTNAME).unwrap_or_else(|err| {
        tracing::warn!("couldn't read the container's name: {err}");
        String::new()
    });
    let boot_id = BootId(read_trimmed(BOOT_ID)?);
    let host = bind(Path::new(HOST_SOCKET), 0o600)?;
    let agent = bind(Path::new(AGENT_SOCKET), 0o666)?;

    let mut guest = link::Guest::new(
        holds.clone(),
        holds,
        Arc::new(open_history),
        Arc::new(files::Files::owned_by(
            files::Config::container(),
            link::AGENT,
        )),
        Arc::new(ports::Ports::start().with_ssh(Arc::new(ssh))),
        instance,
        boot_id,
    );
    if test_host {
        let host = lease::Linux::user(link::AGENT).map_err(io::Error::other)?;
        guest.test_host = Some(Arc::new(host));
    }
    let guest = Arc::new(guest);
    tracing::info!("the guest listens on {HOST_SOCKET} and {AGENT_SOCKET}");
    tokio::spawn(agent::accept(guest.clone(), agent, PathBuf::from("/proc")));
    link::accept(guest, host).await;
    Ok(())
}

/// The vendor's own history reader on the container's files.
fn open_history(
    vendor: Vendor,
    config_dir: &Path,
    session: &str,
    agent: AgentId,
) -> Option<Box<dyn History>> {
    let pictures = Some(PathBuf::from(maxhq_protocol::guest::ATTACHMENTS_DIR));
    Wired { pictures }.history(vendor, config_dir, session, None, agent)
}

/// Makes `dir` with `mode`, or sets the mode of the one already there.
fn make_dir(dir: &Path, mode: u32) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(mode).create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(mode))
}

/// Listens on `path` with `mode`, replacing a socket an earlier run left.
fn bind(path: &Path, mode: u32) -> io::Result<UnixListener> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(listener)
}

/// A one-line file without its newline.
fn read_trimmed(path: &str) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_owned())
}

/// `maxd guest token codex`: prints one Codex access token, as Codex's
/// auth command.
pub fn token_codex() -> ExitCode {
    helper::token_codex(Path::new(AGENT_SOCKET))
}

/// `maxd guest codex <program> [args...]`: fetches Codex's model catalog
/// into `CODEX_HOME`, then runs the app-server in this process with the
/// file as its catalog.
pub fn codex(command: &[&str]) -> ExitCode {
    catalog::codex(Path::new(AGENT_SOCKET), command)
}

/// `maxd guest git-credential <action>`: git's credential helper protocol,
/// for github.com only. `get` asks the host for a token, and `store` and
/// `erase` do nothing.
pub fn git_credential(action: &str) -> ExitCode {
    helper::git_credential(Path::new(AGENT_SOCKET), action)
}

/// `maxd guest github-token`: prints a token for the workspace's repo, for
/// the `gh` wrapper.
pub fn github_token() -> ExitCode {
    helper::github_token(Path::new(AGENT_SOCKET))
}
