// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Restarts a device's `maxd` through its service manager rather than by
//! asking `maxd`, so it works whatever protocol that `maxd` speaks, even
//! one too old for anything else the app sends. On this machine it runs
//! `launchctl` or `systemctl --user` itself. Over SSH it runs one fixed
//! command that picks the far side's manager. A WSL distro's runs
//! `systemctl --user` through `wsl.exe`, on this PC or over SSH on the one
//! it's on. The commands come from `maxhq_core::service`, which `maxd`
//! restarts itself with too.

use std::future::Future;
use std::io;

use maxhq_client::setup::{wsl_over_ssh, Output};
use maxhq_client::wsl;
use maxhq_core::service::{remote_restart_command, restart_command, Manager};
use maxhq_protocol::link::Route;

use crate::request::CommandError;

/// Runs the restart's commands. Tests give a fake.
pub trait Restarter: Send + Sync {
    /// Runs `program` with `args` on this machine.
    fn here(
        &self,
        program: &str,
        args: Vec<String>,
    ) -> impl Future<Output = io::Result<Output>> + Send;

    /// Runs `command` on `host` over SSH.
    fn over_ssh(
        &self,
        host: &str,
        command: &str,
    ) -> impl Future<Output = io::Result<Output>> + Send;
}

/// This machine's service manager and your user ID, which the local
/// restart needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Here {
    /// launchd or systemd, or none on a system with neither.
    pub manager: Option<Manager>,
    /// Your user ID, which names your launchd session.
    pub uid: u32,
}

impl Here {
    /// This machine.
    pub fn now() -> Self {
        Self {
            manager: Manager::here(),
            uid: user_id(),
        }
    }
}

/// Your user ID on Unix. Windows names no launchd or systemd session by
/// one, so it's 0 there.
fn user_id() -> u32 {
    #[cfg(unix)]
    let uid = rustix::process::getuid().as_raw();
    #[cfg(not(unix))]
    let uid = 0;
    uid
}

/// Restarts the `maxd` that `route` reaches, and waits for the service
/// manager to say it did.
pub async fn restart(
    route: &Route,
    here: Here,
    runner: &impl Restarter,
) -> Result<(), CommandError> {
    let ran = match route {
        Route::Local => {
            let Some(manager) = here.manager else {
                return Err(failed(
                    "This machine has neither launchd nor systemd to restart maxd with",
                ));
            };
            let (program, args) = restart_command(manager, here.uid);
            runner
                .here(program, args)
                .await
                .map_err(|error| failed(&format!("Couldn't run {program}: {error}")))?
        }
        Route::Ssh { host } => runner
            .over_ssh(host, &remote_restart_command())
            .await
            .map_err(|error| failed(&format!("Couldn't reach {host}: {error}")))?,
        Route::Wsl { distro, via } => {
            let (program, args) = restart_command(Manager::Systemd, 0);
            let script = format!("exec {program} {}", args.join(" "));
            match via {
                None => runner
                    .here(
                        wsl::PROGRAM,
                        wsl::exec_args(distro, &wsl::in_distro(&script), &[]),
                    )
                    .await
                    .map_err(|error| failed(&format!("Couldn't run wsl.exe: {error}")))?,
                Some(host) => {
                    let Some(command) = wsl_over_ssh(distro, &script) else {
                        return Err(failed("That isn't a WSL distro's name"));
                    };
                    runner
                        .over_ssh(host, &command)
                        .await
                        .map_err(|error| failed(&format!("Couldn't reach {host}: {error}")))?
                }
            }
        }
    };
    if ran.ok {
        tracing::info!(?route, "restarted maxd");
        return Ok(());
    }
    tracing::warn!(?route, stderr = %ran.stderr, "restarting maxd failed");
    Err(failed(match ran.stderr.trim() {
        "" => "The service manager couldn't restart maxd",
        reason => reason,
    }))
}

/// The error for a restart that didn't happen, as a sentence.
fn failed(message: &str) -> CommandError {
    let mut chars = message.chars();
    let message = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    CommandError::Failed { message }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use futures::executor::block_on;

    use super::*;

    /// Records what would run, and answers with `answer`.
    pub(crate) struct Recorded {
        pub ran: Mutex<Vec<String>>,
        pub answer: fn() -> io::Result<Output>,
    }

    impl Recorded {
        pub(crate) fn answering(answer: fn() -> io::Result<Output>) -> Self {
            Self {
                ran: Mutex::new(Vec::new()),
                answer,
            }
        }

        pub(crate) fn ran(&self) -> Vec<String> {
            self.ran.lock().unwrap().clone()
        }
    }

    impl Restarter for Recorded {
        async fn here(&self, program: &str, args: Vec<String>) -> io::Result<Output> {
            self.ran
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            (self.answer)()
        }

        async fn over_ssh(&self, host: &str, command: &str) -> io::Result<Output> {
            self.ran
                .lock()
                .unwrap()
                .push(format!("ssh {host} {command}"));
            (self.answer)()
        }
    }

    pub(crate) fn worked() -> io::Result<Output> {
        Ok(Output {
            ok: true,
            ..Default::default()
        })
    }

    fn mac() -> Here {
        Here {
            manager: Some(Manager::Launchd),
            uid: 501,
        }
    }

    #[test]
    fn each_route_and_manager_has_its_command() {
        let launchd = Recorded::answering(worked);
        block_on(restart(&Route::Local, mac(), &launchd)).unwrap();
        assert_eq!(
            launchd.ran(),
            ["launchctl kickstart -k gui/501/com.maxagenthq.maxd"]
        );

        let systemd = Recorded::answering(worked);
        let linux = Here {
            manager: Some(Manager::Systemd),
            uid: 1000,
        };
        block_on(restart(&Route::Local, linux, &systemd)).unwrap();
        assert_eq!(systemd.ran(), ["systemctl --user restart maxd.service"]);

        let ssh = Recorded::answering(worked);
        let route = Route::Ssh {
            host: "dev@devbox".to_owned(),
        };
        block_on(restart(&route, mac(), &ssh)).unwrap();
        assert_eq!(
            ssh.ran(),
            [format!("ssh dev@devbox {}", remote_restart_command())],
            "the far side picks its own manager, whatever this machine has"
        );

        let distro = Recorded::answering(worked);
        let wsl = |via: Option<&str>| Route::Wsl {
            distro: "Ubuntu".to_owned(),
            via: via.map(str::to_owned),
        };
        block_on(restart(&wsl(None), mac(), &distro)).unwrap();
        let ran = distro.ran();
        assert!(
            ran[0].starts_with("wsl.exe -d Ubuntu --exec sh -c ")
                && ran[0].ends_with("exec systemctl --user restart maxd.service"),
            "{ran:?}"
        );

        let pc = Recorded::answering(worked);
        block_on(restart(&wsl(Some("dev@pc")), mac(), &pc)).unwrap();
        let ran = pc.ran();
        assert!(
            ran[0].starts_with("ssh dev@pc powershell -NoProfile -NonInteractive -EncodedCommand "),
            "a Windows machine's shell may be cmd or PowerShell: {ran:?}"
        );

        let nothing = Recorded::answering(worked);
        let none = Here {
            manager: None,
            uid: 501,
        };
        assert!(block_on(restart(&Route::Local, none, &nothing)).is_err());
        assert!(nothing.ran().is_empty());
    }

    #[test]
    fn a_failed_restart_says_why() {
        let refused = Recorded::answering(|| {
            Ok(Output {
                ok: false,
                stderr: "could not find service \"com.maxagenthq.maxd\"\n".to_owned(),
                ..Default::default()
            })
        });
        assert_eq!(
            block_on(restart(&Route::Local, mac(), &refused)),
            Err(CommandError::Failed {
                message: "Could not find service \"com.maxagenthq.maxd\"".to_owned()
            })
        );

        let quiet = Recorded::answering(|| Ok(Output::default()));
        assert_eq!(
            block_on(restart(&Route::Local, mac(), &quiet)),
            Err(CommandError::Failed {
                message: "The service manager couldn't restart maxd".to_owned()
            })
        );

        let unreachable = Recorded::answering(|| Err(io::Error::other("SSH sign-in was refused")));
        let route = Route::Ssh {
            host: "dev@devbox".to_owned(),
        };
        assert_eq!(
            block_on(restart(&route, mac(), &unreachable)),
            Err(CommandError::Failed {
                message: "Couldn't reach dev@devbox: SSH sign-in was refused".to_owned()
            })
        );
    }
}
