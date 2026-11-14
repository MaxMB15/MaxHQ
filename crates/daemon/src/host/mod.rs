// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host spec at work on this device. `maxd host check` compares the
//! device with its `maxhq-host.toml` and says how each item stands,
//! `maxd host apply` does the part that needs no root, and
//! `maxd host apply --system` does the root part on Debian and Ubuntu. The
//! check goes out as this device's [`HostReport`].
//!
//! Everything reads the system through an [`Env`]: a root folder that is
//! `/` for real and a temp folder in tests, the Incus client, and how to
//! find a program. Commands go through [`crate::service::Runner`].

mod apply;
mod check;
mod render;
pub mod system;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use maxhq_core::host_spec::{self, HostSpec};
use maxhq_protocol::hosts::{HostFix, HostItem, HostItemState, HostReport, HostSource};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use crate::hub::Hub;
use crate::incus::Incus;
use crate::service::Setup;

pub use apply::{apply_user, Restart};
pub use render::render;

/// Where the NixOS module renders the spec, which marks the host as one
/// the module manages.
pub const NIX_SPEC: &str = "/etc/maxhq/host.json";

/// The NixOS change for a host whose module doesn't read the spec yet,
/// and for the items the spec alone turns on.
pub const HOST_CHANGE: &str = "services.maxd.host = lib.importTOML ./maxhq-host.toml;";

/// The group whose members may use the Incus socket.
pub const INCUS_GROUP: &str = "incus-admin";

/// Lets one apply run at a time, so two never make the same pool.
static APPLYING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The system the device runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Os {
    /// macOS, where only the user service and the tools apply.
    Mac,
    /// NixOS, where the module does the root part.
    Nix,
    /// Debian or Ubuntu, where `--system` does the root part.
    Apt {
        /// `debian` or `ubuntu`.
        id: String,
        /// Such as `13` or `24.04`, when the release names one.
        version: Option<String>,
        /// Such as `trixie` or `noble`.
        codename: Option<String>,
    },
    /// Any other Linux, named by its `ID`.
    Other(String),
}

impl Os {
    /// Reads `/etc/os-release` under `root`. Off Linux it's macOS.
    pub fn read(root: &Path) -> Self {
        if cfg!(target_os = "macos") {
            return Os::Mac;
        }
        let text = std::fs::read_to_string(root.join("etc/os-release")).unwrap_or_default();
        Self::from_release(&text)
    }

    /// The system an `os-release` file names.
    pub fn from_release(text: &str) -> Self {
        let field = |name: &str| {
            text.lines().find_map(|line| {
                let value = line.strip_prefix(name)?.strip_prefix('=')?;
                Some(value.trim().trim_matches('"').to_owned())
            })
        };
        let id = field("ID").unwrap_or_else(|| "linux".to_owned());
        match id.as_str() {
            "nixos" => Os::Nix,
            "debian" | "ubuntu" => Os::Apt {
                id,
                version: field("VERSION_ID").filter(|v| !v.is_empty()),
                codename: field("VERSION_CODENAME").filter(|v| !v.is_empty()),
            },
            _ => Os::Other(id),
        }
    }

    /// True on Linux, where Incus, cgroups and systemd apply.
    pub fn is_linux(&self) -> bool {
        *self != Os::Mac
    }

    /// True when the distro's own archive has Incus: Debian 13 and Ubuntu
    /// 24.04 or later, or a release with no version, such as testing.
    pub fn packages_incus(&self) -> bool {
        let Os::Apt { id, version, .. } = self else {
            return false;
        };
        let Some(version) = version else {
            return true;
        };
        let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
        let major = parts.next().flatten().unwrap_or(0);
        let minor = parts.next().flatten().unwrap_or(0);
        match id.as_str() {
            "debian" => major >= 13,
            _ => (major, minor) >= (24, 4),
        }
    }
}

/// The user maxd runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    /// The login name.
    pub name: String,
    /// The uid.
    pub uid: u32,
    /// The primary gid.
    pub gid: u32,
    /// The home folder.
    pub home: PathBuf,
}

impl User {
    /// The user called `name` in `/etc/passwd` under `root`.
    pub fn lookup(root: &Path, name: &str) -> Option<Self> {
        let passwd = std::fs::read_to_string(root.join("etc/passwd")).ok()?;
        passwd.lines().find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 7 || fields[0] != name {
                return None;
            }
            Some(Self {
                name: name.to_owned(),
                uid: fields[2].parse().ok()?,
                gid: fields[3].parse().ok()?,
                home: PathBuf::from(fields[5]),
            })
        })
    }
}

/// Finds a program by name.
pub type Find = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// What the check and apply read the system through.
pub struct Env {
    /// `/` for real, a temp folder in tests. Every system path is read
    /// under it.
    pub root: PathBuf,
    /// The system.
    pub os: Os,
    /// Who maxd runs as.
    pub user: User,
    /// The maxd the units and the root command run.
    pub maxd: PathBuf,
    /// The Incus client, on Linux.
    pub incus: Option<Incus>,
    /// The user service's setup, unless it can't be told here.
    pub service: Option<Setup>,
    /// Finds a program by name, the way the user's shell would.
    pub find: Find,
}

impl Env {
    /// This device, as the user running maxd.
    pub fn this_device() -> Result<Self, String> {
        let root = PathBuf::from("/");
        let os = Os::read(&root);
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or("HOME isn't set, so maxd can't find the host spec")?;
        let name = std::env::var("USER")
            .ok()
            .filter(|name| !name.is_empty())
            .or_else(|| {
                let home = home.to_string_lossy();
                home.rsplit('/').next().map(str::to_owned)
            })
            .unwrap_or_default();
        let maxd = this_maxd(&home);
        Ok(Self {
            incus: os.is_linux().then(|| Incus::new(crate::incus::SOCKET)),
            service: Setup::from_env().ok(),
            find: Box::new(crate::service::find_program),
            root,
            os,
            user: User {
                name,
                uid,
                gid,
                home,
            },
            maxd,
        })
    }

    /// A system path under [`Env::root`].
    pub fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// A system file's text, or None when it can't be read.
    pub fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.at(path)).ok()
    }

    /// The one root command that sets up this host.
    pub fn root_command(&self) -> String {
        format!("sudo {} host apply --system", self.maxd.display())
    }

    /// The fix for an item the root part sets up: the root command where
    /// `--system` runs, and none off Debian, Ubuntu and NixOS. On NixOS it's
    /// `change`, the module option that renders the item, once the module
    /// reads the spec, and [`HOST_CHANGE`] until then.
    pub fn root_fix(&self, change: &str) -> Option<HostFix> {
        match self.os {
            Os::Nix => {
                let managed = self.at(NIX_SPEC).exists();
                let change = if managed { change } else { HOST_CHANGE };
                Some(HostFix::NixChange {
                    change: change.to_owned(),
                })
            }
            Os::Apt { .. } => Some(HostFix::RootCommand {
                command: self.root_command(),
            }),
            _ => None,
        }
    }
}

/// The maxd that's running, through `~/.local/bin/maxd` when that leads
/// to it, so the units keep running the newest after an update.
pub fn this_maxd(home: &Path) -> PathBuf {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| home.join(".local/bin/maxd"));
    crate::service::stable_exe(home, exe)
}

/// Where the user's spec file lives.
pub fn spec_path(os: &Os, home: &Path) -> PathBuf {
    match os {
        Os::Mac => home.join("Library/Application Support/MaxHQ"),
        _ => home.join(".config/maxhq"),
    }
    .join(host_spec::FILE)
}

/// A spec and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// The spec.
    pub spec: HostSpec,
    /// Where it came from.
    pub source: HostSource,
}

/// Reads the spec: the NixOS module's rendering when there is one, else
/// the user's `maxhq-host.toml`, else the defaults.
pub fn load(env: &Env) -> Result<Loaded, String> {
    if let Some(json) = env.read(NIX_SPEC) {
        let spec = HostSpec::from_json(&json).map_err(|e| e.to_string())?;
        return Ok(Loaded {
            spec,
            source: HostSource::Nix,
        });
    }
    let path = spec_path(&env.os, &env.user.home);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Loaded {
            spec: HostSpec::from_toml(&text).map_err(|e| e.to_string())?,
            source: HostSource::File {
                path: path.to_string_lossy().into_owned(),
            },
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
            spec: HostSpec::default(),
            source: HostSource::Defaults,
        }),
        Err(err) => Err(format!("couldn't read {}: {err}", path.display())),
    }
}

/// Checks the device against its spec.
pub async fn report(env: &Env, device: DeviceId) -> Result<HostReport, String> {
    let loaded = load(env)?;
    let items = check::items(env, &loaded.spec).await;
    Ok(HostReport {
        device,
        source: loaded.source,
        checked_at: Timestamp::now(),
        items,
    })
}

/// True once every item is fine.
pub fn all_ok(report: &HostReport) -> bool {
    report
        .items
        .iter()
        .all(|item| item.state == HostItemState::Ok)
}

/// This device, read off the async threads, since the service setup runs
/// a command.
async fn this_device() -> Result<Env, String> {
    tokio::task::spawn_blocking(Env::this_device)
        .await
        .map_err(|_| "maxd couldn't read this device".to_owned())?
}

/// Checks this device and publishes the report.
async fn check_and_publish(hub: &Hub) -> Result<HostReport, String> {
    let env = this_device().await?;
    let report = report(&env, hub.device().id).await?;
    hub.apply([Change::Upsert {
        value: Entity::HostReport(report.clone()),
    }]);
    Ok(report)
}

/// The check `maxd run` makes when it starts.
pub async fn publish(hub: Hub) {
    if let Err(err) = check_and_publish(&hub).await {
        warn!("the host check didn't run: {err}");
    }
}

/// `CheckHost`.
pub async fn check_request(hub: Hub) -> ActionOutcome {
    match check_and_publish(&hub).await {
        Ok(_) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// `ApplyHost`: fixes `items` that maxd can fix itself, then checks again.
pub async fn apply_request(hub: Hub, items: Vec<String>) -> ActionOutcome {
    let applied = async {
        let env = this_device().await?;
        let loaded = load(&env)?;
        let _turn = APPLYING.lock().await;
        let found = check::items(&env, &loaded.spec).await;
        apply_user(
            &env,
            &loaded.spec,
            &found,
            Some(&items),
            apply::Restart::Later,
        )
        .await
    }
    .await;
    let checked = check_and_publish(&hub).await;
    match (applied, checked) {
        (Ok(_), Ok(_)) => ActionOutcome::Done,
        (Err(reason), _) | (_, Err(reason)) => ActionOutcome::Refused { reason },
    }
}

/// One line of `maxd host check` for an item.
pub fn line(item: &HostItem) -> String {
    let state = match &item.state {
        HostItemState::Ok => "ok".to_owned(),
        HostItemState::Drift { want, have } => format!("differs: wants {want}, has {have}"),
        HostItemState::Missing => "missing".to_owned(),
        HostItemState::Unknown { why } => format!("can't tell: {why}"),
    };
    let fix = match &item.fix {
        None => String::new(),
        Some(HostFix::Now) => "\n    fix: maxd host apply".to_owned(),
        Some(HostFix::RootCommand { command }) => format!("\n    fix: {command}"),
        Some(HostFix::NixChange { change }) => {
            format!("\n    fix: add `{change}` to the NixOS configuration and rebuild")
        }
    };
    format!("{}: {state}{fix}", item.label)
}

/// What the report's source means, for `maxd host check`.
pub fn source_line(source: &HostSource) -> String {
    match source {
        HostSource::File { path } => format!("Spec: {path}"),
        HostSource::Nix => format!("Spec: {NIX_SPEC}, from the NixOS module"),
        HostSource::Defaults => {
            "No maxhq-host.toml here, so the check uses the defaults.".to_owned()
        }
    }
}
