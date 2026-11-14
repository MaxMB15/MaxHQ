// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The steps. Each check decides "done" from what the container holds now,
//! never from a record of an earlier run.

use std::sync::LazyLock;

use futures::future::FutureExt;

use super::files::{
    self, BASHRC_PATH, GH_WRAPPER, GH_WRAPPER_PATH, GITCONFIG, GITCONFIG_PATH, GUEST_UNIT,
    GUEST_UNIT_DROP_INS, GUEST_UNIT_NAME, GUEST_UNIT_PATH, TEST_HOST, TEST_HOST_PATH,
};
use maxhq_protocol::model::Tool;
use maxhq_protocol::state::{ToolKind, ToolState};

use super::{ok, Exec, ExecError, ExecUser, Step, AGENT, AGENT_HOME, AGENT_UID, CODEX_HOST, WORK};
use crate::tools;
use crate::tools::install::{flow, Layout, Publishers, Spot};

/// The Debian packages the guest needs. `bubblewrap` and `socat` are for
/// the CLIs' sandboxes, and `poppler-utils` lets Claude read PDFs by page.
pub const PACKAGES: &[&str] = &[
    "git",
    "curl",
    "ca-certificates",
    "poppler-utils",
    "bubblewrap",
    "socat",
    "ripgrep",
    "jq",
    "python3",
    "gh",
];

/// What commands run as the agent user start with, before `PATH`. Incus
/// exec doesn't set up a login, so this gives them the home a login would.
pub const AGENT_ENV: &[&str] = &[
    "env",
    "HOME=/home/agent",
    "USER=agent",
    "LOGNAME=agent",
    "SHELL=/bin/bash",
];

/// The agent user's `PATH`, with Claude's folder first.
pub static AGENT_PATH: LazyLock<String> =
    LazyLock::new(|| format!("PATH={}:/usr/local/bin:/usr/bin:/bin", files::agent_bin()));

/// The oldest Codex the recipe accepts, and the one it installs.
pub const CODEX_MIN: Version = Version {
    major: 0,
    minor: 164,
    patch: 0,
    pre: false,
};
/// Where the release of [`CODEX_MIN`] keeps its downloads.
pub const CODEX_RELEASE: &str = "https://github.com/openai/codex/releases/download/rust-v0.164.0";

/// One Linux build of Codex in the release.
pub struct CodexBuild {
    /// What `uname -m` prints on the machine it runs on.
    pub machine: &'static str,
    /// The release's target name, which names the archive and the binary in it.
    pub target: &'static str,
    /// The archive's SHA-256, as GitHub publishes it for the release asset.
    pub sha256: &'static str,
    /// The SHA-256 of the code-mode host's archive for the same target.
    pub host_sha256: &'static str,
}

/// The release's static musl builds.
pub const CODEX_BUILDS: &[CodexBuild] = &[
    CodexBuild {
        machine: "x86_64",
        target: "x86_64-unknown-linux-musl",
        sha256: "90dd0b03b28afcb7fa733a4f73b413db2916a47347efd43fb669cc1c774ee8e4",
        host_sha256: "feade7026436baa4e82e898050c7ef82a8adbd98163dc504771275b5ef224f2f",
    },
    CodexBuild {
        machine: "aarch64",
        target: "aarch64-unknown-linux-musl",
        sha256: "29a507a2a3fd8f6cd94b96b762d281d299d15d7902dee6132d314b78cf9108eb",
        host_sha256: "0e0bdde24d5b31b05519693635baa9c4f2dc4fe7eee31c67b92155f190dd32d2",
    },
];

/// The Cursor CLI the recipe installs. `agent acp` is hidden and can
/// change with any release, so the version is pinned.
pub const CURSOR_VERSION: &str = "2026.10.22-27dcf5f";
/// The oldest Cursor CLI the recipe accepts: [`CURSOR_VERSION`].
pub const CURSOR_MIN: Version = Version {
    major: 2026,
    minor: 10,
    patch: 22,
    pre: true,
};
/// Where Cursor keeps its CLI's releases.
pub const CURSOR_DOWNLOADS: &str = "https://downloads.cursor.com/lab";

/// Every step, in the order they run. The unit comes last, so the guest
/// starts only once everything it hands out is in place.
pub fn all() -> Vec<Step> {
    vec![
        Step {
            id: "apt",
            version: 1,
            check: |exec| apt_check(exec).boxed(),
            apply: |exec| apt_apply(exec).boxed(),
        },
        Step {
            id: "agent-user",
            version: 1,
            check: |exec| user_check(exec).boxed(),
            apply: |exec| user_apply(exec).boxed(),
        },
        Step {
            id: "agent-path",
            version: 1,
            check: |exec| path_check(exec).boxed(),
            apply: |exec| path_apply(exec).boxed(),
        },
        Step {
            id: "claude",
            version: 4,
            check: |exec| claude_check(exec).boxed(),
            apply: |exec| claude_apply(exec).boxed(),
        },
        Step {
            id: "codex",
            version: 4,
            check: |exec| codex_check(exec).boxed(),
            apply: |exec| codex_apply(exec).boxed(),
        },
        Step {
            id: "cursor",
            version: 4,
            check: |exec| cursor_check(exec).boxed(),
            apply: |exec| cursor_apply(exec).boxed(),
        },
        Step {
            id: "gitconfig",
            version: 1,
            check: |exec| fixed_check(exec, &GITCONFIG_FILE).boxed(),
            apply: |exec| fixed_apply(exec, &GITCONFIG_FILE).boxed(),
        },
        Step {
            id: "gh-wrapper",
            version: 1,
            check: |exec| fixed_check(exec, &GH_WRAPPER_FILE).boxed(),
            apply: |exec| fixed_apply(exec, &GH_WRAPPER_FILE).boxed(),
        },
        Step {
            id: "guest-unit",
            version: 1,
            check: |exec| unit_check(exec).boxed(),
            apply: |exec| unit_apply(exec).boxed(),
        },
    ]
}

/// The `linux` test host's steps: the packages, the test user, who is the
/// agent user, and a guest that's a test host. The drop-in goes in before
/// the unit's step, which reloads systemd and restarts the guest.
pub fn linux() -> Vec<Step> {
    let mut steps: Vec<Step> = all()
        .into_iter()
        .filter(|step| matches!(step.id, "apt" | "agent-user" | "guest-unit"))
        .collect();
    let unit = steps.len() - 1;
    steps.insert(
        unit,
        Step {
            id: "test-host",
            version: 1,
            check: |exec| fixed_check(exec, &TEST_HOST_FILE).boxed(),
            apply: |exec| test_host_apply(exec).boxed(),
        },
    );
    steps
}

/// `cmd` run through [`AGENT_ENV`] and [`AGENT_PATH`].
fn as_agent<'a>(cmd: &[&'a str]) -> Vec<&'a str> {
    let mut full = AGENT_ENV.to_vec();
    full.push(AGENT_PATH.as_str());
    full.extend(cmd);
    full
}

/// The mode, uid and gid of whatever is at `path`, or `None` when nothing
/// is there.
async fn stat(exec: &dyn Exec, path: &str) -> Result<Option<String>, ExecError> {
    let output = exec
        .run(&["stat", "-c", "%a %u %g %F", path], ExecUser::Root, None)
        .await?;
    // stat exits 1 for a path that isn't there, and for any other failure.
    // A step whose check reads false applies and checks again, so a real
    // failure still surfaces.
    Ok(output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// [`stat`] of what `path` leads to, through any links.
async fn stat_followed(exec: &dyn Exec, path: &str) -> Result<Option<String>, ExecError> {
    let output = exec
        .run(
            &["stat", "-L", "-c", "%a %u %g %F", path],
            ExecUser::Root,
            None,
        )
        .await?;
    Ok(output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// What [`stat`] prints for a file or folder with this mode and owner.
fn stat_line(mode: u32, owner: u32, kind: &str) -> String {
    format!("{mode:o} {owner} {owner} {kind}")
}

/// True when every package is installed.
async fn apt_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let mut cmd = vec![
        "dpkg-query",
        "--show",
        "--showformat=${Package} ${db:Status-Status}\\n",
    ];
    cmd.extend(PACKAGES);
    let output = exec.run(&cmd, ExecUser::Root, None).await?;
    // dpkg-query exits 1 when it has never heard of a package, which here
    // means it isn't installed.
    if output.status > 1 {
        return Err(ExecError::failed(&cmd, &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(PACKAGES.iter().all(|package| {
        text.lines()
            .any(|line| line.split_whitespace().eq([*package, "installed"]))
    }))
}

async fn apt_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    ok(exec, &["apt-get", "update", "-q"], ExecUser::Root).await?;
    let mut cmd = vec![
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "-q",
    ];
    cmd.extend(PACKAGES);
    ok(exec, &cmd, ExecUser::Root).await?;
    Ok(())
}

/// One line of `/etc/passwd` or `/etc/group`, as far as the recipe reads it.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    id: u32,
    gid: Option<u32>,
    home: Option<String>,
}

/// Looks up a user or group by name or ID with `getent`.
async fn getent(exec: &dyn Exec, database: &str, key: &str) -> Result<Option<Entry>, ExecError> {
    let cmd = ["getent", database, key];
    let output = exec.run(&cmd, ExecUser::Root, None).await?;
    // getent exits 2 when the key isn't there.
    match output.status {
        0 => {}
        2 => return Ok(None),
        _ => return Err(ExecError::failed(&cmd, &output)),
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = text.lines().next().unwrap_or("").split(':').collect();
    let id = fields.get(2).and_then(|id| id.parse().ok());
    let (Some(name), Some(id)) = (fields.first(), id) else {
        return Err(ExecError::Conflict(format!(
            "getent {database} {key} printed an entry the recipe can't read"
        )));
    };
    let user = database == "passwd";
    Ok(Some(Entry {
        name: (*name).to_owned(),
        id,
        gid: fields
            .get(3)
            .filter(|_| user)
            .and_then(|gid| gid.parse().ok()),
        home: fields
            .get(5)
            .filter(|_| user)
            .map(|home| (*home).to_owned()),
    }))
}

/// The agent user as the recipe makes it.
fn agent_entry() -> Entry {
    Entry {
        name: AGENT.to_owned(),
        id: AGENT_UID,
        gid: Some(AGENT_UID),
        home: Some(AGENT_HOME.to_owned()),
    }
}

/// True when `agent` has uid and gid 1000 and `/home/agent`, and the work
/// folder is a folder the user owns.
async fn user_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    if getent(exec, "passwd", AGENT).await? != Some(agent_entry()) {
        return Ok(false);
    }
    let want = stat_line(0o755, AGENT_UID, "directory");
    Ok(stat(exec, WORK).await?.as_deref() == Some(want.as_str()))
}

async fn user_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    let id = AGENT_UID.to_string();
    // Someone else holding uid or gid 1000, or an agent user made some
    // other way, is for a person to sort out.
    for database in ["passwd", "group"] {
        if let Some(entry) = getent(exec, database, &id).await?
            && entry.name != AGENT
        {
            return Err(ExecError::Conflict(format!(
                "{database} ID {id} belongs to {}, not {AGENT}",
                entry.name
            )));
        }
    }
    let user = getent(exec, "passwd", AGENT).await?;
    if let Some(user) = &user
        && *user != agent_entry()
    {
        return Err(ExecError::Conflict(format!(
            "user {AGENT} exists with uid {}, gid {:?} and home {:?}",
            user.id, user.gid, user.home
        )));
    }
    match getent(exec, "group", AGENT).await? {
        Some(group) if group.id != AGENT_UID => {
            return Err(ExecError::Conflict(format!(
                "group {AGENT} exists with gid {}",
                group.id
            )));
        }
        Some(_) => {}
        None => {
            ok(exec, &["groupadd", "--gid", &id, AGENT], ExecUser::Root).await?;
        }
    }
    if user.is_none() {
        let cmd = [
            "useradd",
            "--uid",
            &id,
            "--gid",
            &id,
            "--create-home",
            "--home-dir",
            AGENT_HOME,
            "--shell",
            "/bin/bash",
            AGENT,
        ];
        ok(exec, &cmd, ExecUser::Root).await?;
    }
    let cmd = ["install", "-d", "-o", &id, "-g", &id, "-m", "0755", WORK];
    ok(exec, &cmd, ExecUser::Root).await?;
    Ok(())
}

/// True when `.bashrc` starts with the recipe's block.
async fn path_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let Some(bytes) = exec.read_as_agent(BASHRC_PATH).await? else {
        return Ok(false);
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(files::bashrc(Some(&text)) == text)
}

async fn path_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    let existing = exec.read_as_agent(BASHRC_PATH).await?;
    let existing = existing.as_deref().map(String::from_utf8_lossy);
    let text = files::bashrc(existing.as_deref());
    exec.write(BASHRC_PATH, text.as_bytes(), 0o644, ExecUser::Agent)
        .await
}

/// True when Claude's launcher runs for the agent user. Any version will
/// do, since Claude updates itself.
async fn claude_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    // Through env, so a missing launcher is exit 127 rather than an exec
    // error.
    let claude = &tools::defs::CLAUDE;
    let mut cmd = vec![claude.container_path];
    cmd.extend(claude.version_args);
    let cmd = as_agent(&cmd);
    Ok(exec.run(&cmd, claude.container_user, None).await?.success())
}

async fn claude_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    install(exec, Tool::Claude, ExecUser::Agent).await
}

/// Installs `tool` through the installer, in the container's layout for
/// `user`, at the version the recipe pins when it pins one. The recipe owns
/// the container, so its links replace whatever holds their names.
async fn install(exec: &dyn Exec, tool: Tool, user: ExecUser) -> Result<(), ExecError> {
    let output = ok(exec, &["uname", "-m"], ExecUser::Root).await?;
    let machine = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let kind = ToolKind::Agent { tool };
    let Some(def) = tools::defs::plan(&kind, true) else {
        return Err(ExecError::Conflict(format!("MaxHQ can't install {tool:?}")));
    };
    // Installs into one container queue per Exec; the lock file covers
    // any other.
    let label = format!("recipe {:p}", exec as *const dyn Exec as *const ());
    let mut layout = Layout::container(&label, user, &machine);
    layout.replace_links = true;
    let spot = Spot { exec, layout };
    let quiet = |_: ToolState| {};
    flow::install(
        &spot,
        &def,
        tools::defs::pin(&kind, true),
        &Publishers::default(),
        &quiet,
    )
    .await?;
    Ok(())
}

/// A release version, as far as the recipe compares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    /// The major number.
    pub major: u64,
    /// The minor number.
    pub minor: u64,
    /// The patch number.
    pub patch: u64,
    /// True for a prerelease, which comes before its release.
    pub pre: bool,
}

impl Version {
    /// Reads `0.164.0`, `v0.164.0` or `0.165.0-alpha.1`. Build metadata
    /// after `+` doesn't count.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, _)) => (core, true),
            None => (text, false),
        };
        let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
        let version = Version {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
            pre,
        };
        parts.next().is_none().then_some(version)
    }

    /// True when this version is `other` or newer.
    pub fn at_least(self, other: Version) -> bool {
        let this = (self.major, self.minor, self.patch);
        let that = (other.major, other.minor, other.patch);
        this > that || (this == that && (other.pre || !self.pre))
    }
}

/// The version `codex --version` prints, as in `codex-cli 0.164.0`.
pub fn codex_version(stdout: &str) -> Option<Version> {
    tools::defs::version_of(stdout)
}

/// True when [`CODEX`](super::CODEX) runs and reports [`CODEX_MIN`] or newer, and the
/// code-mode host at [`CODEX_HOST`] leads to an executable file.
async fn codex_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let host = stat_followed(exec, CODEX_HOST).await?;
    let executable = host.as_deref().is_some_and(|line| {
        let mode = line
            .split(' ')
            .next()
            .and_then(|mode| u32::from_str_radix(mode, 8).ok());
        line.ends_with(" regular file") && mode.is_some_and(|mode| mode & 0o111 != 0)
    });
    if !executable {
        return Ok(false);
    }
    // Through env, so a missing binary is exit 127 rather than an exec
    // error.
    let codex = &tools::defs::CODEX;
    let mut cmd = vec!["env", codex.container_path];
    cmd.extend(codex.version_args);
    let output = exec.run(&cmd, codex.container_user, None).await?;
    if !output.success() {
        return Ok(false);
    }
    let version = codex_version(&String::from_utf8_lossy(&output.stdout));
    Ok(version.is_some_and(|version| version.at_least(CODEX_MIN)))
}

async fn codex_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    install(exec, Tool::Codex, ExecUser::Root).await
}

/// True when [`CURSOR`](super::CURSOR) runs for the agent user and reports
/// [`CURSOR_MIN`] or newer.
async fn cursor_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    let cursor = &tools::defs::CURSOR;
    let mut cmd = vec![cursor.container_path];
    cmd.extend(cursor.version_args);
    let cmd = as_agent(&cmd);
    let output = exec.run(&cmd, cursor.container_user, None).await?;
    if !output.success() {
        return Ok(false);
    }
    let version = tools::defs::version_of(&String::from_utf8_lossy(&output.stdout));
    Ok(version.is_some_and(|version| version.at_least(CURSOR_MIN)))
}

async fn cursor_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    install(exec, Tool::Cursor, ExecUser::Agent).await
}

/// A file the recipe owns whole.
struct Fixed {
    path: &'static str,
    text: &'static str,
    mode: u32,
}

const GITCONFIG_FILE: Fixed = Fixed {
    path: GITCONFIG_PATH,
    text: GITCONFIG,
    mode: 0o644,
};

const GH_WRAPPER_FILE: Fixed = Fixed {
    path: GH_WRAPPER_PATH,
    text: GH_WRAPPER,
    mode: 0o755,
};

const GUEST_UNIT_FILE: Fixed = Fixed {
    path: GUEST_UNIT_PATH,
    text: GUEST_UNIT,
    mode: 0o644,
};

const TEST_HOST_FILE: Fixed = Fixed {
    path: TEST_HOST_PATH,
    text: TEST_HOST,
    mode: 0o644,
};

/// True when the file holds exactly the recipe's text, with its mode and
/// owned by root.
async fn fixed_check(exec: &dyn Exec, file: &Fixed) -> Result<bool, ExecError> {
    if exec.read(file.path).await?.as_deref() != Some(file.text.as_bytes()) {
        return Ok(false);
    }
    let want = stat_line(file.mode, 0, "regular file");
    Ok(stat(exec, file.path).await?.as_deref() == Some(want.as_str()))
}

async fn fixed_apply(exec: &dyn Exec, file: &Fixed) -> Result<(), ExecError> {
    exec.write(file.path, file.text.as_bytes(), file.mode, ExecUser::Root)
        .await
}

/// Writes the test host's drop-in, in a folder of its own.
async fn test_host_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    let folder = [
        "install",
        "-d",
        "-o",
        "0",
        "-g",
        "0",
        "-m",
        "0755",
        GUEST_UNIT_DROP_INS,
    ];
    ok(exec, &folder, ExecUser::Root).await?;
    fixed_apply(exec, &TEST_HOST_FILE).await
}

/// True when the unit file is the recipe's and systemd has it enabled.
async fn unit_check(exec: &dyn Exec) -> Result<bool, ExecError> {
    if !fixed_check(exec, &GUEST_UNIT_FILE).await? {
        return Ok(false);
    }
    // is-enabled exits 1 for a unit that's disabled or not loaded.
    let output = exec
        .run(
            &["systemctl", "is-enabled", GUEST_UNIT_NAME],
            ExecUser::Root,
            None,
        )
        .await?;
    Ok(output.success() && String::from_utf8_lossy(&output.stdout).trim() == "enabled")
}

/// Writes the unit, then enables and restarts it so a changed unit takes
/// effect.
async fn unit_apply(exec: &dyn Exec) -> Result<(), ExecError> {
    fixed_apply(exec, &GUEST_UNIT_FILE).await?;
    ok(exec, &["systemctl", "daemon-reload"], ExecUser::Root).await?;
    ok(
        exec,
        &["systemctl", "enable", GUEST_UNIT_NAME],
        ExecUser::Root,
    )
    .await?;
    ok(
        exec,
        &["systemctl", "restart", GUEST_UNIT_NAME],
        ExecUser::Root,
    )
    .await?;
    Ok(())
}
