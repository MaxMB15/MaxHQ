// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The command line that starts a held local ACP command: its program,
//! found on `PATH` or used as given, and its arguments as given, with no
//! shell. Its variables' values go only in the holder's secret variables,
//! and every vendor credential `maxd` carries is scrubbed, as for the
//! vendors' own CLIs, unless the definition sets that variable itself.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::drive::local::AcpCommand;
use crate::drive::programs::{self, Cli};
use crate::drive::{claude, codex, cursor, Framing, Launch, Place};

/// How to run `command` for an agent in `place`.
pub fn launch(command: &AcpCommand, place: &Place) -> Launch {
    let given = &command.agent.program;
    let path = env::var_os("PATH");
    let host = Host {
        program: program(given, path.as_deref()).unwrap_or_else(|| PathBuf::from(given)),
        path,
        inherited: env::vars_os().map(|(name, _)| name).collect(),
    };
    command_line(&host, command, place)
}

/// The program `given` names: an absolute path that runs, or a name found
/// in an absolute folder on `path`, and on Windows in the usual install
/// folders. None for anything else, such as a relative path.
pub fn program(given: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let named = Path::new(given);
    if named.is_absolute() {
        return programs::runnable(named).then(|| named.to_owned());
    }
    let separated = given.contains('/') || (cfg!(windows) && given.contains('\\'));
    if given.is_empty() || separated {
        return None;
    }
    programs::find(given, path, &[], Cli::Other)
}

/// What `launch` reads from `maxd`'s own process, kept apart so tests can
/// build a command line without it.
pub(super) struct Host {
    /// The program to run.
    pub program: PathBuf,
    /// `maxd`'s `PATH`.
    pub path: Option<OsString>,
    /// The names of the variables `maxd` runs with.
    pub inherited: Vec<OsString>,
}

/// Builds the command line from `host` without reading anything else.
pub(super) fn command_line(host: &Host, command: &AcpCommand, place: &Place) -> Launch {
    let agent = &command.agent;
    let secret_env: Vec<_> = command
        .env
        .iter()
        .filter(|value| agent.env.contains(&value.name))
        .map(|value| (OsString::from(&value.name), value.value.clone()))
        .collect();
    let env: Vec<_> = path_first(&host.program, host.path.as_deref())
        .into_iter()
        .collect();

    let mut scrub: Vec<OsString> = ["NODE_OPTIONS", claude::OAUTH_TOKEN, cursor::API_KEY]
        .into_iter()
        .chain([cursor::AUTH_TOKEN])
        .chain(codex::OTHER_AUTH)
        .map(OsString::from)
        .collect();
    scrub.extend(
        host.inherited
            .iter()
            .filter(|name| claude_family(name))
            .cloned(),
    );
    let mut env_remove: Vec<OsString> = Vec::new();
    for name in scrub {
        let set_here = name == "PATH" || agent.env.iter().any(|set| OsStr::new(set) == name);
        if !set_here && !env_remove.contains(&name) {
            env_remove.push(name);
        }
    }

    Launch {
        program: host.program.clone(),
        args: agent.args.iter().map(OsString::from).collect(),
        env,
        env_remove,
        cwd: place.cwd.clone(),
        framing: Framing::JsonRpc,
        secret_env,
        run_as: None,
    }
}

/// Whether `name` is one of the variables that could pick another Claude
/// account or key.
fn claude_family(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        claude::STRIPPED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    })
}

/// `PATH` with the program's own folder first, so a CLI that runs its
/// siblings finds them. None for a program with no absolute folder.
pub(crate) fn path_first(program: &Path, path: Option<&OsStr>) -> Option<(OsString, OsString)> {
    let dir = program.parent().filter(|dir| dir.is_absolute())?;
    Some(("PATH".into(), programs::path_with_first(dir, path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::PlaceHost;
    use maxhq_protocol::acp::{AcpAgent, AcpEnvValue};
    use maxhq_protocol::ids::{AcpAgentId, DeviceId, WorkspaceId};
    use maxhq_protocol::Secret;

    fn command(env: &[&str], values: &[(&str, &str)]) -> AcpCommand {
        AcpCommand {
            agent: AcpAgent {
                id: AcpAgentId::derive("gemini"),
                device: DeviceId::derive("devbox"),
                name: "Gemini".to_owned(),
                program: "gemini".to_owned(),
                args: vec!["--experimental-acp".to_owned(), "a b; rm -rf /".to_owned()],
                env: env.iter().map(|name| (*name).to_owned()).collect(),
                containers: false,
                problem: None,
                skills: Default::default(),
            },
            env: values
                .iter()
                .map(|(name, value)| AcpEnvValue {
                    name: (*name).to_owned(),
                    value: Secret::new((*value).to_owned()),
                })
                .collect(),
        }
    }

    fn place() -> Place {
        Place {
            device: DeviceId::derive("devbox"),
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
            program: PathBuf::from("/home/me/.local/bin/gemini"),
            path: Some("/usr/bin:/bin".into()),
            inherited: vec![
                "HOME".into(),
                "ANTHROPIC_API_KEY".into(),
                "CLAUDE_CONFIG_DIR".into(),
            ],
        }
    }

    fn strings(list: &[OsString]) -> Vec<&str> {
        list.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn the_arguments_go_as_given_with_the_programs_folder_on_path() {
        let launch = command_line(&host(), &command(&[], &[]), &place());
        assert_eq!(launch.program, PathBuf::from("/home/me/.local/bin/gemini"));
        assert_eq!(
            strings(&launch.args),
            ["--experimental-acp", "a b; rm -rf /"]
        );
        assert_eq!(launch.cwd, PathBuf::from("/work/repo"));
        assert_eq!(launch.framing, Framing::JsonRpc);
        assert_eq!(
            launch.env,
            [("PATH".into(), "/home/me/.local/bin:/usr/bin:/bin".into())]
        );
    }

    #[test]
    fn values_go_only_in_the_secret_variables() {
        let launch = command_line(
            &host(),
            &command(
                &["GEMINI_API_KEY"],
                &[("GEMINI_API_KEY", "g_123"), ("OTHER", "x")],
            ),
            &place(),
        );
        assert_eq!(
            launch.secret_env,
            [(
                OsString::from("GEMINI_API_KEY"),
                Secret::new("g_123".to_owned())
            )]
        );
        assert!(!launch
            .env
            .iter()
            .any(|(_, value)| value.to_string_lossy().contains("g_123")));
        assert!(!strings(&launch.args)
            .iter()
            .any(|arg| arg.contains("g_123")));
    }

    #[test]
    fn every_vendor_credential_is_scrubbed_but_what_the_definition_sets() {
        let launch = command_line(&host(), &command(&[], &[]), &place());
        let removed = strings(&launch.env_remove);
        for name in [
            "NODE_OPTIONS",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CONFIG_DIR",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CURSOR_API_KEY",
            "CURSOR_AUTH_TOKEN",
        ] {
            assert!(removed.contains(&name), "{name} should be removed");
        }
        assert!(!removed.contains(&"HOME"));

        let own = command(&["OPENAI_API_KEY"], &[("OPENAI_API_KEY", "sk")]);
        let launch = command_line(&host(), &own, &place());
        assert!(!strings(&launch.env_remove).contains(&"OPENAI_API_KEY"));
        assert_eq!(launch.secret_env.len(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn a_program_is_an_absolute_path_or_a_name_on_path() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = crate::testing::TempDir::new();
        let bin = dir.path().join("agent");
        fs::write(&bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.path().join("notes");
        fs::write(&plain, "").unwrap();
        let path = OsString::from(dir.path());

        assert_eq!(program("agent", Some(&path)), Some(bin.clone()));
        assert_eq!(program(bin.to_str().unwrap(), None), Some(bin.clone()));
        assert_eq!(program("notes", Some(&path)), None);
        assert_eq!(program("missing", Some(&path)), None);
        assert_eq!(program("./agent", Some(&path)), None);
        assert_eq!(program("", Some(&path)), None);
        let relative = crate::testing::from_here(dir.path());
        assert_eq!(program("agent", Some(relative.as_os_str())), None);
    }
}
