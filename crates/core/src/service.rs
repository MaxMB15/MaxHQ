// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The login service `maxd` runs as: its names under launchd and systemd,
//! and the commands that restart it. `maxd` restarts itself with them after
//! an update, and the app restarts a device's `maxd` with them, which works
//! however old that `maxd` is. The app also reads which `maxd` the
//! LaunchAgent or the systemd unit runs, from the file both of them know.

use std::path::{Path, PathBuf};

/// The LaunchAgent's label, which is also its file name.
pub const LABEL: &str = "com.maxagenthq.maxd";

/// The systemd user unit's name. The NixOS module's unit has it too.
pub const UNIT: &str = "maxd.service";

/// The LaunchAgent `maxd service install` writes for the user at `home`.
pub fn launch_agent_file(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The program a LaunchAgent's `plist` runs: the first of its
/// `ProgramArguments`.
pub fn program_in(plist: &str) -> Option<PathBuf> {
    let (_, rest) = plist.split_once("<key>ProgramArguments</key>")?;
    let (_, rest) = rest.split_once("<string>")?;
    let (program, _) = rest.split_once("</string>")?;
    let program = program
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    Some(PathBuf::from(program))
}

/// The systemd user unit `maxd service install` writes, under
/// `config_home`: `$XDG_CONFIG_HOME`, or `~/.config` without one.
pub fn unit_file(config_home: &Path) -> PathBuf {
    config_home.join("systemd/user").join(UNIT)
}

/// The program a systemd `unit` runs: the first word of its `ExecStart=`,
/// unquoted the way `maxd service install` quotes it. systemd reads `%%`
/// and `$$` as one `%` and `$`, and a backslash keeps the next character.
pub fn program_in_unit(unit: &str) -> Option<PathBuf> {
    let line = unit
        .lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))?
        .trim_start();
    let (quoted, line) = match line.strip_prefix('"') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let mut program = String::new();
    let mut chars = line.chars().peekable();
    loop {
        match chars.next() {
            // An open quote with no end is no program.
            None if quoted => return None,
            None => break,
            Some('"') if quoted => break,
            Some(c) if !quoted && c.is_whitespace() => break,
            Some('\\') => match chars.next()? {
                'n' => program.push('\n'),
                c => program.push(c),
            },
            Some(c @ ('%' | '$')) => {
                chars.next_if_eq(&c);
                program.push(c);
            }
            Some(c) => program.push(c),
        }
    }
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// What starts and stops services on a system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    /// macOS's launchd, through a LaunchAgent.
    Launchd,
    /// systemd's per-user manager, through a user unit.
    Systemd,
}

impl Manager {
    /// The manager on this system: launchd on macOS, systemd on Linux, and
    /// none anywhere else.
    pub fn here() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Self::Systemd)
        } else {
            None
        }
    }
}

/// The program and arguments that restart `maxd`'s service under
/// `manager`. `uid` names your launchd session, and systemd doesn't need
/// it.
pub fn restart_command(manager: Manager, uid: u32) -> (&'static str, Vec<String>) {
    match manager {
        Manager::Launchd => (
            "launchctl",
            vec![
                "kickstart".to_owned(),
                "-k".to_owned(),
                format!("gui/{uid}/{LABEL}"),
            ],
        ),
        Manager::Systemd => (
            "systemctl",
            vec!["--user".to_owned(), "restart".to_owned(), UNIT.to_owned()],
        ),
    }
}

/// One command that restarts `maxd`'s service on a device you reach over
/// SSH, for its login shell to run. It picks launchd on macOS and
/// `systemctl --user` everywhere else, the same as [`restart_command`].
/// The script is fixed text in single quotes, which sh, bash, zsh and fish
/// all pass on unchanged.
pub fn remote_restart_command() -> String {
    format!(
        r#"sh -c 'if [ "$(uname)" = Darwin ]; then exec launchctl kickstart -k "gui/$(id -u)/{LABEL}"; else exec systemctl --user restart {UNIT}; fi'"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_manager_has_its_restart() {
        let (program, args) = restart_command(Manager::Launchd, 501);
        assert_eq!(program, "launchctl");
        assert_eq!(args, ["kickstart", "-k", "gui/501/com.maxagenthq.maxd"]);
        let (program, args) = restart_command(Manager::Systemd, 1000);
        assert_eq!(program, "systemctl");
        assert_eq!(args, ["--user", "restart", "maxd.service"]);
    }

    #[test]
    fn the_unit_names_the_program_it_runs() {
        assert_eq!(
            unit_file(Path::new("/home/someone/.config")),
            Path::new("/home/someone/.config/systemd/user/maxd.service")
        );
        let unit =
            "[Service]\nExecStart=\"/opt/max hq/100%%/$$bin/m\\\\d\\\"x\" run\nRestart=always\n";
        assert_eq!(
            program_in_unit(unit),
            Some(PathBuf::from("/opt/max hq/100%/$bin/m\\d\"x"))
        );
        assert_eq!(
            program_in_unit("ExecStart=/home/someone/.local/bin/maxd run\n"),
            Some(PathBuf::from("/home/someone/.local/bin/maxd"))
        );
        assert_eq!(program_in_unit("ExecStart=\"/opt/maxd run\n"), None);
        assert_eq!(program_in_unit("ExecStart=\n"), None);
        assert_eq!(program_in_unit("[Service]\nRestart=always\n"), None);
    }

    #[test]
    fn the_remote_restart_is_fixed() {
        assert_eq!(
            remote_restart_command(),
            r#"sh -c 'if [ "$(uname)" = Darwin ]; then exec launchctl kickstart -k "gui/$(id -u)/com.maxagenthq.maxd"; else exec systemctl --user restart maxd.service; fi'"#
        );
    }

    /// The script runs as written on this machine, with fake service
    /// managers that print what they were asked.
    #[cfg(unix)]
    #[test]
    fn the_remote_restart_runs_in_sh() {
        use std::os::unix::fs::PermissionsExt;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("maxhq-restart-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for program in ["launchctl", "systemctl"] {
            let path = dir.join(program);
            std::fs::write(&path, format!("#!/bin/sh\necho {program} \"$@\"\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("{}:/usr/bin:/bin", dir.display());
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(remote_restart_command())
            .env("PATH", path)
            .output()
            .unwrap();
        let printed = String::from_utf8(output.stdout).unwrap();
        let expected = if cfg!(target_os = "macos") {
            let uid = std::process::Command::new("id").arg("-u").output().unwrap();
            let uid = String::from_utf8(uid.stdout).unwrap();
            format!("launchctl kickstart -k gui/{}/{LABEL}\n", uid.trim())
        } else {
            format!("systemctl --user restart {UNIT}\n")
        };
        assert_eq!(printed, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
