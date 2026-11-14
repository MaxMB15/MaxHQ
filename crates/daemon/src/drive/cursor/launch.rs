// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The command line that starts a held `agent acp` for one Cursor agent.
//! The global options go before `acp`, as Cursor's reference lists them
//! for every command, and 2026.10.22 takes them there.
//!
//! There's no `--model`: it takes the CLI's model names, such as `auto`,
//! which aren't the IDs the session's config options list, so the driver
//! asks the session for the model instead.
//!
//! [`login_launch`] runs `agent login` under the same variables, so the
//! login it stores is the one a session reads.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use maxhq_protocol::model::PermissionMode;

use crate::drive::acp::launch::{path_first, runnable};
use crate::drive::{Framing, Launch, Login, LoginAuth, Place};

/// Keeps a repository's `.cursor/cli.json` from replacing the owner's allow
/// and deny lists, which would let matching commands run with no prompt and
/// no sandbox. The CLI hides this flag from its help.
const NO_PROJECT_CONFIGS: &str = "--disable-project-configs";

/// The names Cursor's installer gives the CLI. `cursor-agent` comes first,
/// since another tool may also be called `agent`.
const NAMES: [&str; 2] = ["cursor-agent", "agent"];

/// Where the installer puts the CLI under the home folder, tried after
/// `PATH`.
const HOME_DIRS: &[&str] = &[".local/bin", ".cursor/bin"];

/// System-wide places, tried last.
const SYSTEM_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The variable an API key login reaches the CLI in. `initialize` lists
/// only `cursor_login` as a way to sign in, so the key works only if the
/// CLI reads the variable itself, as its docs say it does.
pub const API_KEY: &str = "CURSOR_API_KEY";

/// The variable for an auth token, which no MaxHQ login passes on.
pub const AUTH_TOKEN: &str = "CURSOR_AUTH_TOKEN";

/// How to run `agent acp` for an agent in `place` under `login`.
pub fn launch(login: &Login, place: &Place, mode: PermissionMode) -> Launch {
    command(&Host::here(), login, place, mode)
}

/// How to run `agent login` on this device for `login`, a CLI login,
/// under the variables a session of it gets. The CLI prints its link
/// instead of opening a browser here.
pub fn login_launch(login: &Login) -> Launch {
    login_command(&Host::here(), login)
}

/// The Cursor CLI this device runs, from `PATH` or a usual install folder.
pub fn program() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    find(env::var_os("PATH").as_deref(), home.as_deref())
}

/// What `launch` reads from `maxd`'s own process, kept apart so tests can
/// build a command line without it.
pub(super) struct Host {
    /// The CLI to run.
    pub program: PathBuf,
    /// `maxd`'s `PATH`.
    pub path: Option<OsString>,
    /// `maxd`'s home folder.
    pub home: Option<PathBuf>,
}

impl Host {
    /// What `maxd`'s own process gives.
    fn here() -> Self {
        Self {
            program: program().unwrap_or_else(|| PathBuf::from(NAMES[0])),
            path: env::var_os("PATH"),
            home: env::var_os("HOME").map(PathBuf::from),
        }
    }
}

/// Builds the command line from `host` without reading anything else.
/// Full access asks for Cursor's sandbox off and lets every command run, and
/// every other mode asks for it on and has the CLI ask. Cursor 2026.10.22
/// doesn't use the sandbox flag over ACP, so its commands run unsandboxed
/// whatever this says; the flag stays for a version that does. Only its
/// prompts keep a command from running.
pub(super) fn command(host: &Host, login: &Login, place: &Place, mode: PermissionMode) -> Launch {
    let mut args: Vec<OsString> = vec![
        NO_PROJECT_CONFIGS.into(),
        "--trust".into(),
        "--sandbox".into(),
    ];
    if mode == PermissionMode::FullAccess {
        args.push("disabled".into());
        args.push("--force".into());
    } else {
        args.push("enabled".into());
    }
    if mode == PermissionMode::Plan {
        args.push("--mode".into());
        args.push("plan".into());
    }
    args.push("acp".into());
    under(host, login, args, place.cwd.clone())
}

/// Builds `agent login` from `host`. It runs in the home folder, since a
/// CLI that never signed in may have no config folder yet. `NO_COLOR`
/// keeps escape codes out of the link it prints.
pub(super) fn login_command(host: &Host, login: &Login) -> Launch {
    let cwd = host
        .home
        .clone()
        .unwrap_or_else(|| login.config_dir.clone());
    let mut launch = under(host, login, vec!["login".into()], cwd);
    launch.env.push(("NO_OPEN_BROWSER".into(), "1".into()));
    launch.env.push(("NO_COLOR".into(), "1".into()));
    launch
}

/// `host`'s CLI with `args` in `cwd`, under `login` and with the variables
/// MaxHQ sets and scrubs.
fn under(host: &Host, login: &Login, args: Vec<OsString>, cwd: PathBuf) -> Launch {
    let env: Vec<(OsString, OsString)> = path_first(&host.program, host.path.as_deref())
        .into_iter()
        .collect();

    // An API key login gets its key on the holder's pipe, and only then is
    // the key's variable kept out of the scrub. A CLI login on the Mac
    // never sees a key or token `maxd` happens to carry.
    let mut secret_env = Vec::new();
    let mut env_remove: Vec<OsString> = vec!["NODE_OPTIONS".into(), AUTH_TOKEN.into()];
    match &login.auth {
        LoginAuth::CursorApiKey(key) => secret_env.push((API_KEY.into(), key.clone())),
        _ => env_remove.push(API_KEY.into()),
    }

    Launch {
        program: host.program.clone(),
        args,
        env,
        env_remove,
        cwd,
        framing: Framing::JsonRpc,
        secret_env,
        run_as: None,
    }
}

/// Why the CLI stopped, if its error says it doesn't take
/// `NO_PROJECT_CONFIGS`. The agent stays stopped, since a start without the
/// flag would let the repository decide what runs unasked.
pub(super) fn refused_flag(stderr: &str) -> Option<String> {
    let refused = stderr.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.contains(NO_PROJECT_CONFIGS)
            && ["unknown", "unrecognized", "unexpected"]
                .iter()
                .any(|word| line.contains(word))
    });
    refused.then(|| {
        format!(
            "This Cursor CLI doesn't take {NO_PROJECT_CONFIGS}, which stops a repository \
             changing what runs without asking, so MaxHQ won't start it. Update the Cursor \
             CLI. If it's already current, update MaxHQ."
        )
    })
}

/// Finds the CLI on `PATH`, leaving out relative folders, then where the
/// installer puts it.
fn find(path: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = path
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|dir| dir.is_absolute())
        .collect();
    if let Some(home) = home {
        dirs.extend(HOME_DIRS.iter().map(|dir| home.join(dir)));
    }
    dirs.extend(SYSTEM_DIRS.iter().map(PathBuf::from));
    NAMES
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)))
        .find(|candidate| runnable(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::PlaceHost;
    use maxhq_protocol::ids::{AccountId, DeviceId, WorkspaceId};
    use maxhq_protocol::Secret;

    #[test]
    fn a_relative_folder_on_path_is_never_the_cli() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testing::TempDir::new();
        for name in NAMES {
            let program = dir.path().join(name);
            std::fs::write(&program, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let relative = crate::testing::from_here(dir.path());
        let found = find(Some(relative.as_os_str()), None);
        assert!(!found.is_some_and(|program| program.is_relative()));
    }

    fn login(auth: LoginAuth) -> Login {
        Login {
            account: AccountId::derive("cursor"),
            config_dir: PathBuf::from("/home/me/.cursor"),
            auth,
        }
    }

    fn place() -> Place {
        Place {
            device: DeviceId::derive("mac"),
            workspace: WorkspaceId::derive("repo"),
            cwd: PathBuf::from("/work/repo"),
            attachments: PathBuf::from("/data/attachments/agent"),
            host: PlaceHost::Local,
            dispatcher: false,
            trust: None,
        }
    }

    fn host() -> Host {
        Host {
            program: PathBuf::from("/home/me/.local/bin/cursor-agent"),
            path: Some("/usr/bin:/bin".into()),
            home: Some(PathBuf::from("/home/me")),
        }
    }

    fn strings(list: &[OsString]) -> Vec<&str> {
        list.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn ask_keeps_the_sandbox_and_ends_with_acp() {
        let launch = command(
            &host(),
            &login(LoginAuth::ConfigDir),
            &place(),
            PermissionMode::Ask,
        );
        assert_eq!(
            strings(&launch.args),
            [
                "--disable-project-configs",
                "--trust",
                "--sandbox",
                "enabled",
                "acp"
            ]
        );
        assert_eq!(launch.cwd, PathBuf::from("/work/repo"));
        assert_eq!(launch.framing, Framing::JsonRpc);
        assert!(launch
            .env
            .contains(&("PATH".into(), "/home/me/.local/bin:/usr/bin:/bin".into())));
    }

    #[test]
    fn full_access_turns_the_sandbox_off_and_plan_starts_in_plan_mode() {
        let full = command(
            &host(),
            &login(LoginAuth::ConfigDir),
            &place(),
            PermissionMode::FullAccess,
        );
        assert_eq!(
            strings(&full.args),
            [
                "--disable-project-configs",
                "--trust",
                "--sandbox",
                "disabled",
                "--force",
                "acp"
            ]
        );
        let plan = command(
            &host(),
            &login(LoginAuth::ConfigDir),
            &place(),
            PermissionMode::Plan,
        );
        assert_eq!(
            strings(&plan.args),
            [
                "--disable-project-configs",
                "--trust",
                "--sandbox",
                "enabled",
                "--mode",
                "plan",
                "acp"
            ]
        );
    }

    #[test]
    fn a_cli_that_refuses_the_project_config_flag_is_named_for_it() {
        let reason = refused_flag("error: unknown option '--disable-project-configs'\n");
        assert!(reason.is_some_and(|reason| reason.contains("won't start")));
        // A crash that only logs its arguments stays an ordinary crash.
        assert_eq!(
            refused_flag("boom\nargv: --disable-project-configs\n"),
            None
        );
    }

    #[test]
    fn a_cli_login_has_keys_and_tokens_scrubbed() {
        let launch = command(
            &host(),
            &login(LoginAuth::ConfigDir),
            &place(),
            PermissionMode::Ask,
        );
        let removed = strings(&launch.env_remove);
        for name in ["NODE_OPTIONS", "CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        assert!(launch.secret_env.is_empty());
    }

    #[test]
    fn the_login_prints_its_link_with_keys_and_tokens_scrubbed() {
        let launch = login_command(&host(), &login(LoginAuth::ConfigDir));
        assert_eq!(launch.program, host().program);
        assert_eq!(strings(&launch.args), ["login"]);
        assert_eq!(launch.cwd, PathBuf::from("/home/me"));
        for (name, value) in [
            ("NO_OPEN_BROWSER", "1"),
            ("NO_COLOR", "1"),
            ("PATH", "/home/me/.local/bin:/usr/bin:/bin"),
        ] {
            assert!(
                launch.env.contains(&(name.into(), value.into())),
                "{name} should be {value}"
            );
        }
        let removed = strings(&launch.env_remove);
        for name in ["NODE_OPTIONS", "CURSOR_API_KEY", "CURSOR_AUTH_TOKEN"] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        assert!(launch.secret_env.is_empty());

        // With no home folder it runs in the login's folder.
        let homeless = Host {
            home: None,
            ..host()
        };
        let launch = login_command(&homeless, &login(LoginAuth::ConfigDir));
        assert_eq!(launch.cwd, PathBuf::from("/home/me/.cursor"));
    }

    #[test]
    fn an_api_key_goes_only_in_the_secret_variables() {
        let key = Secret::new("key_123".to_owned());
        let launch = command(
            &host(),
            &login(LoginAuth::CursorApiKey(key.clone())),
            &place(),
            PermissionMode::Ask,
        );
        assert_eq!(launch.secret_env, vec![(OsString::from(API_KEY), key)]);
        assert!(!strings(&launch.env_remove).contains(&API_KEY));
        assert!(strings(&launch.env_remove).contains(&"CURSOR_AUTH_TOKEN"));
        assert!(!launch.env.iter().any(|(name, _)| name == API_KEY));
        assert!(!strings(&launch.args)
            .iter()
            .any(|arg| arg.contains("key_123")));
    }
}
