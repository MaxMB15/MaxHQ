// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd guest --vm`, the guest in a macOS test VM. A LaunchAgent runs it
//! as the VM's logged-in user, since the Simulator needs a session. It
//! listens on vsock port [`GUEST_PORT`], which `maxd-vm` relays to the
//! host's guest link socket, and answers only lease requests.
//!
//! What it serves comes from the shared folder: the lease file names the
//! kind and the instance, and `maxd` there is the build to run. Each VM
//! resumes from the base's saved state with the guest the build left, so
//! at each new host link a guest whose build or instance is out of date
//! updates or exits, and launchd starts it again.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_core::leases::MAX_OUTPUT_BYTES;
use maxhq_hold::{Detach, Dirs, Holds};
use maxhq_protocol::guest::{HostToGuest, Placement};
use maxhq_protocol::ids::{AgentId, LeaseId, SessionId, StreamId, TransferId};
use maxhq_protocol::leases::{RunOutput, TestKind};
use maxhq_vm::{GUEST_PORT, GUEST_SHARE};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, OnceCell};

use super::lease::{self, Device, TestHost};
use super::link::{self, Incoming};
use super::{holds, Files, Outbox, Tunnels};
use crate::agent_device::allow::Platform;
use crate::agent_device::{self, AgentDevice, Config, Target};
use crate::leases::mac_host::{self, HostXcrun, Xcrun as _, SIMULATOR_NAME};
use crate::leases::mac_vm::{LeaseFile, LEASE_FILE, MAXD_FILE};

/// The guest's folder under the user's home.
const DATA: &str = ".maxhq";

/// The Simulator's UDID, in [`DATA`].
const UDID_FILE: &str = "simulator";

/// The `PATH` a macOS lease's programs get, with Node first.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// How long a screenshot of the VM's screen may take.
const SCREEN_WAIT: Duration = Duration::from_secs(30);

/// `maxd guest --vm`.
pub fn run() -> ExitCode {
    crate::init_logging();
    let runtime = match crate::multi_thread_runtime() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("couldn't start the runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("the VM's guest stopped: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> io::Result<()> {
    let home = std::env::home_dir().ok_or_else(|| io::Error::other("no home folder"))?;
    let data = home.join(DATA);
    let share = PathBuf::from(GUEST_SHARE);
    let instance = read_lease(&share)
        .map(|lease| lease.instance)
        .unwrap_or_default();
    let holds = Holds::new(Dirs {
        sessions: data.join("holds"),
        sockets: data.join("holders"),
    })
    .with_detach(Detach::Setsid);
    let holds = Arc::new(holds::GuestHolds::new(holds));
    let mut guest = link::Guest::new(
        holds.clone(),
        holds,
        Arc::new(|_, _, _, _| None),
        Arc::new(NoFiles),
        Arc::new(NoTunnels),
        instance.clone(),
        crate::identity::boot_id(),
    );
    guest.build = None;
    guest.test_host = Some(Arc::new(VmHost::new(home, share.clone())));
    let listener = Vsock::listen(GUEST_PORT)?;
    tracing::info!("the VM's guest listens on vsock port {GUEST_PORT}");
    link::accept(
        Arc::new(guest),
        Fresh {
            incoming: listener,
            share,
            instance,
        },
    )
    .await;
    Ok(())
}

fn read_lease(share: &Path) -> Option<LeaseFile> {
    let json = std::fs::read(share.join(LEASE_FILE)).ok()?;
    serde_json::from_slice(&json).ok()
}

/// Host links, each taken only while this guest is still the one the
/// shared folder asks for.
struct Fresh {
    incoming: Vsock,
    share: PathBuf,
    instance: String,
}

impl Incoming for Fresh {
    fn next(&mut self) -> BoxFuture<'_, io::Result<UnixStream>> {
        Box::pin(async move {
            let stream = self.incoming.next().await?;
            if let Err(err) = update(&self.share.join(MAXD_FILE)) {
                tracing::warn!("couldn't update the VM's guest: {err}");
            }
            let now = read_lease(&self.share).map(|lease| lease.instance);
            if now.is_some_and(|now| now != self.instance) {
                tracing::info!("the VM serves another lease now, so its guest starts again");
                std::process::exit(0);
            }
            Ok(stream)
        })
    }
}

/// Replaces this binary with `fresh` and exits, when their builds differ.
fn update(fresh: &Path) -> io::Result<()> {
    let Ok(build) = crate::identity::build_of(fresh) else {
        return Ok(());
    };
    if crate::identity::build() == Some(build.as_str()) {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let next = exe.with_extension("next");
    std::fs::copy(fresh, &next)?;
    std::fs::rename(&next, &exe)?;
    tracing::info!("the VM's guest is now build {build}, so it starts again");
    std::process::exit(0);
}

/// The vsock listener, accepted on a thread of its own, since tokio and
/// std take only Unix sockets.
struct Vsock(mpsc::Receiver<io::Result<UnixStream>>);

impl Vsock {
    fn listen(port: u32) -> io::Result<Self> {
        let listener = vsock_listener(port)?;
        let (tx, rx) = mpsc::channel(1);
        let handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || loop {
            let accepted = vsock_accept(&listener).and_then(|fd| {
                let stream = std::os::unix::net::UnixStream::from(fd);
                stream.set_nonblocking(true)?;
                let _entered = handle.enter();
                UnixStream::from_std(stream)
            });
            if tx.blocking_send(accepted).is_err() {
                return;
            }
        });
        Ok(Self(rx))
    }

    async fn next(&mut self) -> io::Result<UnixStream> {
        self.0
            .recv()
            .await
            .unwrap_or_else(|| Err(io::ErrorKind::BrokenPipe.into()))
    }
}

/// Keeps `fd` from the agents and commands this guest starts. macOS has no
/// `SOCK_CLOEXEC` or `accept4` to open it that way.
#[allow(unsafe_code)]
fn close_on_exec(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl takes no pointers here, and fd stays open through the call.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A vsock stream socket listening on `port` for any CID.
#[allow(unsafe_code)]
fn vsock_listener(port: u32) -> io::Result<OwnedFd> {
    // SAFETY: socket takes no pointers, and a fd it returns is ours.
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a socket nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    close_on_exec(&fd)?;
    let addr = libc::sockaddr_vm {
        svm_len: size_of::<libc::sockaddr_vm>() as u8,
        svm_family: libc::AF_VSOCK as libc::sa_family_t,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: libc::VMADDR_CID_ANY,
    };
    // SAFETY: addr is a valid sockaddr_vm of the given length.
    let bound = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&raw const addr).cast(),
            size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    };
    // SAFETY: listen takes no pointers.
    if bound < 0 || unsafe { libc::listen(fd.as_raw_fd(), 4) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// The next connection on `listener`, blocking until one comes.
#[allow(unsafe_code)]
fn vsock_accept(listener: &OwnedFd) -> io::Result<OwnedFd> {
    loop {
        // SAFETY: null address pointers ask for no peer address.
        let fd = unsafe {
            libc::accept(
                listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if fd >= 0 {
            // SAFETY: accept returned a new fd nothing else owns.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            close_on_exec(&fd)?;
            return Ok(fd);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// The VM's test host: its Simulator for an `ios_simulator` lease, and
/// the VM itself for a `macos` one, as the lease file says.
struct VmHost {
    home: PathBuf,
    share: PathBuf,
    /// The Simulator and its agent-device, made at the first iOS request.
    device: OnceCell<Device>,
}

impl VmHost {
    fn new(home: PathBuf, share: PathBuf) -> Self {
        Self {
            home,
            share,
            device: OnceCell::new(),
        }
    }

    fn kind(&self) -> TestKind {
        read_lease(&self.share).map_or(TestKind::Macos, |lease| lease.kind)
    }

    /// The Simulator, made and booted if it isn't yet.
    async fn device(&self) -> Result<&Device, String> {
        self.device.get_or_try_init(|| self.make_device()).await
    }

    async fn make_device(&self) -> Result<Device, String> {
        let data = self.home.join(DATA);
        let xcrun = &HostXcrun;
        let kept = data.join(UDID_FILE);
        let udid = match std::fs::read_to_string(&kept) {
            Ok(udid) if mac_host::exists(xcrun, udid.trim()).await? => udid.trim().to_owned(),
            _ => {
                let listed = xcrun
                    .simctl(&["list", "runtimes", "--json"], Duration::from_secs(60))
                    .await?;
                let runtime = crate::leases::detect::newest_ios_runtime(&listed)
                    .ok_or("The VM's Xcode has no iOS runtime.")?;
                let udid = mac_host::create(xcrun, SIMULATOR_NAME, &runtime.identifier).await?;
                std::fs::create_dir_all(&data)
                    .and_then(|()| std::fs::write(&kept, &udid))
                    .map_err(|err| format!("couldn't keep the Simulator's UDID: {err}"))?;
                udid
            }
        };
        mac_host::boot(xcrun, &udid).await?;
        let node = data.join("node").join("bin");
        let config = Config {
            node: node.join("node"),
            npm: node.join("npm"),
            ..Config::under(&data, "vm")
        };
        let agent = AgentDevice::new(config);
        agent.start().await?;
        let target = Target {
            platform: Platform::Ios,
            id: udid,
        };
        Ok(Device::new(agent, target))
    }

    /// Runs `command` as this user in their home folder.
    async fn run_here(&self, command: Vec<String>, timeout: Duration) -> Result<RunOutput, String> {
        let Some((program, args)) = command.split_first() else {
            return Err("Give a program to run, such as sw_vers.".to_owned());
        };
        let node = self.home.join(DATA).join("node").join("bin");
        let mut process = std::process::Command::new(program);
        process
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", format!("{}:{PATH}", node.display()))
            .current_dir(&self.home);
        if let Ok(user) = std::env::var("USER") {
            process.env("USER", user);
        }
        agent_device::process::run(process, timeout, MAX_OUTPUT_BYTES)
            .await
            .map_err(|err| format!("couldn't run {program}: {err}"))
    }

    /// A PNG of the VM's screen.
    async fn screen(&self) -> Result<Vec<u8>, String> {
        let file = std::env::temp_dir().join(format!("maxhq-screen-{}.png", std::process::id()));
        let mut command = std::process::Command::new("/usr/sbin/screencapture");
        command.args(["-x", "-t", "png"]).arg(&file);
        let ran = agent_device::process::run(command, SCREEN_WAIT, MAX_OUTPUT_BYTES)
            .await
            .map_err(|err| format!("couldn't take a screenshot: {err}"));
        let png = std::fs::read(&file);
        let _ = std::fs::remove_file(&file);
        match (ran?.exit_code, png) {
            (Some(0), Ok(png)) => Ok(png),
            _ => Err("The VM's screen couldn't be captured.".to_owned()),
        }
    }
}

impl TestHost for VmHost {
    fn run(
        &self,
        lease: LeaseId,
        command: Vec<String>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<RunOutput, String>> {
        Box::pin(async move {
            match self.kind() {
                TestKind::IosSimulator => self.device().await?.run(lease, command, timeout).await,
                _ => self.run_here(command, timeout).await,
            }
        })
    }

    fn screenshot(&self, lease: LeaseId) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            match self.kind() {
                TestKind::IosSimulator => self.device().await?.screenshot(lease).await,
                _ => self.screen().await,
            }
        })
    }

    fn install(&self, lease: LeaseId, path: PathBuf) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            match self.kind() {
                TestKind::IosSimulator => self.device().await?.install(lease, path).await,
                _ => lease::copy_in(&path, &self.home, None),
            }
        })
    }

    fn close(&self, lease: LeaseId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            match self.kind() {
                TestKind::IosSimulator => self.device().await?.close(lease).await,
                _ => Ok(()),
            }
        })
    }
}

/// No transfers: a lease's uploads come through the shared folder.
struct NoFiles;

impl Files for NoFiles {
    fn message(&self, _message: HostToGuest, _outbox: &Outbox) {}

    fn chunk(&self, _transfer: TransferId, _offset: u64, _data: Vec<u8>, _outbox: &Outbox) {}

    fn link_down(&self) {}

    fn held(
        &self,
        _session: SessionId,
        _args: Vec<String>,
    ) -> crate::session::clock::BoxFuture<()> {
        Box::pin(async {})
    }

    fn forgotten(&self, _session: SessionId) -> crate::session::clock::BoxFuture<()> {
        Box::pin(async {})
    }

    fn keep_picture(&self, _agent: AgentId, _media_type: String, _data: String) {}

    fn place(
        &self,
        _staged: TransferId,
        _cwd: String,
        _overwrite: bool,
    ) -> crate::session::clock::BoxFuture<Result<Placement, String>> {
        Box::pin(async { Err("A test VM has no checkout to place files in.".to_owned()) })
    }
}

/// No ports or tunnels.
struct NoTunnels;

impl Tunnels for NoTunnels {
    fn link_up(&self, _link: u64, _outbox: &Outbox) {}

    fn message(&self, _message: HostToGuest, _outbox: &Outbox) {}

    fn data(&self, _stream: StreamId, _data: Vec<u8>, _outbox: &Outbox) {}

    fn link_down(&self, _link: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn host(dir: &TempDir, kind: TestKind) -> VmHost {
        let (home, share) = (dir.path().join("home"), dir.path().join("share"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&share).unwrap();
        let lease = LeaseFile {
            kind,
            instance: "mhq-vm-test".into(),
        };
        std::fs::write(share.join(LEASE_FILE), serde_json::to_vec(&lease).unwrap()).unwrap();
        VmHost::new(home, share)
    }

    #[test]
    #[allow(unsafe_code)]
    fn what_this_guest_starts_doesnt_get_its_vsock_listener() {
        // A machine that can't open vsock has nothing to check.
        let Ok(listener) = vsock_listener(47_411) else {
            return;
        };
        // SAFETY: fcntl takes no pointers here, and the fd is open.
        let flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) };
        assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    }

    #[tokio::test]
    async fn a_macos_lease_runs_as_this_user_at_home_and_installs_there() {
        let dir = TempDir::new();
        let host = host(&dir, TestKind::Macos);
        assert_eq!(host.kind(), TestKind::Macos);
        let lease = LeaseId::derive("lease");
        let command = ["/bin/sh", "-c", "pwd; echo $HOME; echo $PATH"]
            .map(str::to_owned)
            .to_vec();
        let ran = host
            .run(lease, command, Duration::from_secs(10))
            .await
            .unwrap();
        let home = std::fs::canonicalize(&host.home).unwrap();
        let node = host.home.join(DATA).join("node").join("bin");
        assert_eq!(
            ran.stdout,
            format!(
                "{}\n{}\n{}:{PATH}\n",
                home.display(),
                host.home.display(),
                node.display()
            )
        );
        let upload = dir.path().join("share").join("uploads").join("0");
        std::fs::create_dir_all(&upload).unwrap();
        std::fs::write(upload.join("tool"), "bytes").unwrap();
        host.install(lease, upload.join("tool")).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(host.home.join("tool")).unwrap(),
            "bytes"
        );
        host.close(lease).await.unwrap();
    }

    #[test]
    fn the_lease_file_names_the_kind() {
        let dir = TempDir::new();
        assert_eq!(
            host(&dir, TestKind::IosSimulator).kind(),
            TestKind::IosSimulator
        );
        let none = VmHost::new(dir.path().join("home"), dir.path().join("none"));
        assert_eq!(none.kind(), TestKind::Macos);
    }
}
