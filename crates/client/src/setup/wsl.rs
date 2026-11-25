// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Setup of a WSL distro: the Linux steps, each run in the distro by
//! `wsl.exe` in place of SSH. On this machine setup starts `wsl.exe`
//! itself, with the arguments it takes word for word. On a Windows
//! machine it reaches over SSH, a PowerShell step there starts it, as on
//! any Windows PC ([`super::windows`]): the arguments go in the one
//! command line Windows programs split, and stdin and stdout pass through.
//!
//! A distro has no host key of its own to keep: the link reaches it
//! through `wsl.exe`, on this machine or the Windows one.

use super::windows::powershell;
use super::{
    parse_probe, stop, Confirmations, Prepared, Run, Setup, SetupBinary, SetupError, Step,
};
use super::{SetupPaths, PROBE};
use crate::wsl::{self, exec_args, is_distro_name};

/// Starts `wsl.exe` with the arguments set before it as `$line`, passes
/// stdin to it and its output back, and exits as it does.
const RELAY: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$p = New-Object System.Diagnostics.Process
$p.StartInfo.FileName = 'wsl.exe'
$p.StartInfo.Arguments = $line
$p.StartInfo.UseShellExecute = $false
$p.StartInfo.RedirectStandardInput = $true
$p.StartInfo.RedirectStandardOutput = $true
$p.StartInfo.RedirectStandardError = $true
[void]$p.Start()
$out = $p.StandardOutput.BaseStream.CopyToAsync([Console]::OpenStandardOutput())
$err = $p.StandardError.BaseStream.CopyToAsync([Console]::OpenStandardError())
[Console]::OpenStandardInput().CopyTo($p.StandardInput.BaseStream)
$p.StandardInput.Close()
$p.WaitForExit()
$out.Wait()
$err.Wait()
exit $p.ExitCode
"#;

impl<'a, R: Run> Setup<'a, R> {
    /// A round for the WSL distro `distro` on this machine, or with `via`
    /// on the Windows machine that SSH destination reaches.
    pub fn wsl(
        run: &'a R,
        paths: &'a SetupPaths,
        distro: &str,
        via: Option<&str>,
    ) -> Result<Self, SetupError> {
        if !is_distro_name(distro) {
            return Err(stop(format!("{distro} isn't a WSL distro's name")));
        }
        let host = match via {
            Some(via) => Self::new(run, paths, via)?.host,
            None => String::new(),
        };
        Ok(Self {
            run,
            paths,
            host,
            wsl: Some(distro.to_owned()),
            trust: Vec::new(),
            app_key: None,
        })
    }

    /// [`Setup::prepare`] for a WSL distro: the Windows machine's host key
    /// when it's another, then the Linux probe and steps in the distro.
    pub(super) async fn prepare_wsl(
        mut self,
        binary: impl Fn(&str) -> Option<SetupBinary>,
        confirms: &Confirmations,
        step: &(dyn Fn(Step) + Sync),
    ) -> Result<Prepared, SetupError> {
        if !self.host.is_empty() {
            step(Step::HostKey);
            // They're the Windows machine's keys, not the distro's.
            self.host_key(confirms, step).await?;
        }
        step(Step::Probe);
        let facts = parse_probe(&self.linux(PROBE, &[], None).await?);
        self.prepare_linux(facts, binary, step, (Vec::new(), false))
            .await
    }

    /// Runs `script` in `sh` in the WSL distro `distro`, with `args` as
    /// `$1` onwards.
    pub(super) async fn in_distro(
        &self,
        distro: &str,
        script: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<String, SetupError> {
        let mut all = vec!["maxhq"];
        all.extend_from_slice(args);
        let args = exec_args(distro, &wsl::in_distro(script), &all);
        let output = if self.host.is_empty() {
            self.run
                .run(wsl::PROGRAM, args, stdin)
                .await
                .map_err(|error| stop(format!("wsl.exe didn't run: {error}")))?
        } else {
            return self.ssh(vec![powershell(&relay(&args))], stdin).await;
        };
        if !output.ok {
            let why = output.stderr.trim();
            return Err(stop(if why.is_empty() {
                format!("A step in {distro} failed")
            } else {
                format!("A step in {distro} failed: {why}")
            }));
        }
        Ok(output.stdout)
    }
}

/// What runs `script` in WSL distro `distro` on a Windows machine over
/// SSH, whichever shell its SSH server starts: a PowerShell step that
/// starts `wsl.exe`, as setup's do. None for a name that isn't a distro's.
pub fn over_ssh(distro: &str, script: &str) -> Option<String> {
    is_distro_name(distro).then(|| {
        powershell(&relay(&exec_args(
            distro,
            &wsl::in_distro(script),
            &["maxhq"],
        )))
    })
}

/// The PowerShell script that runs `wsl.exe` with `args` on a Windows
/// machine.
fn relay(args: &[String]) -> String {
    format!(
        "$line = {}\n{RELAY}",
        super::windows::literal(&command_line(args))
    )
}

/// `args` as one Windows command line, quoted so a program splits it back
/// into the same words, as Rust's own `Command` does on Windows.
fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One argument for a Windows command line. One with no space, tab or
/// quote goes as it is. Otherwise it goes in quotes, a quote in it after
/// a backslash, and the backslashes before a quote or the closing one
/// doubled.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
            }
        }
        if c != '\\' {
            quoted.push(c);
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// Why a distro without a systemd user session can't take `maxd`, and
/// how to give it one.
pub(super) fn no_systemd(distro: &str) -> String {
    format!(
        "{distro} has no systemd user session, which maxd's service needs. Turn systemd on in \
         the distro: add [boot] with systemd=true to /etc/wsl.conf, then run wsl.exe --shutdown \
         in Windows. A WSL 1 distro has no systemd, so first run wsl.exe --set-version {distro} 2"
    )
}

/// What keeps `user`'s services running in `distro` while nothing's
/// logged in, run from Windows. Root in a distro needs no password there.
pub(super) fn linger(distro: &str, user: &str) -> String {
    format!("wsl.exe -d {distro} -u root loginctl enable-linger {user}")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{Output, MAXD_LINK_TEMP};
    use super::*;

    /// A program it ran: its name, its arguments and what went to its
    /// stdin.
    type Ran = (String, Vec<String>, Option<Vec<u8>>);

    /// A Windows machine with a distro that answers the probe, run here
    /// or over SSH.
    struct Fake {
        probe: String,
        ran: Mutex<Vec<Ran>>,
    }

    impl Fake {
        fn new(probe: &str) -> Self {
            Self {
                probe: probe.to_owned(),
                ran: Mutex::new(Vec::new()),
            }
        }

        fn ran(&self) -> Vec<Ran> {
            self.ran.lock().unwrap().clone()
        }
    }

    impl Run for Fake {
        async fn run(
            &self,
            program: &str,
            args: Vec<String>,
            stdin: Option<Vec<u8>>,
        ) -> std::io::Result<Output> {
            self.ran
                .lock()
                .unwrap()
                .push((program.to_owned(), args.clone(), stdin));
            let probe = args.iter().any(|arg| arg.ends_with(PROBE))
                || args.iter().any(|arg| {
                    script_of(arg).is_some_and(|script| script.contains("uname=$(uname -sm)"))
                });
            let known = program == "ssh-keygen" && args.first().is_some_and(|arg| arg == "-F");
            Ok(Output {
                ok: true,
                stdout: if probe {
                    self.probe.clone()
                } else if known {
                    "pc ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHostHostHostHostHostHostHostHostHostHost\n"
                        .to_owned()
                } else if program == "ssh" && args.first().is_some_and(|arg| arg == "-G") {
                    "hostname pc\nport 22\nuserknownhostsfile /home/dev/.ssh/known_hosts\n"
                        .to_owned()
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }
    }

    fn script_of(command: &str) -> Option<String> {
        use base64::Engine;
        let encoded =
            command.strip_prefix("powershell -NoProfile -NonInteractive -EncodedCommand ")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).ok()
    }

    fn paths() -> SetupPaths {
        let dir = std::env::temp_dir().join(format!("maxhq-wsl-setup-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("id_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeFakeFakeFakeFakeFakeFakeFakeFakeFake maxhq@pc\n",
        )
        .unwrap();
        std::fs::write(dir.join("maxd"), b"maxd").unwrap();
        SetupPaths {
            key: dir.join("id_ed25519"),
            known_hosts: dir.join("known_hosts"),
            scratch: dir.join("setup"),
        }
    }

    fn binary(paths: &SetupPaths) -> impl Fn(&str) -> Option<SetupBinary> {
        let path = paths.key.with_file_name("maxd");
        move |target| {
            (target == "x86_64-unknown-linux-musl").then(|| SetupBinary {
                path: path.clone(),
                sha256: "ab".repeat(32),
            })
        }
    }

    const PROBED: &str = "uname=Linux x86_64\nhome=/home/dev\nuser=dev\nsystemd=yes\nlinger=no\n";

    #[tokio::test]
    async fn a_distro_here_takes_the_linux_steps_through_wsl_exe() {
        let paths = paths();
        let fake = Fake::new(PROBED);
        let steps = Mutex::new(Vec::new());
        let prepared = Setup::wsl(&fake, &paths, "Ubuntu", None)
            .unwrap()
            .prepare(binary(&paths), &Confirmations::default(), &|step| {
                steps.lock().unwrap().push(step);
            })
            .await
            .unwrap();
        assert_eq!(
            prepared,
            Prepared {
                host: "wsl:Ubuntu".into(),
                home: "/home/dev".into(),
                target: "x86_64-unknown-linux-musl".into(),
                host_keys: Vec::new(),
                host_keys_confirmed: false,
                linger: false,
            }
        );
        let ran = fake.ran();
        assert!(
            ran.iter().all(|(program, ..)| program == "wsl.exe"),
            "{ran:?}"
        );
        assert_eq!(ran.len(), 4, "{ran:?}");
        // The probe, then the install with the binary on stdin and the
        // sum, build and link name as arguments.
        assert_eq!(
            ran[0].1,
            exec_args("Ubuntu", &wsl::in_distro(PROBE), &["maxhq"])
        );
        let build = "ab".repeat(8);
        let sum = "ab".repeat(32);
        assert_eq!(
            ran[1].1,
            exec_args(
                "Ubuntu",
                &wsl::in_distro(super::super::INSTALL),
                &["maxhq", &sum, &build, MAXD_LINK_TEMP]
            )
        );
        assert_eq!(ran[1].2.as_deref(), Some(&b"maxd"[..]));
        assert_eq!(
            ran[2].1[5],
            wsl::in_distro("\"$HOME/.local/bin/maxd\" service install")
        );
        assert_eq!(ran[3].1[5], wsl::in_distro(super::super::ADD_KEY));
        let steps = steps.lock().unwrap().clone();
        assert_eq!(steps[0], Step::Probe);
        assert!(steps.contains(&Step::Linger {
            command: "wsl.exe -d Ubuntu -u root loginctl enable-linger dev".into()
        }));
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }

    #[tokio::test]
    async fn a_distro_without_systemd_says_how_to_turn_it_on() {
        let paths = paths();
        let fake = Fake::new("uname=Linux x86_64\nhome=/home/dev\nuser=dev\nsystemd=no\n");
        let error = Setup::wsl(&fake, &paths, "Debian", None)
            .unwrap()
            .prepare(binary(&paths), &Confirmations::default(), &|_| {})
            .await
            .unwrap_err();
        assert!(error.0.contains("systemd=true"), "{error}");
        assert!(
            error.0.contains("wsl.exe --set-version Debian 2"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }

    #[tokio::test]
    async fn a_distro_on_another_pc_runs_wsl_exe_there_through_powershell() {
        let paths = paths();
        let fake = Fake::new(PROBED);
        let prepared = Setup::wsl(&fake, &paths, "Ubuntu", Some("dev@pc"))
            .unwrap()
            .prepare(binary(&paths), &Confirmations::default(), &|_| {})
            .await
            .unwrap();
        assert_eq!(prepared.host, "wsl:Ubuntu@dev@pc");
        assert!(prepared.host_keys.is_empty());
        let scripts: Vec<String> = fake
            .ran()
            .into_iter()
            .filter(|(program, args, _)| program == "ssh" && args.iter().any(|a| a == "dev@pc"))
            .filter_map(|(_, args, _)| args.last().and_then(|last| script_of(last)))
            .collect();
        assert_eq!(scripts.len(), 4, "{scripts:?}");
        assert!(
            scripts[0].starts_with("$line = '-d Ubuntu --exec sh -c \"if [ -d \\\"/run/user/"),
            "{}",
            scripts[0]
        );
        assert!(
            scripts[0].contains("echo \\\"uname=$(uname -sm)\\\""),
            "{}",
            scripts[0]
        );
        assert!(scripts.iter().all(|script| script.ends_with(RELAY)));
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }

    #[test]
    fn a_distro_name_must_be_one() {
        let paths = paths();
        let fake = Fake::new(PROBED);
        assert!(Setup::wsl(&fake, &paths, "-d", None).is_err());
        assert!(Setup::wsl(&fake, &paths, "Ubuntu", Some("-oProxyCommand=x")).is_err());
        let _ = std::fs::remove_dir_all(paths.key.parent().unwrap());
    }

    #[test]
    fn arguments_split_back_into_the_same_words() {
        let words =
            |args: &[&str]| command_line(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        assert_eq!(words(&["-d", "Ubuntu"]), "-d Ubuntu");
        assert_eq!(words(&["a b", ""]), "\"a b\" \"\"");
        assert_eq!(words(&["say \"hi\""]), "\"say \\\"hi\\\"\"");
        assert_eq!(words(&["C:\\dir\\ x\\"]), "\"C:\\dir\\ x\\\\\"");
        assert_eq!(words(&["a\\\"b"]), "\"a\\\\\\\"b\"");
    }
}
