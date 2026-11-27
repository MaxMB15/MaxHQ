// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The command line that starts a held Claude Code session, and the one
//! that signs its login in again.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use maxhq_core::tools::dispatch;
use maxhq_protocol::model::PermissionMode;
use uuid::Uuid;

use super::artifacts;
use super::entrypoint;
use super::wire::mode_name;
use crate::drive::{Framing, Launch, Login, LoginAuth, Place, PlaceHost, ServerLogin};

/// Where installers put `claude` under the home folder, tried after `PATH`.
const HOME_PLACES: &[&str] = &[
    ".local/bin/claude",
    ".claude/local/claude",
    ".bun/bin/claude",
    ".npm-global/bin/claude",
    ".nix-profile/bin/claude",
];

/// System-wide places for `claude`, tried last.
const SYSTEM_PLACES: &[&str] = &[
    "/opt/homebrew/bin/claude",
    "/usr/local/bin/claude",
    "/run/current-system/sw/bin/claude",
];

/// Variables the CLI must not inherit from `maxd`. `NODE_OPTIONS` could
/// load code into it, and the `CLAUDE` and `ANTHROPIC` families could pick
/// another account or an API key, or name another app as the one running
/// the CLI. MaxHQ sets that name itself.
pub const STRIPPED_PREFIXES: &[&str] = &["CLAUDE", "ANTHROPIC"];

/// The variable a setup-token reaches the CLI in, per W-5.
pub const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The settings the dispatcher's session adds to the login's own. Its file
/// tools write only inside its working folders, and its commands run in
/// Claude Code's sandbox, which writes only there too. A command that
/// wants out of the sandbox goes to auto mode's reviewer first.
const DISPATCHER_SETTINGS: &str =
    r#"{"permissions":{"blockWritesOutsideWorkingDirectories":true},"sandbox":{"enabled":true}}"#;

/// The variable a model server's token reaches a local agent's CLI in.
pub const SERVER_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";

/// The Claude Code built-in tools a local agent goes without while Fewer
/// tools is on, so a small model has less to read. The tools for claude.ai
/// pages, `artifacts::TOOLS`, are among them.
const LEAN_DENIED: &str =
    "Workflow,ScheduleWakeup,ReportFindings,Artifact,ArtifactComments,ArtifactData,ArtifactCheck";

/// The command line for a Claude session in `place` under `login`.
/// `resume` continues the session with ID `session`, and otherwise starts
/// it with that ID. Full access is always reachable later, so a running
/// agent can switch to it without a restart.
pub fn launch(
    login: &Login,
    place: &Place,
    session: Uuid,
    resume: bool,
    model: Option<&str>,
    mode: PermissionMode,
) -> Launch {
    let entrypoint = match place.host {
        PlaceHost::Local => entrypoint::label(&login.config_dir),
        // The guest's settings are in the guest, out of reach here.
        PlaceHost::Guest { .. } => entrypoint::MAXHQ.to_owned(),
    };
    command(
        &Host::here(entrypoint),
        login,
        place,
        session,
        resume,
        model,
        mode,
    )
}

/// Adds the effort level the CLI's turns run at, unless it's none, which
/// leaves the CLI to its own default.
pub fn add_effort(launch: &mut Launch, effort: Option<&str>) {
    if let Some(effort) = effort {
        launch.args.push("--effort".into());
        launch.args.push(effort.into());
    }
}

/// The command line for `claude auth login` on this device, under the
/// variables a session of `login` gets, so the login lands in its folder.
pub fn login_launch(login: &Login) -> Launch {
    let host = Host::here(entrypoint::label(&login.config_dir));
    login_command(&host, login)
}

/// What `launch` reads from `maxd`'s own process, kept apart so tests can
/// build a command line without it.
pub(super) struct Host {
    /// The `claude` to run.
    pub program: PathBuf,
    /// `maxd`'s home folder.
    pub home: Option<PathBuf>,
    /// `maxd`'s `PATH`.
    pub path: Option<OsString>,
    /// The names of the variables `maxd` runs with.
    pub inherited: Vec<OsString>,
    /// The name the CLI gives for the app running it.
    pub entrypoint: String,
}

impl Host {
    /// What `maxd`'s own process gives, with `entrypoint` as the name the
    /// CLI gives for the app running it.
    fn here(entrypoint: String) -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        let path = env::var_os("PATH");
        let user = env::var_os("USER");
        let program = find(path.as_deref(), home.as_deref(), user.as_deref())
            .unwrap_or_else(|| PathBuf::from("claude"));
        Self {
            program,
            home,
            path,
            inherited: env::vars_os().map(|(name, _)| name).collect(),
            entrypoint,
        }
    }
}

/// Builds the command line from `host` without reading anything else.
pub(super) fn command(
    host: &Host,
    login: &Login,
    place: &Place,
    session: Uuid,
    resume: bool,
    model: Option<&str>,
    mode: PermissionMode,
) -> Launch {
    let mut args: Vec<OsString> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(if resume { "--resume" } else { "--session-id" }.into());
    args.push(session.to_string().into());
    // The dispatcher runs in auto mode whatever its mode says, and never
    // gets full access.
    let mode = if place.dispatcher {
        PermissionMode::Auto
    } else {
        mode
    };
    args.push("--permission-mode".into());
    args.push(mode_name(mode).into());
    if !place.dispatcher {
        args.push("--allow-dangerously-skip-permissions".into());
    }
    // A local agent runs the model its server serves: the one asked for,
    // else the server's own pick.
    let model = match &login.auth {
        LoginAuth::ModelServer(server) => model.or(server.model.as_deref()),
        _ => model,
    };
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    if place.dispatcher {
        // It reads anywhere and runs what the reviewer allows, but writes
        // only in its own folder and the attachments one.
        args.push("--settings".into());
        args.push(DISPATCHER_SETTINGS.into());
        args.push("--append-system-prompt".into());
        args.push(dispatch::instructions().into());
    }
    // A lean local agent goes without Fewer tools' list, and any other
    // without the claude.ai page tools while the device turned them off.
    let lean =
        !place.dispatcher && matches!(&login.auth, LoginAuth::ModelServer(server) if server.lean);
    let denied = if lean {
        Some(LEAN_DENIED)
    } else {
        (!place.claude_artifacts).then_some(artifacts::TOOLS)
    };
    if let Some(denied) = denied {
        args.push("--disallowedTools".into());
        args.push(denied.into());
    }
    args.push("--add-dir".into());
    args.push(place.attachments.clone().into());
    under(host, login, args, place.cwd.clone(), model)
}

/// Builds `claude auth login` from `host`, run in the login's folder.
pub(super) fn login_command(host: &Host, login: &Login) -> Launch {
    let args = vec!["auth".into(), "login".into()];
    under(host, login, args, login.config_dir.clone(), None)
}

/// The output cap a local agent's turns run with. A small context can't
/// spare Claude Code's own 32,000 for one answer.
const LOCAL_OUTPUT_TOKENS: u32 = 4096;

/// The most a cloud agent writes in one answer: Claude Code's own default,
/// or the model's limit when that's lower. A hosted model's context has
/// room for it, and a longer answer seldom helps a coding turn.
const CLOUD_OUTPUT_TOKENS: u32 = 32_000;

/// The output cap of an agent on `server` running `model`.
fn output_tokens(server: &ServerLogin, model: Option<&str>) -> u32 {
    if !server.cloud {
        return LOCAL_OUTPUT_TOKENS;
    }
    model
        .and_then(|model| server.max_outputs.get(model))
        .map_or(CLOUD_OUTPUT_TOKENS, |limit| {
            (*limit).min(CLOUD_OUTPUT_TOKENS)
        })
}

/// The variables that point Claude Code at a model server instead of
/// Anthropic, with `model` for every model it would pick on its own, and
/// the server's context size so it compacts before the server overflows.
fn server_env(server: &ServerLogin, model: Option<&str>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![
        ("ANTHROPIC_BASE_URL".into(), server.base_url.clone().into()),
        // An empty key, so no key from anywhere wins over the token.
        ("ANTHROPIC_API_KEY".into(), OsString::new()),
        ("CLAUDE_CODE_ATTRIBUTION_HEADER".into(), "0".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        (
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            output_tokens(server, model).to_string().into(),
        ),
    ];
    if let Some(context) = server.context {
        env.push((
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),
            context.to_string().into(),
        ));
    }
    if let Some(model) = model {
        for name in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            env.push((name.into(), model.into()));
        }
    }
    env
}

/// Whether an agent's CLI that started on `then`, for an agent running
/// `model` (or none, for the server's pick), sees the server as `now`
/// would give it: the same variables, token and tools. A CLI that doesn't
/// still runs with the old ones, such as a key you replaced or an output
/// cap from before the server counted as cloud, until it starts again.
pub fn same_server(then: &ServerLogin, now: &ServerLogin, model: Option<&str>) -> bool {
    then.token == now.token
        && then.lean == now.lean
        && server_env(then, model.or(then.model.as_deref()))
            == server_env(now, model.or(now.model.as_deref()))
}

/// `host`'s `claude` with `args` in `cwd`, under `login`'s folder and
/// with the variables MaxHQ sets and scrubs. `model` is the model a local
/// agent runs.
fn under(
    host: &Host,
    login: &Login,
    args: Vec<OsString>,
    cwd: PathBuf,
    model: Option<&str>,
) -> Launch {
    let mut env: Vec<(OsString, OsString)> = Vec::new();
    let default_dir = host.home.as_ref().map(|home| home.join(".claude"));
    if default_dir.as_deref() != Some(login.config_dir.as_path()) {
        env.push(("CLAUDE_CONFIG_DIR".into(), login.config_dir.clone().into()));
    }
    env.push((entrypoint::VAR.into(), host.entrypoint.clone().into()));
    env.push(("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS".into(), "1".into()));
    env.push(("CLAUDE_CODE_STARTUP_FAILURE_RESULTS".into(), "1".into()));
    if let Some(dir) = host.program.parent().filter(|dir| dir.is_absolute()) {
        let mut path = OsString::from(dir);
        if let Some(rest) = host.path.as_deref().filter(|rest| !rest.is_empty()) {
            path.push(":");
            path.push(rest);
        }
        env.push(("PATH".into(), path));
    }

    // A setup-token login gets its token on the holder's pipe, and only
    // then is the token's variable kept out of the scrub. Any other login
    // has it scrubbed even when `maxd` doesn't carry it.
    let mut secret_env = Vec::new();
    let mut env_remove: Vec<OsString> = vec!["NODE_OPTIONS".into()];
    match &login.auth {
        LoginAuth::SetupToken(token) => secret_env.push((OAUTH_TOKEN.into(), token.clone())),
        LoginAuth::ModelServer(server) => {
            env.extend(server_env(server, model));
            secret_env.push((SERVER_TOKEN.into(), server.token.clone()));
            env_remove.push(OAUTH_TOKEN.into());
        }
        _ => env_remove.push(OAUTH_TOKEN.into()),
    }
    for name in &host.inherited {
        let stripped = name
            .to_str()
            .is_some_and(|name| STRIPPED_PREFIXES.iter().any(|p| name.starts_with(p)));
        let set_here =
            env.iter().any(|(set, _)| set == name) || secret_env.iter().any(|(set, _)| set == name);
        if stripped && !set_here && !env_remove.contains(name) {
            env_remove.push(name.clone());
        }
    }

    Launch {
        program: host.program.clone(),
        args,
        env,
        env_remove,
        cwd,
        framing: Framing::ClaudeStreamJson,
        secret_env,
        run_as: None,
    }
}

/// Finds `claude` on `PATH`, then where installers put it. The same search
/// as the usage reader's.
fn find(path: Option<&OsStr>, home: Option<&Path>, user: Option<&OsStr>) -> Option<PathBuf> {
    let on_path = path
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("claude"));
    let in_home = home
        .into_iter()
        .flat_map(|home| HOME_PLACES.iter().map(move |place| home.join(place)));
    let per_user = user.map(|user| {
        let mut path = OsString::from("/etc/profiles/per-user/");
        path.push(user);
        path.push("/bin/claude");
        PathBuf::from(path)
    });
    let system = SYSTEM_PLACES.iter().map(PathBuf::from);
    on_path
        .chain(in_home)
        .chain(per_user)
        .chain(system)
        .find(|candidate| runnable(candidate))
}

/// Whether `path` is a file someone may run.
fn runnable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::{AccountId, DeviceId, WorkspaceId};
    use maxhq_protocol::Secret;
    use std::collections::BTreeMap;

    #[test]
    fn a_relative_folder_on_path_is_never_claude() {
        let dir = crate::testing::TempDir::new();
        let program = dir.path().join("claude");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let relative = crate::testing::from_here(dir.path());
        let found = find(Some(relative.as_os_str()), None, None);
        assert!(!found.is_some_and(|program| program.is_relative()));
    }

    fn login(dir: &str) -> Login {
        Login {
            account: AccountId::derive("claude"),
            config_dir: PathBuf::from(dir),
            auth: LoginAuth::ConfigDir,
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
            claude_artifacts: true,
        }
    }

    fn host() -> Host {
        Host {
            program: PathBuf::from("/home/me/.local/bin/claude"),
            home: Some(PathBuf::from("/home/me")),
            path: Some("/usr/bin:/bin".into()),
            inherited: vec![
                "HOME".into(),
                "CLAUDECODE".into(),
                "CLAUDE_CONFIG_DIR".into(),
                "CLAUDE_CODE_ENTRYPOINT".into(),
                "ANTHROPIC_API_KEY".into(),
                "NODE_OPTIONS".into(),
            ],
            entrypoint: entrypoint::MAXHQ.into(),
        }
    }

    /// The value `launch` sets for `name`, if it sets one.
    fn env_value<'a>(launch: &'a Launch, name: &str) -> Option<&'a str> {
        launch
            .env
            .iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.to_str().unwrap())
    }

    fn strings(list: &[OsString]) -> Vec<&str> {
        list.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn an_effort_level_goes_on_the_command_line() {
        let launch = || {
            command(
                &host(),
                &login("/home/me/.claude"),
                &place(),
                Uuid::from_u128(7),
                false,
                None,
                PermissionMode::Ask,
            )
        };
        let mut set = launch();
        let before = set.args.len();
        add_effort(&mut set, Some("xhigh"));
        assert_eq!(strings(&set.args[before..]), ["--effort", "xhigh"]);
        // None leaves the CLI at its own default.
        let mut plain = launch();
        add_effort(&mut plain, None);
        assert_eq!(plain.args, launch().args);
    }

    #[test]
    fn starts_a_new_session_with_its_id() {
        let session = Uuid::from_u128(7);
        let launch = command(
            &host(),
            &login("/home/me/.claude"),
            &place(),
            session,
            false,
            Some("haiku"),
            PermissionMode::AcceptEdits,
        );
        assert_eq!(launch.program, PathBuf::from("/home/me/.local/bin/claude"));
        assert_eq!(
            strings(&launch.args),
            [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-prompt-tool",
                "stdio",
                "--session-id",
                &session.to_string(),
                "--permission-mode",
                "acceptEdits",
                "--allow-dangerously-skip-permissions",
                "--model",
                "haiku",
                "--add-dir",
                "/data/attachments/agent",
            ]
        );
        assert_eq!(launch.cwd, PathBuf::from("/work/repo"));
        assert_eq!(launch.framing, Framing::ClaudeStreamJson);
    }

    #[test]
    fn the_dispatcher_runs_in_auto_and_writes_only_its_folders() {
        let place = Place {
            dispatcher: true,
            ..place()
        };
        let launch = command(
            &host(),
            &login("/home/me/.claude"),
            &place,
            Uuid::from_u128(7),
            false,
            None,
            PermissionMode::FullAccess,
        );
        let args = strings(&launch.args);
        let after = |flag: &str| {
            let at = args.iter().position(|arg| *arg == flag).unwrap();
            args[at + 1]
        };
        // Auto mode's reviewer judges its commands, whatever mode it had.
        assert_eq!(after("--permission-mode"), "auto");
        assert!(!args.contains(&"--disallowedTools"));
        let settings: serde_json::Value = serde_json::from_str(after("--settings")).unwrap();
        assert_eq!(
            settings["permissions"]["blockWritesOutsideWorkingDirectories"],
            true
        );
        assert_eq!(settings["sandbox"]["enabled"], true);
        // Its working folders are its own and the attachments one.
        assert_eq!(launch.cwd, PathBuf::from("/work/repo"));
        assert_eq!(after("--add-dir"), "/data/attachments/agent");
        let prompt = args.iter().position(|arg| *arg == "--append-system-prompt");
        assert_eq!(args[prompt.unwrap() + 1], dispatch::instructions());
        assert!(!args.contains(&"--allow-dangerously-skip-permissions"));
    }

    #[test]
    fn resumes_and_passes_the_mode_again() {
        let launch = command(
            &host(),
            &login("/home/me/.claude"),
            &place(),
            Uuid::from_u128(7),
            true,
            None,
            PermissionMode::FullAccess,
        );
        let args = strings(&launch.args);
        assert!(args.contains(&"--resume"));
        assert!(!args.contains(&"--session-id"));
        assert!(!args.contains(&"--model"));
        let mode = args
            .iter()
            .position(|arg| *arg == "--permission-mode")
            .unwrap();
        assert_eq!(args[mode + 1], "bypassPermissions");
        assert!(args.contains(&"--add-dir"));
    }

    #[test]
    fn names_the_config_dir_only_when_it_isnt_the_default() {
        let default = command(
            &host(),
            &login("/home/me/.claude"),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        assert!(!default
            .env
            .iter()
            .any(|(name, _)| name == "CLAUDE_CONFIG_DIR"));
        assert!(default
            .env_remove
            .iter()
            .any(|name| name == "CLAUDE_CONFIG_DIR"));

        let other = command(
            &host(),
            &login("/home/me/.claude-work"),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        assert!(other
            .env
            .contains(&("CLAUDE_CONFIG_DIR".into(), "/home/me/.claude-work".into())));
        assert!(!other
            .env_remove
            .iter()
            .any(|name| name == "CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn strips_inherited_logins_and_names_maxhq_as_the_entrypoint() {
        let launch = command(
            &host(),
            &login("/home/me/.claude"),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        let removed = strings(&launch.env_remove);
        for name in ["NODE_OPTIONS", "CLAUDECODE", "ANTHROPIC_API_KEY"] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        assert!(!removed.contains(&"HOME"));
        assert_eq!(
            removed
                .iter()
                .filter(|name| **name == "NODE_OPTIONS")
                .count(),
            1
        );
        assert!(launch
            .env
            .contains(&("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS".into(), "1".into())));
        assert!(launch
            .env
            .contains(&("CLAUDE_CODE_STARTUP_FAILURE_RESULTS".into(), "1".into())));
        // An inherited name, such as `claude-desktop`, gives way to
        // MaxHQ's own, which the holder sets over whatever `maxd` has.
        assert!(!removed.contains(&"CLAUDE_CODE_ENTRYPOINT"));
        assert_eq!(env_value(&launch, "CLAUDE_CODE_ENTRYPOINT"), Some("maxhq"));
        assert_eq!(
            launch
                .env
                .iter()
                .filter(|(name, _)| name == "CLAUDE_CODE_ENTRYPOINT")
                .count(),
            1
        );
    }

    #[test]
    fn a_local_login_s_settings_name_it_and_a_guest_s_launch_says_maxhq() {
        let dir = crate::testing::TempDir::new();
        fs::write(
            dir.path().join("settings.json"),
            r#"{"env": {"CLAUDE_CODE_ENTRYPOINT": "custom", "OTHER_KEY": "placeholder"}}"#,
        )
        .unwrap();
        let login = Login {
            config_dir: dir.path().to_owned(),
            ..login("/unused")
        };
        let run = |place: &Place| {
            launch(
                &login,
                place,
                Uuid::from_u128(1),
                false,
                None,
                PermissionMode::Ask,
            )
        };

        let local = run(&place());
        assert_eq!(env_value(&local, "CLAUDE_CODE_ENTRYPOINT"), Some("custom"));
        assert!(!format!("{local:?}").contains("OTHER_KEY"));

        // This device's file isn't the guest's, so a guest is never named
        // from it.
        let guest = Place {
            host: PlaceHost::Guest {
                workspace: WorkspaceId::derive("box"),
            },
            ..place()
        };
        assert_eq!(
            env_value(&run(&guest), "CLAUDE_CODE_ENTRYPOINT"),
            Some("maxhq")
        );
    }

    #[test]
    fn puts_the_program_folder_first_on_path() {
        let launch = command(
            &host(),
            &login("/home/me/.claude"),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        assert!(launch
            .env
            .contains(&("PATH".into(), "/home/me/.local/bin:/usr/bin:/bin".into())));
    }

    /// A token account's login, as the secret store serves it.
    fn token_login() -> Login {
        Login {
            account: AccountId::derive("token"),
            config_dir: PathBuf::from("/home/agent/.claude-token"),
            auth: LoginAuth::SetupToken(Secret::new("sk-ant-oat01-test-not-a-real-token".into())),
        }
    }

    #[test]
    fn hands_a_setup_token_over_as_a_secret() {
        let mut host = host();
        host.inherited.push(OAUTH_TOKEN.into());
        let launch = command(
            &host,
            &token_login(),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        let [(name, token)] = &launch.secret_env[..] else {
            panic!("one secret variable");
        };
        assert_eq!(name, OAUTH_TOKEN);
        assert_eq!(token.expose(), "sk-ant-oat01-test-not-a-real-token");

        let removed = strings(&launch.env_remove);
        assert!(!removed.contains(&OAUTH_TOKEN));
        // An API key would win over the token, so it still goes.
        assert!(removed.contains(&"ANTHROPIC_API_KEY"));
        assert!(!launch.env.iter().any(|(name, _)| name == OAUTH_TOKEN));
        assert!(launch.env.contains(&(
            "CLAUDE_CONFIG_DIR".into(),
            "/home/agent/.claude-token".into()
        )));
        assert!(!format!("{launch:?}").contains("test-not-a-real-token"));
    }

    #[test]
    fn scrubs_the_token_variable_for_any_other_login() {
        for inherited in [false, true] {
            let mut host = host();
            if inherited {
                host.inherited.push(OAUTH_TOKEN.into());
            }
            let launch = command(
                &host,
                &login("/home/me/.claude"),
                &place(),
                Uuid::from_u128(1),
                false,
                None,
                PermissionMode::Ask,
            );
            assert!(launch.secret_env.is_empty());
            let removed = strings(&launch.env_remove);
            assert_eq!(
                removed.iter().filter(|name| **name == OAUTH_TOKEN).count(),
                1
            );
        }
    }

    #[test]
    fn signs_in_again_to_the_sessions_folder_with_its_variables() {
        let other = login_command(&host(), &login("/home/me/.claude-work"));
        assert_eq!(other.program, PathBuf::from("/home/me/.local/bin/claude"));
        assert_eq!(strings(&other.args), ["auth", "login"]);
        assert_eq!(other.cwd, PathBuf::from("/home/me/.claude-work"));
        assert_eq!(
            env_value(&other, "CLAUDE_CONFIG_DIR"),
            Some("/home/me/.claude-work")
        );
        assert_eq!(
            env_value(&other, "PATH"),
            Some("/home/me/.local/bin:/usr/bin:/bin")
        );
        let removed = strings(&other.env_remove);
        for name in [
            "NODE_OPTIONS",
            "CLAUDECODE",
            "ANTHROPIC_API_KEY",
            OAUTH_TOKEN,
        ] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        assert!(other.secret_env.is_empty());

        let default = login_command(&host(), &login("/home/me/.claude"));
        assert_eq!(env_value(&default, "CLAUDE_CONFIG_DIR"), None);
        assert!(strings(&default.env_remove).contains(&"CLAUDE_CONFIG_DIR"));
    }

    /// A local agent's login, pointed at a server on this device.
    fn server_login(model: Option<&str>, context: Option<u32>) -> Login {
        Login {
            account: AccountId::derive("local"),
            config_dir: PathBuf::from("/data/maxd/local-claude/server"),
            auth: LoginAuth::ModelServer(std::sync::Arc::new(ServerLogin {
                base_url: "http://127.0.0.1:41234".into(),
                token: Secret::new("test-server-key".into()),
                context,
                model: model.map(str::to_owned),
                lean: true,
                cloud: false,
                max_outputs: BTreeMap::from([("qwen3.6-35b-a3b@ud-q4_k_xl".to_owned(), 65_536)]),
            })),
        }
    }

    /// The output cap a launch under `login` running `model` sets.
    fn output_cap(login: &Login, model: Option<&str>) -> Option<String> {
        let launch = command(
            &host(),
            login,
            &place(),
            Uuid::from_u128(1),
            false,
            model,
            PermissionMode::Ask,
        );
        env_value(&launch, "CLAUDE_CODE_MAX_OUTPUT_TOKENS").map(str::to_owned)
    }

    #[test]
    fn a_cloud_agent_writes_up_to_its_model_s_limit_capped_at_claude_code_s_own() {
        let local = server_login(Some("qwen3.6-35b-a3b@ud-q4_k_xl"), Some(65_536));
        // Hardware you run keeps the small cap, whatever the model says.
        assert_eq!(output_cap(&local, None).as_deref(), Some("4096"));
        let LoginAuth::ModelServer(server) = &local.auth else {
            unreachable!()
        };
        let cloud = Login {
            auth: LoginAuth::ModelServer(std::sync::Arc::new(ServerLogin {
                base_url: "https://openrouter.ai/api".into(),
                cloud: true,
                lean: false,
                model: Some("z-ai/glm-5.3-flash".into()),
                max_outputs: BTreeMap::from([
                    ("z-ai/glm-5.3-flash".to_owned(), 131_072),
                    ("small".to_owned(), 8_192),
                ]),
                ..ServerLogin::clone(server)
            })),
            ..local.clone()
        };
        // A limit above Claude Code's own default is capped there.
        assert_eq!(output_cap(&cloud, None).as_deref(), Some("32000"));
        // The model the agent runs counts, not the server's pick.
        assert_eq!(output_cap(&cloud, Some("small")).as_deref(), Some("8192"));
        // A model that says nothing gets the default.
        assert_eq!(output_cap(&cloud, Some("quiet")).as_deref(), Some("32000"));
        assert_eq!(CLOUD_OUTPUT_TOKENS, 32_000);
    }

    #[test]
    fn a_cli_runs_on_the_same_server_only_while_what_it_was_given_holds() {
        let local = server_login(Some("qwen3.6-35b-a3b@ud-q4_k_xl"), Some(65_536));
        let LoginAuth::ModelServer(then) = &local.auth else {
            unreachable!()
        };
        let with = |change: &dyn Fn(&mut ServerLogin)| {
            let mut now = ServerLogin::clone(then);
            change(&mut now);
            now
        };
        assert!(same_server(then, then, None));
        // What the CLI never sees doesn't count: another model's limit.
        let other_limit = with(&|now| {
            now.max_outputs.insert("other".into(), 1024);
        });
        assert!(same_server(then, &other_limit, None));
        // A new key, as a sign-in gives, does.
        let new_key = with(&|now| now.token = Secret::new("new-key".into()));
        assert!(!same_server(then, &new_key, None));
        // So does turning cloud, which lifts the output cap.
        let cloud = with(&|now| now.cloud = true);
        assert!(!same_server(then, &cloud, None));
        // And Fewer tools, a context size or the server's pick of model.
        assert!(!same_server(then, &with(&|now| now.lean = false), None));
        assert!(!same_server(
            then,
            &with(&|now| now.context = Some(32_768)),
            None
        ));
        let picked = with(&|now| now.model = Some("other".into()));
        assert!(!same_server(then, &picked, None));
        // An agent with a model of its own doesn't run the server's pick.
        assert!(same_server(then, &picked, Some("mine")));
    }

    #[test]
    fn a_local_agent_talks_to_its_model_server_and_nothing_else() {
        let mut host = host();
        for name in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_SMALL_FAST_MODEL",
            OAUTH_TOKEN,
        ] {
            host.inherited.push(name.into());
        }
        let launch = command(
            &host,
            &server_login(Some("qwen3.6-35b-a3b@ud-q4_k_xl"), Some(65_536)),
            &place(),
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );

        assert_eq!(
            env_value(&launch, "ANTHROPIC_BASE_URL"),
            Some("http://127.0.0.1:41234")
        );
        assert_eq!(env_value(&launch, "ANTHROPIC_API_KEY"), Some(""));
        for name in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            assert_eq!(
                env_value(&launch, name),
                Some("qwen3.6-35b-a3b@ud-q4_k_xl"),
                "{name}"
            );
        }
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_ATTRIBUTION_HEADER"),
            Some("0")
        );
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
            Some("1")
        );
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("65536")
        );
        assert_eq!(
            env_value(&launch, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"),
            Some("4096")
        );
        let args = strings(&launch.args);
        let model = args.iter().position(|arg| *arg == "--model").unwrap();
        assert_eq!(args[model + 1], "qwen3.6-35b-a3b@ud-q4_k_xl");

        // The key goes on the holder's pipe, never in the plain variables.
        let [(name, token)] = &launch.secret_env[..] else {
            panic!("one secret variable");
        };
        assert_eq!(name, SERVER_TOKEN);
        assert_eq!(token.expose(), "test-server-key");
        assert!(!format!("{launch:?}").contains("test-server-key"));

        // Whatever `maxd` inherited is gone, unless set here.
        let removed = strings(&launch.env_remove);
        for name in [
            "ANTHROPIC_SMALL_FAST_MODEL",
            "CLAUDECODE",
            "NODE_OPTIONS",
            OAUTH_TOKEN,
        ] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        for name in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_API_KEY",
            SERVER_TOKEN,
        ] {
            assert!(!removed.contains(&name), "{name} is set, not removed");
        }

        // Its own folder, never the user's.
        assert_eq!(
            env_value(&launch, "CLAUDE_CONFIG_DIR"),
            Some("/data/maxd/local-claude/server")
        );
        assert!(!removed.contains(&"CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn a_local_agent_s_chosen_model_wins_over_the_server_s_pick() {
        let launch = command(
            &host(),
            &server_login(Some("default@q4"), None),
            &place(),
            Uuid::from_u128(1),
            true,
            Some("other@q8_0"),
            PermissionMode::Ask,
        );
        assert_eq!(env_value(&launch, "ANTHROPIC_MODEL"), Some("other@q8_0"));
        // A server that reported no context leaves Claude Code its own.
        assert_eq!(env_value(&launch, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), None);
    }

    /// What a launch under `login` in `place` turns off with the CLI's
    /// disallow flag, which it passes once at most.
    fn denied(login: &Login, place: &Place) -> Option<String> {
        let launch = command(
            &host(),
            login,
            place,
            Uuid::from_u128(1),
            false,
            None,
            PermissionMode::Ask,
        );
        let args = strings(&launch.args);
        let flags = args.iter().filter(|arg| **arg == "--disallowedTools");
        assert!(flags.count() <= 1, "{args:?}");
        let at = args.iter().position(|arg| *arg == "--disallowedTools")?;
        Some(args[at + 1].to_owned())
    }

    #[test]
    fn fewer_tools_turns_off_the_cli_s_heavy_built_in_tools() {
        let lean = server_login(None, None);
        assert_eq!(
            denied(&lean, &place()).as_deref(),
            Some(
                "Workflow,ScheduleWakeup,ReportFindings,\
                 Artifact,ArtifactComments,ArtifactData,ArtifactCheck"
            )
        );

        let LoginAuth::ModelServer(server) = &lean.auth else {
            unreachable!()
        };
        let mut full = ServerLogin::clone(server);
        full.lean = false;
        let full = Login {
            auth: LoginAuth::ModelServer(std::sync::Arc::new(full)),
            ..lean.clone()
        };
        assert_eq!(denied(&full, &place()), None);

        // An account's own sign-in keeps every tool.
        assert_eq!(denied(&login("/home/me/.claude"), &place()), None);
    }

    #[test]
    fn the_device_switch_turns_off_the_claude_ai_page_tools() {
        let pages = Some("Artifact,ArtifactComments,ArtifactData,ArtifactCheck");
        let on = place();
        let off = Place {
            claude_artifacts: false,
            ..place()
        };
        let account = login("/home/me/.claude");
        assert_eq!(denied(&account, &on), None);
        assert_eq!(denied(&account, &off).as_deref(), pages);

        // The dispatcher goes without them too.
        let dispatcher = |claude_artifacts| Place {
            dispatcher: true,
            claude_artifacts,
            ..place()
        };
        assert_eq!(denied(&account, &dispatcher(true)), None);
        assert_eq!(denied(&account, &dispatcher(false)).as_deref(), pages);

        // A lean local agent goes without them either way, in one list.
        let lean = server_login(None, None);
        assert_eq!(denied(&lean, &on).as_deref(), Some(LEAN_DENIED));
        assert_eq!(denied(&lean, &off).as_deref(), Some(LEAN_DENIED));
        for tool in artifacts::TOOLS.split(',') {
            assert!(LEAN_DENIED.split(',').any(|lean| lean == tool), "{tool}");
        }
    }
}
