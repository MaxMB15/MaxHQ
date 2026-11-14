// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The one installer for every tool MaxHQ puts on a device or in a
//! container workspace: the agent CLIs, the helpers and ACP registry
//! agents. Callers use only [`Installer`], so their tests take a fake.
//!
//! Each tool goes under `~/.local/lib/maxhq/tools/<tool>/<version>/`, is
//! published by renaming a staging folder, and has a `current` link that a
//! rename flips. Its command gets a link in `~/.local/bin` when nothing of
//! yours holds that name. In a container workspace the root is the agent's
//! `~/.local`, or `/usr/local` for a tool root installs. The same code runs
//! for both, over [`Exec`].
//!
//! [`Setup`] is the installer maxd runs. The recipe calls [`flow`] itself
//! with the container it's making.

use std::sync::Arc;

use futures::future::BoxFuture;
use maxhq_protocol::state::{ToolKind, ToolPlace, ToolState};

use crate::recipe::{Exec, ExecError, ExecOutput, ExecUser};

pub mod fetch;
pub mod flow;
mod lock;
pub mod releases;
pub mod scripts;
#[cfg(test)]
pub(crate) mod tests;

pub use releases::{Publishers, Releases};

/// A tool and where its files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallDef {
    /// Which tool.
    pub kind: ToolKind,
    /// The command its link in `~/.local/bin` takes, if it gets one.
    pub binary: Option<String>,
    /// Where its files come from.
    pub source: Source,
}

/// Where a tool's files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The tool's own install script, after which the tool updates itself.
    Script {
        /// The script's URL.
        url: String,
    },
    /// An archive per target.
    Archive {
        /// One per target the tool ships for.
        files: Vec<ArchiveFile>,
        /// You agreed to install a file with no checksum, as an ACP
        /// registry entry may be. Without it such a file is refused.
        confirmed_unchecked: bool,
    },
    /// An npm package at an exact version, in its own prefix, run with
    /// MaxHQ's Node.
    Npm {
        /// The package's name.
        package: String,
    },
    /// A Python package, run with uvx.
    Uv {
        /// The package's name.
        package: String,
    },
}

/// One archive of a tool, for one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveFile {
    /// The target, as `maxd` names its own build's, such as
    /// `aarch64-apple-darwin`.
    pub target: String,
    /// Where to fetch it.
    pub url: String,
    /// Its SHA-256 in hex, when the source gives one.
    pub sha256: Option<String>,
    /// Its size in bytes, when the source gives one.
    pub size: Option<u64>,
}

/// What an install put in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The version now current.
    pub version: String,
    /// The command to run, through its link when it has one.
    pub path: String,
    /// The version's folder.
    pub root: String,
}

/// Why an install or a removal failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstallError {
    /// This device can't install yet.
    #[error("{}", crate::hosts::NOT_YET)]
    NotBuilt,
    /// An archive has no checksum and you didn't agree to that.
    #[error("the download has no checksum to check it against")]
    Unchecked,
    /// The download wasn't what its checksum says.
    #[error("the download's SHA-256 is {got}, not {want}")]
    Checksum {
        /// What the source says.
        want: String,
        /// What arrived.
        got: String,
    },
    /// The tool ships nothing for this target.
    #[error("there's no build for {0}")]
    NoTarget(String),
    /// Anything else, in words.
    #[error("{0}")]
    Failed(String),
}

/// Installs and removes tools in one place or another.
pub trait Installer: Send + Sync {
    /// Makes sure `def` is installed in `place` at `version`, or at the
    /// latest when it's none, and returns what's current there.
    fn ensure<'a>(
        &'a self,
        def: &'a InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<Installed, InstallError>>;

    /// Removes a tool MaxHQ installed from `place`, with its link.
    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), InstallError>>;
}

/// Why the steps of an install stopped: a command, or the install itself.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A command failed or didn't reach the place.
    #[error(transparent)]
    Exec(#[from] ExecError),
    /// The install refused or failed.
    #[error(transparent)]
    Install(#[from] InstallError),
}

impl From<Error> for InstallError {
    fn from(err: Error) -> Self {
        match err {
            Error::Install(err) => err,
            Error::Exec(err) => InstallError::Failed(err.to_string()),
        }
    }
}

impl From<Error> for ExecError {
    fn from(err: Error) -> Self {
        match err {
            Error::Exec(err) => err,
            Error::Install(err) => ExecError::Conflict(err.to_string()),
        }
    }
}

/// The target a machine's archives are for: the static musl build on
/// Linux, as maxd's own, and the Apple one on a Mac. It takes a target or
/// what `uname -m` prints, which counts as Linux.
pub fn target_for(text: &str) -> String {
    let arch = if text.starts_with("aarch64") || text.starts_with("arm64") {
        "aarch64"
    } else if text.starts_with("x86_64") || text.starts_with("amd64") {
        "x86_64"
    } else {
        return text.to_owned();
    };
    if text.contains("apple") {
        format!("{arch}-apple-darwin")
    } else {
        format!("{arch}-unknown-linux-musl")
    }
}

/// Where in a place tools go, and who installs them there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Names the place, so installs from this maxd queue per place.
    pub label: String,
    /// The folder that holds `lib/maxhq/tools` and `bin`.
    pub root: String,
    /// Who installs.
    pub user: ExecUser,
    /// The target archives are picked for, as [`target_for`] gives it.
    pub target: String,
    /// What every command starts with, such as `env HOME=...`.
    pub env: Vec<String>,
    /// Whether a link replaces whatever holds its name. Only the recipe
    /// sets it, in a container MaxHQ is making.
    pub replace_links: bool,
}

impl Layout {
    /// The device itself, for the user `home` belongs to.
    pub fn host(home: &str) -> Self {
        Self {
            label: "host".to_owned(),
            root: format!("{home}/.local"),
            user: ExecUser::Agent,
            target: target_for(crate::identity::TARGET),
            env: Vec::new(),
            replace_links: false,
        }
    }

    /// A container workspace, as `user`, on a machine `uname -m` names.
    /// The agent's tools go in its `~/.local`, root's in `/usr/local`.
    pub fn container(label: &str, user: ExecUser, machine: &str) -> Self {
        let (root, env) = match user {
            ExecUser::Agent => {
                let mut env: Vec<String> = crate::recipe::steps::AGENT_ENV
                    .iter()
                    .map(|word| (*word).to_owned())
                    .collect();
                env.push(crate::recipe::steps::AGENT_PATH.clone());
                (format!("{}/.local", crate::recipe::AGENT_HOME), env)
            }
            ExecUser::Root => (
                "/usr/local".to_owned(),
                ["env", "HOME=/root", "PATH=/usr/local/bin:/usr/bin:/bin"]
                    .map(str::to_owned)
                    .to_vec(),
            ),
        };
        Self {
            label: label.to_owned(),
            root,
            user,
            target: target_for(machine.trim()),
            env,
            replace_links: false,
        }
    }

    /// The folder every tool's folder is in.
    pub fn tools(&self) -> String {
        format!("{}/lib/maxhq/tools", self.root)
    }

    /// The folder links go in.
    pub fn bin(&self) -> String {
        format!("{}/bin", self.root)
    }

    /// The folder `kind` goes in.
    pub fn dir(&self, kind: &ToolKind) -> String {
        format!("{}/{}", self.tools(), name(kind))
    }
}

/// A place to install in: how to reach it and its layout.
pub struct Spot<'a> {
    /// Runs commands there.
    pub exec: &'a dyn Exec,
    /// Where things go.
    pub layout: Layout,
}

impl Spot<'_> {
    /// Runs `cmd` after the layout's `env`, as its user.
    pub async fn run(&self, cmd: &[&str], stdin: Option<&[u8]>) -> Result<ExecOutput, Error> {
        let mut full: Vec<&str> = self.layout.env.iter().map(String::as_str).collect();
        full.extend(cmd);
        Ok(self.exec.run(&full, self.layout.user, stdin).await?)
    }

    /// Runs a program through `env`, so a missing one exits 127.
    pub async fn command(&self, cmd: &[&str]) -> Result<ExecOutput, Error> {
        let mut full: Vec<&str> = if self.layout.env.is_empty() {
            vec!["env"]
        } else {
            self.layout.env.iter().map(String::as_str).collect()
        };
        full.extend(cmd);
        Ok(self.exec.run(&full, self.layout.user, None).await?)
    }

    /// Runs one of the installer's scripts with `args`.
    pub async fn sh(&self, script: &str, args: &[&str]) -> Result<ExecOutput, Error> {
        let mut cmd = vec!["sh", "-c", script, "maxhq-install"];
        cmd.extend(args);
        self.run(&cmd, None).await
    }

    /// Runs one of the installer's scripts and fails unless it exits 0.
    pub async fn sh_ok(&self, script: &str, args: &[&str]) -> Result<ExecOutput, Error> {
        let output = self.sh(script, args).await?;
        if output.success() {
            Ok(output)
        } else {
            Err(self.failed(script, &output))
        }
    }

    /// The error for a script that exited badly.
    pub fn failed(&self, script: &str, output: &ExecOutput) -> Error {
        Error::Exec(ExecError::failed(&["sh", "-c", script], output))
    }
}

/// The name of `kind`'s folder, as `node`, `codex` or `acp-gemini`.
pub fn name(kind: &ToolKind) -> String {
    let word = |value: serde_json::Value| value.as_str().unwrap_or("tool").replace('_', "-");
    match kind {
        ToolKind::Agent { tool } => word(serde_json::to_value(tool).unwrap_or_default()),
        ToolKind::Helper { tool } => word(serde_json::to_value(tool).unwrap_or_default()),
        ToolKind::Acp { id } => {
            let id: String = id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            format!("acp-{}", id.trim_start_matches('.'))
        }
    }
}

/// Told where an install is.
pub type Report<'a> = &'a (dyn Fn(ToolState) + Send + Sync);

/// A place opened for installs: what runs commands there and its layout.
pub type Opened = (Arc<dyn Exec>, Layout);

/// Opens a place for installs: what runs commands there and its layout,
/// which may differ by tool.
pub trait Places: Send + Sync {
    /// The place `place` is, for `kind`.
    fn open<'a>(
        &'a self,
        place: &'a ToolPlace,
        kind: &'a ToolKind,
    ) -> BoxFuture<'a, Result<Opened, InstallError>>;
}

/// Hears each install's progress.
pub type Listener = Arc<dyn Fn(&ToolKind, &ToolPlace, ToolState) + Send + Sync>;

/// The installer maxd runs: it opens the place, looks releases up and
/// tells `listener` how each install is going.
pub struct Setup {
    places: Arc<dyn Places>,
    releases: Arc<dyn Releases>,
    listener: Listener,
}

impl Setup {
    /// An installer over `places` and `releases`.
    pub fn new(places: Arc<dyn Places>, releases: Arc<dyn Releases>, listener: Listener) -> Self {
        Self {
            places,
            releases,
            listener,
        }
    }
}

impl Installer for Setup {
    fn ensure<'a>(
        &'a self,
        def: &'a InstallDef,
        version: Option<&'a str>,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<Installed, InstallError>> {
        Box::pin(async move {
            let (exec, layout) = self.places.open(place, &def.kind).await?;
            let spot = Spot {
                exec: exec.as_ref(),
                layout,
            };
            let report = |state: ToolState| (self.listener)(&def.kind, place, state);
            Ok(flow::install(&spot, def, version, self.releases.as_ref(), &report).await?)
        })
    }

    fn remove<'a>(
        &'a self,
        kind: &'a ToolKind,
        place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), InstallError>> {
        Box::pin(async move {
            let (exec, layout) = self.places.open(place, kind).await?;
            let spot = Spot {
                exec: exec.as_ref(),
                layout,
            };
            Ok(flow::remove(&spot, kind).await?)
        })
    }
}

/// The installer on a device that can't install: every call says so.
pub struct Unbuilt;

impl Installer for Unbuilt {
    fn ensure<'a>(
        &'a self,
        _def: &'a InstallDef,
        _version: Option<&'a str>,
        _place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<Installed, InstallError>> {
        Box::pin(async { Err(InstallError::NotBuilt) })
    }

    fn remove<'a>(
        &'a self,
        _kind: &'a ToolKind,
        _place: &'a ToolPlace,
    ) -> BoxFuture<'a, Result<(), InstallError>> {
        Box::pin(async { Err(InstallError::NotBuilt) })
    }
}
