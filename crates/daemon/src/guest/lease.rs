// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A guest that's a test host answers the host's `Lease*` requests. In a
//! macOS VM it drives the VM's Simulator through agent-device; in the
//! `linux` container it runs one program at a time as the test user. A
//! guest that isn't a test host refuses them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use maxhq_core::leases::{MAX_OUTPUT_BYTES, MAX_RUN_SECONDS, MAX_SCREEN_EDGE};
use maxhq_hold::RunAs;
use maxhq_protocol::agents::ToolImage;
use maxhq_protocol::guest::{GuestToHost, HostToGuest, MAX_MESSAGE_LEN};
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::RunOutput;

use crate::agent_device::{self, AgentDevice, Target};

/// What a guest that isn't a test host answers.
pub(crate) const NOT_A_TEST_HOST: &str = "This machine isn't a test host.";

/// What a test host does for a lease.
pub(crate) trait TestHost: Send + Sync + 'static {
    /// Runs `command` for `lease`, for at most `timeout`.
    fn run(
        &self,
        lease: LeaseId,
        command: Vec<String>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<RunOutput, String>>;
    /// A PNG of the screen `lease` drives.
    fn screenshot(&self, lease: LeaseId) -> BoxFuture<'_, Result<Vec<u8>, String>>;
    /// Installs the upload at `path` for `lease`.
    fn install(&self, lease: LeaseId, path: PathBuf) -> BoxFuture<'_, Result<(), String>>;
    /// Ends `lease`'s session.
    fn close(&self, lease: LeaseId) -> BoxFuture<'_, Result<(), String>>;
}

/// The answer to a `Lease*` request, or none for any other message.
pub(crate) async fn answer(
    host: Option<&Arc<dyn TestHost>>,
    message: HostToGuest,
) -> Option<GuestToHost> {
    let refused = || NOT_A_TEST_HOST.to_owned();
    Some(match message {
        HostToGuest::LeaseRun {
            req,
            lease,
            command,
            timeout_seconds,
        } => {
            let timeout = Duration::from_secs(timeout_seconds.min(MAX_RUN_SECONDS).into());
            let result = match host {
                Some(host) => host.run(lease, command, timeout).await,
                None => Err(refused()),
            };
            GuestToHost::LeaseRan { req, result }
        }
        HostToGuest::LeaseScreenshot { req, lease } => {
            let result = match host {
                Some(host) => host.screenshot(lease).await.and_then(|png| image(&png)),
                None => Err(refused()),
            };
            GuestToHost::LeaseScreen { req, result }
        }
        HostToGuest::LeaseInstall { req, lease, path } => {
            let result = match host {
                Some(host) => host.install(lease, PathBuf::from(path)).await,
                None => Err(refused()),
            };
            GuestToHost::LeaseDone { req, result }
        }
        HostToGuest::LeaseClose { req, lease } => {
            let result = match host {
                Some(host) => host.close(lease).await,
                None => Err(refused()),
            };
            GuestToHost::LeaseDone { req, result }
        }
        _ => return None,
    })
}

/// The screenshot `png`, shrunk to the size the lease tools show, as a
/// tool image.
fn image(png: &[u8]) -> Result<ToolImage, String> {
    let png = crate::thumbnail::png_within(png, MAX_SCREEN_EDGE)
        .map_err(|err| format!("The screenshot couldn't be read: {err}"))?;
    encoded(&png)
}

/// `png` as a tool image, if its message fits on the link.
fn encoded(png: &[u8]) -> Result<ToolImage, String> {
    // Room for the rest of the message around the picture.
    if png.len().div_ceil(3) * 4 > MAX_MESSAGE_LEN - 1024 {
        return Err("The screenshot is too large to send.".to_owned());
    }
    Ok(ToolImage {
        mime: "image/png".to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

/// A test host whose leases drive one Simulator or emulator through
/// agent-device, as in a macOS VM, where `maxd guest --vm` makes one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Device {
    agent: AgentDevice,
    target: Target,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl Device {
    /// Leases drive `target` with `agent`.
    pub(crate) fn new(agent: AgentDevice, target: Target) -> Self {
        Self { agent, target }
    }
}

impl TestHost for Device {
    fn run(
        &self,
        lease: LeaseId,
        command: Vec<String>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<RunOutput, String>> {
        Box::pin(async move {
            let session = agent_device::session(lease);
            self.agent
                .run(&self.target, &session, &command, timeout)
                .await
        })
    }

    fn screenshot(&self, lease: LeaseId) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            let session = agent_device::session(lease);
            self.agent.screenshot(&self.target, &session).await
        })
    }

    fn install(&self, lease: LeaseId, path: PathBuf) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let session = agent_device::session(lease);
            self.agent.install_app(&self.target, &session, &path).await
        })
    }

    fn close(&self, lease: LeaseId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let session = agent_device::session(lease);
            self.agent.close(&self.target, &session).await
        })
    }
}

/// A test host whose leases run programs as the test user, as in the
/// `linux` container. The container is restored for each lease, so a
/// lease's close has nothing to undo. `maxd guest --test-host` makes one.
pub(crate) struct Linux {
    /// The test user, or none to run as `maxd` itself, as in tests.
    user: Option<RunAs>,
    /// The folder programs run in and installs land in.
    home: PathBuf,
}

impl Linux {
    /// Leases run as `user`, in its home folder. A user that is root is
    /// refused, as SSH sessions refuse one.
    pub(crate) fn user(name: &str) -> Result<Self, String> {
        let user =
            RunAs::lookup(name).map_err(|err| format!("couldn't find user {name}: {err}"))?;
        if user.uid == 0 {
            return Err(format!("user {name} is root"));
        }
        Ok(Self {
            home: user.home.clone(),
            user: Some(user),
        })
    }

    /// Leases run as `maxd`'s own user, in `home`.
    #[cfg(test)]
    pub(crate) fn here(home: PathBuf) -> Self {
        Self { user: None, home }
    }
}

impl TestHost for Linux {
    fn run(
        &self,
        _lease: LeaseId,
        command: Vec<String>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<RunOutput, String>> {
        Box::pin(async move {
            let Some((program, args)) = command.split_first() else {
                return Err("Give a program to run, such as uname -a.".to_owned());
            };
            if self.user.is_none() && rustix::process::geteuid().is_root() {
                return Err("commands don't run as root".to_owned());
            }
            let mut process = std::process::Command::new(program);
            process.args(args).env_clear();
            match &self.user {
                Some(user) => {
                    let home = self.home.clone();
                    let user = user.clone();
                    let login = tokio::task::spawn_blocking(move || user.login(&home))
                        .await
                        .map_err(|err| err.to_string())??;
                    process.envs(login.env().iter().map(|(key, value)| (key, value)));
                    process.env("PATH", PATH);
                    login.apply(&mut process)?;
                }
                None => {
                    process.env("PATH", PATH).current_dir(&self.home);
                }
            }
            agent_device::process::run(process, timeout, MAX_OUTPUT_BYTES)
                .await
                .map_err(|err| format!("couldn't run {program}: {err}"))
        })
    }

    fn screenshot(&self, _lease: LeaseId) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async { Err("A Linux lease has no screen.".to_owned()) })
    }

    fn install(&self, _lease: LeaseId, path: PathBuf) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { copy_in(&path, &self.home, self.user.as_ref()) })
    }

    fn close(&self, _lease: LeaseId) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// The `PATH` a Linux lease's programs get.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Moves the upload at `path`, with whatever else its folder holds, into
/// `home`, owned by `owner`.
pub(super) fn copy_in(path: &Path, home: &Path, owner: Option<&RunAs>) -> Result<(), String> {
    let top = path
        .parent()
        .ok_or_else(|| format!("{} isn't an upload.", path.display()))?;
    let placed = crate::transfer::deliver::place(top, home, true, owner)?;
    match placed.failed.first() {
        None => Ok(()),
        Some(failed) => Err(format!(
            "couldn't install {}: {}",
            failed.path, failed.reason
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn run(command: &[&str]) -> HostToGuest {
        HostToGuest::LeaseRun {
            req: 1,
            lease: LeaseId::derive("lease"),
            command: command.iter().map(|arg| (*arg).to_owned()).collect(),
            timeout_seconds: 10,
        }
    }

    #[tokio::test]
    async fn only_lease_requests_get_an_answer() {
        assert_eq!(answer(None, HostToGuest::Ping { nonce: 1 }).await, None);
    }

    #[tokio::test]
    async fn a_linux_lease_runs_a_program_in_its_folder() {
        let dir = TempDir::new();
        let host: Arc<dyn TestHost> = Arc::new(Linux::here(dir.path().to_owned()));
        let answer = answer(Some(&host), run(&["/bin/sh", "-c", "pwd; echo $PATH"])).await;
        let Some(GuestToHost::LeaseRan {
            req: 1,
            result: Ok(ran),
        }) = answer
        else {
            panic!("{answer:?}");
        };
        let folder = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(ran.stdout, format!("{}\n{PATH}\n", folder.display()));
        assert_eq!(ran.exit_code, Some(0));
    }

    #[tokio::test]
    async fn a_linux_lease_says_why_a_program_didnt_run() {
        let dir = TempDir::new();
        let host: Arc<dyn TestHost> = Arc::new(Linux::here(dir.path().to_owned()));
        let answer = answer(Some(&host), run(&["/no/such/program"])).await;
        let Some(GuestToHost::LeaseRan {
            result: Err(err), ..
        }) = answer
        else {
            panic!("{answer:?}");
        };
        assert!(err.starts_with("couldn't run /no/such/program"), "{err}");
        let answer = super::answer(Some(&host), run(&[])).await;
        assert!(matches!(
            answer,
            Some(GuestToHost::LeaseRan { result: Err(_), .. })
        ));
    }

    #[tokio::test]
    async fn a_linux_lease_has_no_screen() {
        let dir = TempDir::new();
        let host: Arc<dyn TestHost> = Arc::new(Linux::here(dir.path().to_owned()));
        let lease = LeaseId::derive("lease");
        let answer = answer(Some(&host), HostToGuest::LeaseScreenshot { req: 2, lease }).await;
        assert_eq!(
            answer,
            Some(GuestToHost::LeaseScreen {
                req: 2,
                result: Err("A Linux lease has no screen.".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn a_linux_install_moves_the_upload_home() {
        let dir = TempDir::new();
        let (upload, home) = (dir.path().join("upload"), dir.path().join("home"));
        std::fs::create_dir_all(&upload).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(upload.join("tool.bin"), "bytes").unwrap();
        let host: Arc<dyn TestHost> = Arc::new(Linux::here(home.clone()));
        let install = HostToGuest::LeaseInstall {
            req: 3,
            lease: LeaseId::derive("lease"),
            path: upload.join("tool.bin").display().to_string(),
        };
        assert_eq!(
            answer(Some(&host), install).await,
            Some(GuestToHost::LeaseDone {
                req: 3,
                result: Ok(())
            })
        );
        assert_eq!(
            std::fs::read_to_string(home.join("tool.bin")).unwrap(),
            "bytes"
        );
    }

    #[test]
    fn a_screenshot_goes_as_base64_unless_its_too_large() {
        assert_eq!(
            encoded(b"PNG"),
            Ok(ToolImage {
                mime: "image/png".to_owned(),
                data: "UE5H".to_owned(),
            })
        );
        let large = vec![0u8; MAX_MESSAGE_LEN];
        assert_eq!(
            encoded(&large),
            Err("The screenshot is too large to send.".to_owned())
        );
    }

    #[test]
    fn a_full_size_screen_shrinks_before_it_goes() {
        use image::{GenericImageView, ImageFormat, Rgb, RgbImage};

        let mut png = Vec::new();
        RgbImage::from_fn(2560, 1600, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 9])
        })
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
        let sent = image(&png).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(sent.data)
            .unwrap();
        let shown = image::load_from_memory_with_format(&bytes, ImageFormat::Png).unwrap();
        assert_eq!(shown.dimensions(), (MAX_SCREEN_EDGE, 800));
        assert!(image(b"not a picture")
            .unwrap_err()
            .starts_with("The screenshot couldn't be read"));
    }
}
