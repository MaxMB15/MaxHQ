// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// the agent-device pin, the target, config and session flags its launcher
// fills in, which flags only count before `--`, and that the agent can't
// pass the flags it fills in or the ones that pick the daemon. That code
// refuses a few subcommands; this names the ones that may run instead.

//! Which agent-device commands a lease may run, and the flags `maxd` adds.

/// The agent-device release `maxd` installs and drives. Its flags and
/// subcommands below match this release, so a bump checks them too.
pub const VERSION: &str = "0.21.31";

/// The subcommands a lease may run. Anything else, such as `connect`,
/// `daemon`, `auth` or a subcommand a later release adds, is refused until
/// it's checked.
pub const ALLOWED: &[&str] = &[
    "snapshot",
    "press",
    "fill",
    "type",
    "swipe",
    "scroll",
    "back",
    "home",
    "open",
    "close",
    "alert",
    "settings",
    "screenshot",
    "logs",
    "wait",
];

/// Flags the agent can't pass, since `maxd` fills them in or they pick
/// another daemon, device, session or place to write.
const REFUSED_FLAGS: &[&str] = &[
    "--platform",
    "--udid",
    "--serial",
    "--device",
    "-d",
    "-s",
    "--config",
    "--remote-config",
    "--state-dir",
    "--target",
    "--tenant",
    "--run-id",
    "--bearer-token",
    "--header",
    "--host",
    "--port",
    "--listen-host",
    "--status-host",
    "--proxy-base-url",
    "--public-base-url",
    "--ios-simulator-device-set",
    "--android-device-allowlist",
    "--runtime-file",
    "--out",
    "--artifacts-dir",
    "--report-junit",
    "--github-actions-artifact",
];

/// Starts of flag names the agent can't pass, for the families above.
const REFUSED_PREFIXES: &[&str] = &[
    "--session",
    "--daemon-",
    "--provider",
    "--aws-",
    "--lease-",
    "--ios-xctest",
];

/// Flags a lease can't pass, since they write a file outside its folder,
/// replace a file that's there, hand words to the platform's launch command,
/// or change what the app loads. `maxd`'s own calls pass none of them.
const LEASE_REFUSED_FLAGS: &[&str] = &[
    "--save-script",
    "--launch-console",
    "--launch-args",
    "--force",
    "--overwrite",
    "--bundle-url",
    "--launch-url",
    "--metro-host",
    "--metro-port",
];

/// What `install` calls an app whose file name isn't plain.
const FALLBACK_APP_NAME: &str = "app";

/// Where a lease's commands run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// An iOS Simulator, named by its UDID.
    Ios,
    /// An Android emulator, named by its serial.
    Android,
}

/// The machine a lease drives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Its platform.
    pub platform: Platform,
    /// The Simulator's UDID or the emulator's serial.
    pub id: String,
}

impl Target {
    /// The flags that point agent-device at it.
    pub fn args(&self) -> [&str; 4] {
        match self.platform {
            Platform::Ios => ["--platform", "ios", "--udid", &self.id],
            Platform::Android => ["--platform", "android", "--serial", &self.id],
        }
    }
}

/// Checks a command the agent gave, as the subcommand first and its
/// arguments after. The answer is why it can't run, for the agent to read.
pub fn check(command: &[String]) -> Result<(), String> {
    let Some(name) = command.first() else {
        return Err(format!(
            "Give an agent-device command, such as snapshot -i. These can run: {}.",
            ALLOWED.join(", ")
        ));
    };
    if !ALLOWED.contains(&name.as_str()) {
        return Err(format!(
            "`{name}` isn't a command a lease can run. These can: {}.",
            ALLOWED.join(", ")
        ));
    }
    if name == "screenshot" {
        // A screenshot's file stays in the lease's own folder, whether it's
        // named before a `--` or after one, where a dash starts a name too.
        let before = options(command).iter().filter(|arg| !arg.starts_with('-'));
        let after = command[options(command).len()..].iter().skip(1);
        if let Some(path) = before.chain(after).find(|path| !inside(path)) {
            return Err(format!(
                "Save the screenshot under a name such as shot.png, not at `{path}`, or call lease_screenshot to look."
            ));
        }
    }
    for arg in options(command) {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if LEASE_REFUSED_FLAGS.contains(&flag) {
            return Err(format!(
                "A lease can't pass `{flag}`. It can write outside the lease's folder or change what the app loads. To open a dev server, use open <app> <url>."
            ));
        }
        let refused = REFUSED_FLAGS.contains(&flag)
            || (flag.starts_with("--") && REFUSED_PREFIXES.iter().any(|p| flag.starts_with(p)));
        if refused {
            return Err(format!(
                "MaxHQ sets `{flag}` itself, or keeps it for the device's host, so leave it out."
            ));
        }
    }
    Ok(())
}

/// Whether `path` is relative and stays beneath the folder it's read in. A
/// leading `~` counts as outside, in case something expands it.
fn inside(path: &str) -> bool {
    use std::path::{Component, Path};
    !path.starts_with('~')
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// What `install` calls the app in the file named `stem`: the stem when it's
/// plain, and a fixed name otherwise, so it can't read as a flag.
pub fn install_name(stem: &str) -> &str {
    let plain = stem.starts_with(char::is_alphanumeric)
        && stem
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'));
    if plain {
        stem
    } else {
        FALLBACK_APP_NAME
    }
}

/// The arguments before any `--`, which are the ones agent-device reads
/// as flags.
fn options(command: &[String]) -> &[String] {
    let end = command
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(command.len());
    &command[..end]
}

/// `command` with `bound` added before any `--`, so they're read as flags.
pub fn bind(command: &[String], bound: &[&str]) -> Vec<String> {
    let end = options(command).len();
    let mut args = Vec::with_capacity(command.len() + bound.len());
    args.extend_from_slice(&command[..end]);
    args.extend(bound.iter().map(|arg| (*arg).to_owned()));
    args.extend_from_slice(&command[end..]);
    args
}
