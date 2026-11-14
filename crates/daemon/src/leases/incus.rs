// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `linux` test host: one Incus container, [`NAME`], made like a
//! workspace from the `linux` recipe and kept at its `clean` snapshot. Its
//! guest runs `maxd guest --test-host`, so each lease's calls go over the
//! guest link, and each lease's end takes it back to the snapshot.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use maxhq_protocol::guest::GuestToHost;
use maxhq_protocol::ids::LeaseId;
use maxhq_protocol::leases::{RunOutput, TestHostBackend, TestKind};
use tokio::io::AsyncReadExt;

use super::backend::{Built, LeaseTarget, TestBackend};
use super::detect;
use super::via_guest::{self, Guest};
use crate::incus::{Incus, InstanceName};
use crate::transfer::{self, MakeRequest};
use crate::workspace::{self, Manager};

/// The container's name.
pub const NAME: &str = "mhq-test-linux";

/// What the container's test host key says it is.
const KIND: &str = "linux";

/// Where each upload gets a folder of its own, which the guest's install
/// empties into the test user's home.
const UPLOADS: &str = "/var/tmp";

/// How much of a file goes in one push.
const CHUNK: usize = 4 * 1024 * 1024;

/// What the backend asks of the container. [`Wired`] does it through the
/// workspace manager and the guest links.
trait Container: Guest + 'static {
    /// Makes it from nothing, stopped at its snapshot.
    fn build(&self) -> BoxFuture<'_, Result<(), String>>;
    /// Starts it and waits for its guest.
    fn start(&self) -> BoxFuture<'_, Result<(), String>>;
    /// Stops it and takes it back to its snapshot.
    fn clean(&self) -> BoxFuture<'_, Result<(), String>>;
    /// Puts the file or folder at `source` into the new folder `folder`
    /// and answers where it landed.
    fn upload<'a>(
        &'a self,
        source: &'a Path,
        folder: &'a str,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// The `linux` test host on this device's Incus.
pub struct IncusTest {
    container: Box<dyn Container>,
    /// Numbers each upload's folder.
    uploads: AtomicU64,
}

impl IncusTest {
    /// The host whose guest calls go through `guests`.
    pub fn wired(guests: Arc<dyn transfer::Guests>) -> Self {
        let name = InstanceName::new(NAME).expect("the test host's name is valid");
        Self::with(Box::new(Wired { guests, name }))
    }

    fn with(container: Box<dyn Container>) -> Self {
        Self {
            container,
            uploads: AtomicU64::new(0),
        }
    }
}

impl TestBackend for IncusTest {
    fn kind(&self) -> TestKind {
        TestKind::Linux
    }

    fn backend(&self) -> TestHostBackend {
        TestHostBackend::IncusTest
    }

    fn detect(&self) -> BoxFuture<'_, Option<String>> {
        async move {
            detect::ready(TestKind::Linux, TestHostBackend::IncusTest)
                .await
                .err()
        }
        .boxed()
    }

    fn build(&self) -> BoxFuture<'_, Result<Built, String>> {
        async move {
            self.container.build().await?;
            Ok(Built::default())
        }
        .boxed()
    }

    fn start(&self, _lease: LeaseId) -> BoxFuture<'_, Result<String, String>> {
        async move {
            self.container.start().await?;
            Ok(NAME.to_owned())
        }
        .boxed()
    }

    fn run<'a>(
        &'a self,
        on: &'a LeaseTarget,
        command: &'a [String],
        timeout: Duration,
    ) -> BoxFuture<'a, Result<RunOutput, String>> {
        via_guest::run(&*self.container, on.lease, command, timeout).boxed()
    }

    fn screenshot<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        via_guest::screenshot(&*self.container, on.lease).boxed()
    }

    fn install<'a>(
        &'a self,
        on: &'a LeaseTarget,
        source: PathBuf,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move {
            let lease = on.lease;
            let n = self.uploads.fetch_add(1, Ordering::Relaxed);
            let folder = format!("{UPLOADS}/maxhq-{lease}-{n}");
            let path = self.container.upload(&source, &folder).await?;
            via_guest::install(&*self.container, lease, path).await?;
            let name = source.file_name().unwrap_or_default().to_string_lossy();
            Ok(format!("Copied {name} into the test user's home folder."))
        }
        .boxed()
    }

    fn close<'a>(&'a self, on: &'a LeaseTarget) -> BoxFuture<'a, Result<(), String>> {
        async move {
            let lease = on.lease;
            // The restore undoes the lease whatever the guest said.
            let closed = via_guest::close(&*self.container, lease).await;
            self.container.clean().await?;
            if let Err(error) = closed {
                tracing::warn!(%lease, %error, "the test host's guest didn't close the lease");
            }
            Ok(())
        }
        .boxed()
    }

    fn reset(&self) -> BoxFuture<'_, Result<(), String>> {
        self.container.clean()
    }
}

/// The container through the workspace manager and the guest links.
struct Wired {
    guests: Arc<dyn transfer::Guests>,
    name: InstanceName,
}

impl Wired {
    fn manager(&self) -> Result<&'static Manager, String> {
        workspace::manager().ok_or_else(|| "Container workspaces aren't running yet.".to_owned())
    }
}

impl Container for Wired {
    fn build(&self) -> BoxFuture<'_, Result<(), String>> {
        async move {
            let steps = crate::recipe::linux_steps();
            self.manager()?
                .build_test_host(&self.name, KIND, &steps)
                .await
        }
        .boxed()
    }

    fn start(&self) -> BoxFuture<'_, Result<(), String>> {
        async move { self.manager()?.start_test_host(&self.name).await }.boxed()
    }

    fn clean(&self) -> BoxFuture<'_, Result<(), String>> {
        async move { self.manager()?.clean_test_host(&self.name).await }.boxed()
    }

    fn upload<'a>(
        &'a self,
        source: &'a Path,
        folder: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        async move { upload(self.manager()?.incus(), &self.name, source, folder).await }.boxed()
    }
}

impl Guest for Wired {
    fn ask(&self, wait: Duration, make: MakeRequest) -> BoxFuture<'_, Result<GuestToHost, String>> {
        async move {
            let link = self.manager()?.test_link(&self.name);
            let out = self
                .guests
                .link(link)
                .ok_or_else(|| "the test host's guest isn't linked".to_owned())?;
            out.request(wait, make).await
        }
        .boxed()
    }
}

/// Copies the file or folder at `source` into a new folder, `folder`, in
/// the container `name`, keeping each file's mode, and answers the path
/// it landed at. The folder holds only the upload, since the guest's
/// install moves all it holds.
async fn upload(
    incus: &Incus,
    name: &InstanceName,
    source: &Path,
    folder: &str,
) -> Result<String, String> {
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no name to install it by.", source.display()))?;
    let failed =
        |error: crate::incus::IncusError| format!("Incus couldn't take the upload: {error}");
    incus
        .make_folder(name, folder, 0o700)
        .await
        .map_err(failed)?;
    let top = format!("{folder}/{file_name}");
    let mut stack = vec![(source.to_path_buf(), top.clone())];
    while let Some((from, to)) = stack.pop() {
        let meta = tokio::fs::symlink_metadata(&from)
            .await
            .map_err(|error| format!("couldn't read {}: {error}", from.display()))?;
        let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
        if meta.is_dir() {
            incus.make_folder(name, &to, mode).await.map_err(failed)?;
            let mut entries = tokio::fs::read_dir(&from)
                .await
                .map_err(|error| format!("couldn't read {}: {error}", from.display()))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| format!("couldn't read {}: {error}", from.display()))?
            {
                let Some(entry_name) = entry.file_name().to_str().map(str::to_owned) else {
                    return Err(format!("{} has a name that isn't UTF-8.", from.display()));
                };
                stack.push((entry.path(), format!("{to}/{entry_name}")));
            }
        } else if meta.is_file() {
            let mut file = tokio::fs::File::open(&from)
                .await
                .map_err(|error| format!("couldn't read {}: {error}", from.display()))?;
            let mut first = true;
            loop {
                let mut chunk = Vec::with_capacity(CHUNK.min(meta.len() as usize));
                (&mut file)
                    .take(CHUNK as u64)
                    .read_to_end(&mut chunk)
                    .await
                    .map_err(|error| format!("couldn't read {}: {error}", from.display()))?;
                if chunk.is_empty() && !first {
                    break;
                }
                let more = chunk.len() == CHUNK;
                if first {
                    incus.push_file(name, &to, chunk, mode).await
                } else {
                    incus.append_file(name, &to, chunk, mode).await
                }
                .map_err(failed)?;
                first = false;
                if !more {
                    break;
                }
            }
        } else {
            return Err(format!(
                "{} is a link or a special file, which an install doesn't take.",
                from.display()
            ));
        }
    }
    Ok(top)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use maxhq_protocol::agents::ToolImage;
    use serde_json::Value;

    use super::*;
    use crate::incus::fake::{FakeIncus, State};
    use crate::incus::MANAGED_KEY;
    use crate::leases::via_guest::WRONG;
    use crate::testing::TempDir;
    use base64::Engine as _;

    /// A container that answers from a list and logs what it was asked.
    #[derive(Default)]
    struct FakeContainer {
        log: Mutex<Vec<String>>,
        answers: Mutex<Vec<Result<GuestToHost, String>>>,
    }

    impl FakeContainer {
        fn note(&self, call: String) {
            self.log.lock().unwrap().push(call);
        }

        fn answer(&self, answer: Result<GuestToHost, String>) {
            self.answers.lock().unwrap().push(answer);
        }
    }

    impl Container for Arc<FakeContainer> {
        fn build(&self) -> BoxFuture<'_, Result<(), String>> {
            self.note("build".into());
            async { Ok(()) }.boxed()
        }

        fn start(&self) -> BoxFuture<'_, Result<(), String>> {
            self.note("start".into());
            async { Ok(()) }.boxed()
        }

        fn clean(&self) -> BoxFuture<'_, Result<(), String>> {
            self.note("clean".into());
            async { Ok(()) }.boxed()
        }

        fn upload<'a>(
            &'a self,
            source: &'a Path,
            folder: &'a str,
        ) -> BoxFuture<'a, Result<String, String>> {
            self.note(format!("upload {} {folder}", source.display()));
            let name = source.file_name().unwrap().to_string_lossy();
            let path = format!("{folder}/{name}");
            async move { Ok(path) }.boxed()
        }
    }

    impl Guest for Arc<FakeContainer> {
        fn ask(
            &self,
            wait: Duration,
            make: MakeRequest,
        ) -> BoxFuture<'_, Result<GuestToHost, String>> {
            let message = serde_json::to_value(make(7)).unwrap();
            self.note(format!("ask {}s {message}", wait.as_secs()));
            let answer = self.answers.lock().unwrap().remove(0);
            async move { answer }.boxed()
        }
    }

    fn host() -> (IncusTest, Arc<FakeContainer>) {
        let fake = Arc::new(FakeContainer::default());
        let host = IncusTest::with(Box::new(fake.clone()));
        (host, fake)
    }

    fn target() -> LeaseTarget {
        LeaseTarget {
            lease: LeaseId::derive("lease"),
            target: NAME.to_owned(),
        }
    }

    fn log(fake: &FakeContainer) -> Vec<String> {
        fake.log.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_lease_starts_the_container_and_ends_at_its_snapshot() {
        let (host, fake) = host();
        assert_eq!(host.build().await.unwrap(), Built::default());
        assert_eq!(host.start(target().lease).await.unwrap(), NAME);
        fake.answer(Ok(GuestToHost::LeaseDone {
            req: 7,
            result: Err("gone".into()),
        }));
        // The guest's refusal doesn't stop the restore.
        host.close(&target()).await.unwrap();
        host.reset().await.unwrap();
        let log = log(&fake);
        assert_eq!(log[..2], ["build", "start"]);
        assert!(log[2].contains("\"lease_close\""), "{}", log[2]);
        assert_eq!(log[3..], ["clean", "clean"]);
    }

    #[tokio::test]
    async fn a_run_goes_to_the_guest_with_its_timeout() {
        let (host, fake) = host();
        let output = RunOutput {
            exit_code: Some(0),
            stdout: "Linux\n".into(),
            stderr: String::new(),
            truncated: false,
            timed_out: false,
        };
        fake.answer(Ok(GuestToHost::LeaseRan {
            req: 7,
            result: Ok(output.clone()),
        }));
        let command = ["uname".to_owned(), "-s".to_owned()];
        let ran = host
            .run(&target(), &command, Duration::from_secs(120))
            .await
            .unwrap();
        assert_eq!(ran, output);
        let asked = &log(&fake)[0];
        assert!(asked.starts_with("ask 150s "), "{asked}");
        let message: Value = serde_json::from_str(asked.splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(message["command"], serde_json::json!(["uname", "-s"]));
        assert_eq!(message["timeout_seconds"], 120);

        // An answer of the wrong kind is an error.
        fake.answer(Ok(GuestToHost::LeaseDone {
            req: 7,
            result: Ok(()),
        }));
        let wrong = host.run(&target(), &command, Duration::from_secs(1)).await;
        assert_eq!(wrong.unwrap_err(), WRONG);
    }

    #[tokio::test]
    async fn a_screenshot_comes_back_as_its_bytes() {
        let (host, fake) = host();
        fake.answer(Ok(GuestToHost::LeaseScreen {
            req: 7,
            result: Ok(ToolImage {
                mime: "image/png".into(),
                data: base64::engine::general_purpose::STANDARD.encode(b"PNG"),
            }),
        }));
        assert_eq!(host.screenshot(&target()).await.unwrap(), b"PNG");
        fake.answer(Ok(GuestToHost::LeaseScreen {
            req: 7,
            result: Err("A Linux lease has no screen.".into()),
        }));
        let none = host.screenshot(&target()).await.unwrap_err();
        assert_eq!(none, "A Linux lease has no screen.");
    }

    #[tokio::test]
    async fn each_install_gets_a_folder_of_its_own() {
        let (host, fake) = host();
        for _ in 0..2 {
            fake.answer(Ok(GuestToHost::LeaseDone {
                req: 7,
                result: Ok(()),
            }));
        }
        let said = host
            .install(&target(), PathBuf::from("/up/app.tar"))
            .await
            .unwrap();
        assert_eq!(said, "Copied app.tar into the test user's home folder.");
        host.install(&target(), PathBuf::from("/up/app.tar"))
            .await
            .unwrap();
        let lease = target().lease;
        let log = log(&fake);
        assert_eq!(
            log[0],
            format!("upload /up/app.tar /var/tmp/maxhq-{lease}-0")
        );
        assert!(log[1].contains(&format!("\"path\":\"/var/tmp/maxhq-{lease}-0/app.tar\"")));
        assert_eq!(
            log[2],
            format!("upload /up/app.tar /var/tmp/maxhq-{lease}-1")
        );

        // The guest's refusal is the install's.
        fake.answer(Ok(GuestToHost::LeaseDone {
            req: 7,
            result: Err("no room".into()),
        }));
        let refused = host.install(&target(), PathBuf::from("/up/app.tar")).await;
        assert_eq!(refused.unwrap_err(), "no room");
    }

    #[tokio::test]
    async fn an_upload_copies_files_and_folders_with_their_modes() {
        let dir = TempDir::new();
        let app = dir.path().join("app");
        std::fs::create_dir_all(app.join("bin")).unwrap();
        let big = vec![7u8; CHUNK + 5];
        std::fs::write(app.join("bin/run"), &big).unwrap();
        std::fs::write(app.join("empty"), b"").unwrap();
        let run = app.join("bin/run");
        let mut perms = std::fs::metadata(&run).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&run, perms).unwrap();

        let mut state = State::default();
        state.add(NAME, "Running", &[(MANAGED_KEY, "true")]);
        let fake = FakeIncus::start(state);
        let incus = Incus::new(&fake.socket);
        let name = InstanceName::new(NAME).unwrap();

        let landed = upload(&incus, &name, &app, "/var/tmp/maxhq-1-0")
            .await
            .unwrap();
        assert_eq!(landed, "/var/tmp/maxhq-1-0/app");
        {
            let state = fake.state();
            let file = |path: &str| &state.files[&(NAME.to_owned(), path.to_owned())];
            assert_eq!(file("/var/tmp/maxhq-1-0").kind, "directory");
            assert_eq!(file("/var/tmp/maxhq-1-0").mode, "0700");
            assert_eq!(file("/var/tmp/maxhq-1-0/app/bin").kind, "directory");
            assert_eq!(file("/var/tmp/maxhq-1-0/app/bin/run").bytes, big);
            assert_eq!(file("/var/tmp/maxhq-1-0/app/bin/run").mode, "0755");
            assert_eq!(file("/var/tmp/maxhq-1-0/app/empty").bytes, b"");
        }

        // A link isn't taken.
        std::os::unix::fs::symlink("/etc/passwd", app.join("link")).unwrap();
        let refused = upload(&incus, &name, &app, "/var/tmp/maxhq-1-1").await;
        assert!(refused.unwrap_err().contains("is a link"));
    }
}
