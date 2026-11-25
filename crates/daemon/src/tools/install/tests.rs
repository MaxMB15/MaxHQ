// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The installer's tests. Each runs the real scripts with `sh` in a
//! folder of its own, and every download comes from a local HTTP fixture.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use futures::future::FutureExt;
use maxhq_protocol::state::{HelperTool, ToolPhase};
use tokio::io::AsyncWriteExt;

use super::fixture::{Answer, Fixture};
use super::releases::Feed;
use super::*;
use crate::tools::install::fetch::hex;

/// Runs commands on this machine. Who runs them doesn't matter here.
struct Local;

impl Exec for Local {
    fn run<'a>(
        &'a self,
        cmd: &'a [&'a str],
        _user: ExecUser,
        stdin: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<ExecOutput, ExecError>> {
        async move {
            let mut child = tokio::process::Command::new(cmd[0])
                .args(&cmd[1..])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|err| ExecError::Transport(err.to_string()))?;
            let mut input = child.stdin.take().unwrap();
            if let Some(bytes) = stdin {
                input
                    .write_all(bytes)
                    .await
                    .map_err(|err| ExecError::Transport(err.to_string()))?;
            }
            drop(input);
            let output = child
                .wait_with_output()
                .await
                .map_err(|err| ExecError::Transport(err.to_string()))?;
            Ok(ExecOutput {
                status: output.status.code().unwrap_or(128),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
        .boxed()
    }

    fn read<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, ExecError>> {
        async move {
            match tokio::fs::read(path).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(ExecError::Transport(err.to_string())),
            }
        }
        .boxed()
    }

    fn write<'a>(
        &'a self,
        path: &'a str,
        bytes: &'a [u8],
        mode: u32,
        _owner: ExecUser,
    ) -> BoxFuture<'a, Result<(), ExecError>> {
        async move {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::write(path, bytes)
                .await
                .map_err(|err| ExecError::Transport(err.to_string()))?;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .await
                .map_err(|err| ExecError::Transport(err.to_string()))
        }
        .boxed()
    }
}

/// A folder of the test's own, removed when it ends.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Temp {
        let path = std::env::temp_dir().join(format!("maxhq-install-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&path).unwrap();
        Temp(path.canonicalize().unwrap())
    }

    fn path(&self, rel: &str) -> String {
        format!("{}/{rel}", self.0.display())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TARGET: &str = "x86_64-unknown-linux-musl";

/// A device's layout under `temp`.
fn host(temp: &Temp) -> Layout {
    Layout {
        label: temp.path("host"),
        root: temp.path("home/.local"),
        user: ExecUser::Agent,
        target: TARGET.to_owned(),
        env: Vec::new(),
        replace_links: false,
    }
}

/// A container's layout under `temp`: commands run through `env` with a
/// home of their own, as they do in a workspace.
fn container(temp: &Temp) -> Layout {
    Layout {
        label: temp.path("container"),
        root: temp.path("usr/local"),
        user: ExecUser::Root,
        target: TARGET.to_owned(),
        env: vec!["env".to_owned(), format!("HOME={}", temp.path("root"))],
        replace_links: false,
    }
}

/// A tar.gz holding `files`, as paths and what they hold, made with tar.
fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let temp = Temp::new();
    for (path, text) in files {
        let full = temp.0.join("in").join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = temp.0.join("out.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(temp.0.join("in"))
        .args(
            files
                .iter()
                .map(|(path, _)| path.split('/').next().unwrap()),
        )
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(out).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// A program that prints `tool <version>`.
fn program(version: &str) -> String {
    format!("#!/bin/sh\necho \"tool {version}\"\n")
}

/// The test tool's kind: an ACP id, so its folder is `acp-tool`.
fn kind() -> ToolKind {
    ToolKind::Acp { id: "tool".into() }
}

/// A tool from one archive at `url`.
fn archive_def(url: &str, sha256: Option<String>, size: Option<u64>) -> InstallDef {
    InstallDef {
        kind: kind(),
        binary: Some("tool".into()),
        source: Source::Archive {
            files: vec![ArchiveFile {
                target: TARGET.to_owned(),
                url: url.to_owned(),
                sha256,
                size,
            }],
            confirmed_unchecked: false,
        },
    }
}

/// Serves a release of the tool, with a helper next to it, and gives its
/// definition.
fn release(fixture: &Fixture, version: &str) -> InstallDef {
    let bytes = tarball(&[
        (&format!("tool-{TARGET}"), &program(version)),
        ("tool-helper", "#!/bin/sh\necho helper\n"),
        ("README", "not a program"),
    ]);
    let url = fixture.serve(
        &format!("/tool-{version}.tar.gz"),
        Answer::Body(bytes.clone()),
    );
    archive_def(&url, Some(sha(&bytes)), Some(bytes.len() as u64))
}

/// No releases to look up: every definition here names its files.
struct NoReleases;

impl Releases for NoReleases {
    fn archive<'a>(
        &'a self,
        _feed: &'a Feed,
        _version: Option<&'a str>,
        _target: &'a str,
    ) -> BoxFuture<'a, Result<releases::Release, InstallError>> {
        Box::pin(async { Err(InstallError::Failed("no releases here".into())) })
    }

    fn npm_latest<'a>(&'a self, _package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async { Err(InstallError::Failed("no releases here".into())) })
    }

    fn pypi_latest<'a>(&'a self, _package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async { Err(InstallError::Failed("no releases here".into())) })
    }
}

/// Releases that all carry one version, and name no files.
struct Published(&'static str);

impl Releases for Published {
    fn archive<'a>(
        &'a self,
        _feed: &'a Feed,
        _version: Option<&'a str>,
        _target: &'a str,
    ) -> BoxFuture<'a, Result<releases::Release, InstallError>> {
        let version = self.0.to_owned();
        Box::pin(async move {
            Ok(releases::Release {
                version,
                files: Vec::new(),
            })
        })
    }

    fn npm_latest<'a>(&'a self, _package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async { Ok(self.0.to_owned()) })
    }

    fn pypi_latest<'a>(&'a self, _package: &'a str) -> BoxFuture<'a, Result<String, InstallError>> {
        Box::pin(async { Ok(self.0.to_owned()) })
    }
}

async fn install(
    layout: &Layout,
    def: &InstallDef,
    version: &str,
) -> Result<Installed, InstallError> {
    let spot = Spot {
        exec: &Local,
        layout: layout.clone(),
    };
    let quiet = |_: ToolState| {};
    Ok(flow::install(&spot, def, Some(version), &NoReleases, &quiet).await?)
}

async fn remove(layout: &Layout, kind: &ToolKind) -> Result<(), InstallError> {
    let spot = Spot {
        exec: &Local,
        layout: layout.clone(),
    };
    Ok(flow::remove(&spot, kind).await?)
}

fn run(path: &str) -> String {
    let output = std::process::Command::new(path).output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// What's in the tool's folder, by name.
fn names(layout: &Layout) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(layout.dir(&kind()))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The version `current` names, which must be whole.
fn current(layout: &Layout) -> Option<String> {
    let dir = layout.dir(&kind());
    let version = std::fs::read_link(format!("{dir}/current")).ok()?;
    let version = version.to_string_lossy().into_owned();
    assert!(
        Path::new(&format!("{dir}/{version}/.install-complete")).is_file(),
        "current names {version}, which isn't whole"
    );
    Some(version)
}

#[tokio::test]
async fn an_archive_installs_publishes_and_links() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let def = release(&fixture, "1.2.3");
    let states = Mutex::new(Vec::new());
    let report = |state: ToolState| states.lock().unwrap().push(state);
    let spot = Spot {
        exec: &Local,
        layout: layout.clone(),
    };
    let installed = flow::install(&spot, &def, Some("1.2.3"), &NoReleases, &report)
        .await
        .unwrap();

    let bin = layout.bin();
    assert_eq!(installed.path, format!("{bin}/tool"));
    assert_eq!(installed.version, "1.2.3");
    assert_eq!(run(&installed.path), "tool 1.2.3");
    assert_eq!(run(&format!("{bin}/tool-helper")), "helper");
    assert!(!Path::new(&format!("{bin}/README")).exists());
    assert_eq!(current(&layout).as_deref(), Some("1.2.3"));
    assert_eq!(names(&layout), ["1.2.3", "current", "links"]);
    let record = std::fs::read_to_string(format!("{}/.install-complete", installed.root)).unwrap();
    assert!(record.contains("entry tool\n"), "{record}");
    assert!(record.contains("sha256 tool-1.2.3.tar.gz "), "{record}");

    let phases: Vec<ToolPhase> = states
        .lock()
        .unwrap()
        .iter()
        .filter_map(|state| match state {
            ToolState::Running { phase, .. } => *phase,
            _ => None,
        })
        .collect();
    for phase in [
        ToolPhase::Resolving,
        ToolPhase::Downloading,
        ToolPhase::Verifying,
        ToolPhase::Installing,
        ToolPhase::Linking,
    ] {
        assert!(phases.contains(&phase), "{phase:?} in {phases:?}");
    }
    assert!(states.lock().unwrap().iter().any(|state| matches!(
        state,
        ToolState::Running {
            bytes_done: Some(done),
            bytes_total: Some(total),
            ..
        } if done == total
    )));

    // A version that's whole is reused without a download.
    let hits = fixture.hits.load(Ordering::SeqCst);
    install(&layout, &def, "1.2.3").await.unwrap();
    assert_eq!(fixture.hits.load(Ordering::SeqCst), hits);
}

#[tokio::test]
async fn current_always_names_a_whole_version_and_old_ones_go() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let def = release(&fixture, version);
        let installed = install(&layout, &def, version).await.unwrap();
        assert_eq!(run(&installed.path), format!("tool {version}"));
        assert_eq!(current(&layout).as_deref(), Some(version));
    }
    // The one before stays, for a quick way back.
    assert_eq!(names(&layout), ["2.0.0", "3.0.0", "current", "links"]);
}

#[tokio::test]
async fn a_bad_hash_leaves_nothing_published() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let good = release(&fixture, "1.0.0");
    let Source::Archive { files, .. } = &good.source else {
        unreachable!()
    };
    let bad = archive_def(&files[0].url, Some("0".repeat(64)), files[0].size);
    let err = install(&layout, &bad, "1.0.0").await.unwrap_err();
    assert!(matches!(err, InstallError::Checksum { .. }), "{err:?}");
    assert_eq!(current(&layout), None);
    assert!(!Path::new(&format!("{}/tool", layout.bin())).exists());
    assert!(names(&layout).is_empty(), "{:?}", names(&layout));

    install(&layout, &good, "1.0.0").await.unwrap();
    assert_eq!(current(&layout).as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn a_short_download_leaves_nothing_published() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let bytes = tarball(&[(&format!("tool-{TARGET}"), &program("1.0.0"))]);
    let url = fixture.serve("/short.tar.gz", Answer::Short(bytes.clone()));
    let def = archive_def(&url, Some(sha(&bytes)), None);
    let err = install(&layout, &def, "1.0.0").await.unwrap_err();
    assert!(matches!(err, InstallError::Failed(_)), "{err:?}");
    assert_eq!(current(&layout), None);
    assert!(names(&layout).is_empty(), "{:?}", names(&layout));

    // The source says how big it is, and a smaller one fails too.
    let url = fixture.serve("/sized.tar.gz", Answer::Body(bytes.clone()));
    let def = archive_def(&url, Some(sha(&bytes)), Some(bytes.len() as u64 + 1));
    let err = install(&layout, &def, "1.0.0").await.unwrap_err();
    assert!(err.to_string().contains("bytes"), "{err}");
    assert_eq!(current(&layout), None);
}

#[tokio::test]
async fn a_killed_install_leaves_nothing_and_the_next_one_works() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let bytes = tarball(&[(&format!("tool-{TARGET}"), &program("1.0.0"))]);
    let url = fixture.serve("/stall.tar.gz", Answer::Stall(bytes.clone()));
    let def = archive_def(&url, Some(sha(&bytes)), None);
    let task = tokio::spawn({
        let layout = layout.clone();
        async move { install(&layout, &def, "1.0.0").await }
    });
    // Wait until the download has started writing into staging.
    let dir = layout.dir(&kind());
    for _ in 0..200 {
        let started = names(&layout).iter().any(|name| {
            name.starts_with(".staging-")
                && Path::new(&format!("{dir}/{name}/dl/stall.tar.gz")).exists()
        });
        if started {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    task.abort();
    let _ = task.await;
    assert_eq!(current(&layout), None);
    assert!(names(&layout).contains(&".lock".to_owned()));

    let good = release(&fixture, "1.0.0");
    install(&layout, &good, "1.0.0").await.unwrap();
    assert_eq!(current(&layout).as_deref(), Some("1.0.0"));
    assert_eq!(names(&layout), ["1.0.0", "current", "links"]);
}

#[tokio::test]
async fn two_installs_at_once_take_turns() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let one = release(&fixture, "1.0.0");
    let two = release(&fixture, "2.0.0");
    let (a, b) = tokio::join!(
        install(&layout, &one, "1.0.0"),
        install(&layout, &two, "2.0.0")
    );
    a.unwrap();
    b.unwrap();
    let version = current(&layout).unwrap();
    assert!(version == "1.0.0" || version == "2.0.0");
    assert_eq!(
        run(&format!("{}/tool", layout.bin())),
        format!("tool {version}")
    );
    assert!(!names(&layout).iter().any(|name| name.starts_with('.')));
}

#[tokio::test]
async fn a_container_and_a_device_run_the_same_code() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let def = release(&fixture, "1.0.0");
    for layout in [host(&temp), container(&temp)] {
        let installed = install(&layout, &def, "1.0.0").await.unwrap();
        assert!(
            installed.path.starts_with(&layout.root),
            "{}",
            installed.path
        );
        assert_eq!(run(&installed.path), "tool 1.0.0");
    }
}

#[tokio::test]
async fn a_tool_you_installed_is_never_touched() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    std::fs::create_dir_all(layout.bin()).unwrap();
    let yours = format!("{}/tool", layout.bin());
    std::fs::write(&yours, "yours").unwrap();
    let def = release(&fixture, "1.0.0");
    let installed = install(&layout, &def, "1.0.0").await.unwrap();
    assert_eq!(std::fs::read_to_string(&yours).unwrap(), "yours");
    assert_eq!(
        installed.path,
        format!("{}/current/tool", layout.dir(&kind()))
    );
    assert_eq!(run(&installed.path), "tool 1.0.0");

    // Removing takes only what MaxHQ made.
    remove(&layout, &kind()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&yours).unwrap(), "yours");
    assert!(!Path::new(&format!("{}/tool-helper", layout.bin())).exists());
    assert!(!Path::new(&layout.dir(&kind())).exists());
}

#[tokio::test]
async fn remove_takes_only_its_own_links() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let def = release(&fixture, "1.0.0");
    install(&layout, &def, "1.0.0").await.unwrap();
    let other = format!("{}/other", layout.bin());
    std::fs::write(&other, "yours").unwrap();
    // A link someone pointed elsewhere since stays.
    let helper = format!("{}/tool-helper", layout.bin());
    std::fs::remove_file(&helper).unwrap();
    std::os::unix::fs::symlink(&other, &helper).unwrap();

    remove(&layout, &kind()).await.unwrap();
    assert!(!Path::new(&format!("{}/tool", layout.bin())).exists());
    assert!(std::fs::symlink_metadata(&helper).is_ok());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "yours");
    assert!(!Path::new(&layout.dir(&kind())).exists());

    let err = remove(&layout, &kind()).await.unwrap_err();
    assert!(err.to_string().contains("didn't install"), "{err}");
    assert!(!Path::new(&layout.dir(&kind())).exists());
}

#[tokio::test]
async fn a_published_version_that_isnt_one_is_refused() {
    let temp = Temp::new();
    let layout = host(&temp);
    let mut def = archive_def("http://127.0.0.1:9/never", None, None);
    def.kind = ToolKind::Helper {
        tool: maxhq_protocol::state::HelperTool::Gh,
    };
    def.source = Source::Archive {
        files: Vec::new(),
        confirmed_unchecked: false,
    };
    let spot = Spot {
        exec: &Local,
        layout: layout.clone(),
    };
    let quiet = |_: ToolState| {};
    for bad in [
        "../../../../x",
        "$(touch nope)",
        "https://example.invalid/x",
    ] {
        let released = Published(bad);
        let result = flow::install(&spot, &def, None, &released, &quiet).await;
        assert!(
            matches!(result, Err(Error::Install(InstallError::Failed(_)))),
            "{bad}"
        );
    }
    // A real one goes on as far as the missing files.
    for good in ["2.40.1", "v2.40.1", "0.116.0-beta.1"] {
        let released = Published(good);
        let result = flow::install(&spot, &def, None, &released, &quiet).await;
        assert!(
            matches!(result, Err(Error::Install(InstallError::NoTarget(_)))),
            "{good}"
        );
    }
}

#[tokio::test]
async fn archives_without_a_checksum_or_a_build_are_refused() {
    let temp = Temp::new();
    let layout = host(&temp);
    let def = archive_def("http://127.0.0.1:9/never", None, None);
    assert_eq!(
        install(&layout, &def, "1.0.0").await.unwrap_err(),
        InstallError::Unchecked
    );
    let mut other = archive_def("http://127.0.0.1:9/never", Some("0".repeat(64)), None);
    if let Source::Archive { files, .. } = &mut other.source {
        files[0].target = "aarch64-apple-darwin".into();
    }
    assert_eq!(
        install(&layout, &other, "1.0.0").await.unwrap_err(),
        InstallError::NoTarget(TARGET.into())
    );
}

#[tokio::test]
async fn a_script_installs_once_and_leaves_yours_alone() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let home = temp.path("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut layout = container(&temp);
    layout.root = format!("{home}/.local");
    layout.env = vec!["env".into(), format!("HOME={home}")];
    let script = "set -eu\nmkdir -p \"$HOME/.local/bin\"\nprintf '#!/bin/sh\\necho \"tool %s\"\\n' \"${1:-4.5.6}\" > \"$HOME/.local/bin/tool\"\nchmod 755 \"$HOME/.local/bin/tool\"\n";
    let url = fixture.serve("/install.sh", Answer::Body(script.as_bytes().to_vec()));
    let def = InstallDef {
        kind: kind(),
        binary: Some("tool".into()),
        source: Source::Script { url },
    };
    let spot = Spot {
        exec: &Local,
        layout: layout.clone(),
    };
    let quiet = |_: ToolState| {};
    let installed = flow::install(&spot, &def, None, &NoReleases, &quiet)
        .await
        .unwrap();
    assert_eq!(installed.version, "4.5.6");
    assert_eq!(installed.path, format!("{home}/.local/bin/tool"));
    let hits = fixture.hits.load(Ordering::SeqCst);
    flow::install(&spot, &def, None, &NoReleases, &quiet)
        .await
        .unwrap();
    assert_eq!(fixture.hits.load(Ordering::SeqCst), hits);
    // A version asked for runs the script again with it.
    let installed = flow::install(&spot, &def, Some("4.6.0"), &NoReleases, &quiet)
        .await
        .unwrap();
    assert_eq!(installed.version, "4.6.0");

    remove(&layout, &kind()).await.unwrap();
    assert!(!Path::new(&installed.path).exists());

    // One that's there and not MaxHQ's stays.
    std::fs::write(&installed.path, "yours").unwrap();
    let err = flow::install(&spot, &def, None, &NoReleases, &quiet)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("leaves yours alone"), "{err}");
    assert_eq!(std::fs::read_to_string(&installed.path).unwrap(), "yours");
}

/// Installs a fake helper from an archive, so npm and uv sources have
/// what they run.
async fn helper(fixture: &Fixture, layout: &Layout, tool: HelperTool, files: &[(&str, &str)]) {
    let bytes = tarball(files);
    let url = fixture.serve(&format!("/{tool:?}.tar.gz"), Answer::Body(bytes.clone()));
    let mut def = archive_def(&url, Some(sha(&bytes)), None);
    def.kind = ToolKind::Helper { tool };
    def.binary = None;
    install(layout, &def, "1.0.0").await.unwrap();
}

#[tokio::test]
async fn npm_and_uv_packages_get_wrappers() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    // npm here makes the package's program in the prefix it's given, and
    // one package's manifest names a command of its own.
    let npm = r#"#!/bin/sh
for last; do :; done
while [ "$1" != "--prefix" ]; do shift; done
mkdir -p "$2/node_modules/.bin"
printf '#!/bin/sh\necho "npm tool $*"\n' > "$2/node_modules/.bin/tool"
chmod 755 "$2/node_modules/.bin/tool"
if [ "${last%@*}" = "@scope/manifested" ]; then
  mkdir -p "$2/node_modules/@scope/manifested"
  printf '{"bin":{"runner":"cli.js"}}' > "$2/node_modules/@scope/manifested/package.json"
  printf '#!/bin/sh\necho "runner $*"\n' > "$2/node_modules/.bin/runner"
  chmod 755 "$2/node_modules/.bin/runner"
fi
"#;
    helper(
        &fixture,
        &layout,
        HelperTool::Node,
        &[("node/bin/node", "#!/bin/sh\n"), ("node/bin/npm", npm)],
    )
    .await;
    helper(
        &fixture,
        &layout,
        HelperTool::Uv,
        &[
            ("uv/uv", "#!/bin/sh\n"),
            ("uv/uvx", "#!/bin/sh\necho \"uvx $*\"\n"),
        ],
    )
    .await;

    let def = InstallDef {
        kind: kind(),
        binary: None,
        source: Source::Npm {
            package: "@scope/tool".into(),
        },
    };
    let installed = install(&layout, &def, "1.0.0").await.unwrap();
    assert_eq!(installed.path, format!("{}/tool", layout.bin()));
    let output = std::process::Command::new(&installed.path)
        .arg("hi")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "npm tool hi"
    );

    let def = InstallDef {
        kind: ToolKind::Acp { id: "py".into() },
        binary: Some("py-tool".into()),
        source: Source::Uv {
            package: "py-tool".into(),
        },
    };
    let installed = install(&layout, &def, "0.3.0").await.unwrap();
    let output = std::process::Command::new(&installed.path)
        .arg("hi")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "uvx --from py-tool==0.3.0 py-tool hi"
    );

    let def = InstallDef {
        kind: ToolKind::Acp {
            id: "manifested".into(),
        },
        binary: None,
        source: Source::Npm {
            package: "@scope/manifested".into(),
        },
    };
    let installed = install(&layout, &def, "2.0.0").await.unwrap();
    assert_eq!(installed.path, format!("{}/runner", layout.bin()));
    let output = std::process::Command::new(&installed.path)
        .arg("hi")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "runner hi");
}

#[test]
fn an_npm_manifest_names_the_command() {
    let pick = |manifest: &str, wanted| flow::npm_command(manifest.as_bytes(), "@s/agent", wanted);
    assert_eq!(pick(r#"{"bin":"cli.js"}"#, None).as_deref(), Some("agent"));
    assert_eq!(
        pick(r#"{"bin":{"one":"a.js"}}"#, None).as_deref(),
        Some("one")
    );
    let two = r#"{"bin":{"agent":"a.js","other":"b.js"}}"#;
    assert_eq!(pick(two, None).as_deref(), Some("agent"));
    assert_eq!(pick(two, Some("other")).as_deref(), Some("other"));
    let same = r#"{"bin":{"b":"x.js","a":"x.js"}}"#;
    assert_eq!(pick(same, None).as_deref(), Some("a"));
    assert_eq!(pick(r#"{"bin":{"b":"x.js","a":"y.js"}}"#, None), None);
    assert_eq!(pick("{}", None), None);
}

#[tokio::test]
async fn github_releases_give_their_checksums() {
    let fixture = Fixture::start().await;
    let file = fixture.serve(
        "/dl/uv-x86_64-unknown-linux-musl.tar.gz",
        Answer::Body(b"x".to_vec()),
    );
    let sums = fixture.serve(
        "/dl/checksums.txt",
        Answer::Body(format!("{}  gh_2.0.0_linux_amd64.tar.gz\n", "b".repeat(64)).into_bytes()),
    );
    let gh_file = fixture.serve(
        "/dl/gh_2.0.0_linux_amd64.tar.gz",
        Answer::Body(b"y".to_vec()),
    );
    let uv = serde_json::json!({
        "tag_name": "0.9.0",
        "assets": [{
            "name": "uv-x86_64-unknown-linux-musl.tar.gz",
            "browser_download_url": file,
            "size": 1,
            "digest": format!("sha256:{}", "a".repeat(64)),
        }],
    });
    let gh = serde_json::json!({
        "tag_name": "v2.0.0",
        "assets": [
            {"name": "gh_2.0.0_linux_amd64.tar.gz", "browser_download_url": gh_file, "size": 1},
            {"name": "gh_2.0.0_checksums.txt", "browser_download_url": sums, "size": 1},
        ],
    });
    fixture.serve(
        "/repos/astral-sh/uv/releases/latest",
        Answer::Body(uv.to_string().into_bytes()),
    );
    fixture.serve(
        "/repos/cli/cli/releases/tags/v2.0.0",
        Answer::Body(gh.to_string().into_bytes()),
    );
    let publishers = Publishers {
        github: fixture.base.clone(),
        ..Publishers::default()
    };
    let feed = |tool| crate::tools::defs::feed(&ToolKind::Helper { tool }).unwrap();

    let release = publishers
        .archive(&feed(HelperTool::Uv), None, TARGET)
        .await
        .unwrap();
    assert_eq!(release.version, "0.9.0");
    assert_eq!(release.files[0].sha256, Some("a".repeat(64)));

    let release = publishers
        .archive(&feed(HelperTool::Gh), Some("2.0.0"), TARGET)
        .await
        .unwrap();
    assert_eq!(release.files[0].sha256, Some("b".repeat(64)));
    assert_eq!(release.files[0].url, gh_file);

    let err = publishers
        .archive(&feed(HelperTool::Uv), None, "riscv64")
        .await
        .unwrap_err();
    assert_eq!(err, InstallError::NoTarget("riscv64".into()));
}

#[test]
fn targets_and_names_read_plainly() {
    assert_eq!(target_for("x86_64"), "x86_64-unknown-linux-musl");
    assert_eq!(target_for("arm64"), "aarch64-unknown-linux-musl");
    assert_eq!(target_for("aarch64-apple-darwin"), "aarch64-apple-darwin");
    assert_eq!(
        target_for("x86_64-unknown-linux-gnu"),
        "x86_64-unknown-linux-musl"
    );
    assert_eq!(target_for("riscv64"), "riscv64");
    assert_eq!(
        name(&ToolKind::Helper {
            tool: HelperTool::AndroidSdk
        }),
        "android-sdk"
    );
    assert_eq!(
        name(&ToolKind::Agent {
            tool: maxhq_protocol::model::Tool::Codex
        }),
        "codex"
    );
    assert_eq!(
        name(&ToolKind::Acp {
            id: "../My Agent".into()
        }),
        "acp--my-agent"
    );
}

#[tokio::test]
async fn the_unbuilt_installer_says_so() {
    let def = InstallDef {
        kind: ToolKind::Helper {
            tool: HelperTool::Gh,
        },
        binary: Some("gh".into()),
        source: Source::Archive {
            files: Vec::new(),
            confirmed_unchecked: false,
        },
    };
    let error = Unbuilt
        .ensure(&def, None, &ToolPlace::Host)
        .await
        .unwrap_err();
    assert_eq!(error, InstallError::NotBuilt);
    assert_eq!(error.to_string(), crate::hosts::NOT_YET);
    assert_eq!(
        Unbuilt.remove(&def.kind, &ToolPlace::Host).await,
        Err(InstallError::NotBuilt)
    );
}

#[tokio::test]
async fn a_mac_bundle_unpacks_from_its_home_without_links() {
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let bytes = tarball(&[
        ("jdk-1/Contents/Home/bin/java", &program("1")),
        ("jdk-1/Contents/Info.plist", "<plist/>"),
    ]);
    let url = fixture.serve("/jdk-1.tar.gz", Answer::Body(bytes.clone()));
    let mut def = archive_def(&url, Some(sha(&bytes)), Some(bytes.len() as u64));
    def.binary = None;
    let installed = install(&layout, &def, "1").await.unwrap();

    assert_eq!(
        installed.path,
        format!("{}/current/bin/java", layout.dir(&kind()))
    );
    assert_eq!(run(&installed.path), "tool 1");
    assert!(!Path::new(&format!("{}/Info.plist", installed.root)).exists());
    assert!(!Path::new(&format!("{}/java", layout.bin())).exists());
}

#[tokio::test]
async fn links_in_an_archive_survive_the_unpack() {
    // llama.cpp's Mac build holds its libraries as chains of links, such as
    // libllama.dylib to libllama.1.dylib to libllama.1.0.0.dylib, and its
    // programs load them by the shorter names.
    let fixture = Fixture::start().await;
    let temp = Temp::new();
    let layout = host(&temp);
    let source = temp.0.join("in/tool-1");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("libx.1.0.0.dylib"), "library").unwrap();
    std::os::unix::fs::symlink("libx.1.0.0.dylib", source.join("libx.1.dylib")).unwrap();
    std::os::unix::fs::symlink("libx.1.dylib", source.join("libx.dylib")).unwrap();
    let entry = source.join("tool");
    std::fs::write(&entry, program("1")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = temp.0.join("out.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(temp.0.join("in"))
        .arg("tool-1")
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(out).unwrap();
    let url = fixture.serve("/tool-1.tar.gz", Answer::Body(bytes.clone()));
    let def = archive_def(&url, Some(sha(&bytes)), Some(bytes.len() as u64));
    let installed = install(&layout, &def, "1").await.unwrap();

    assert_eq!(run(&installed.path), "tool 1");
    let dir = Path::new(&installed.root);
    assert_eq!(
        std::fs::read_to_string(dir.join("libx.dylib")).unwrap(),
        "library"
    );
    assert_eq!(
        std::fs::read_link(dir.join("libx.dylib")).unwrap(),
        Path::new("libx.1.dylib")
    );
    assert!(std::fs::symlink_metadata(dir.join("libx.1.dylib"))
        .unwrap()
        .is_symlink());
}
