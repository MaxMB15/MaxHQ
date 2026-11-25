// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The login service `maxd` runs as: its names under launchd, systemd and
//! Task Scheduler, and the commands that restart it. `maxd` restarts
//! itself with them after an update, and the app restarts a device's
//! `maxd` with them, which works however old that `maxd` is. The app also
//! reads which `maxd` the LaunchAgent, the systemd unit or the logon task
//! runs, from the file both of them know.
//!
//! On Windows the service is a logon task of the user's own, which needs
//! no admin. It runs the launcher, `maxdw.exe` ([`launcher_program`]),
//! which starts `maxd run` and starts it again, as launchd and systemd do.

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

/// The start of the logon task's name. Every user's tasks share Task
/// Scheduler's top folder, so [`task_name`] adds the user's SID.
pub const TASK_PREFIX: &str = "MaxHQ maxd";

/// The exit code with which `maxd run` asks to be started again at once,
/// after an update or when asked to restart. On Windows the launcher reads
/// it, and 0 there means stopped for good. launchd and systemd start
/// `maxd` again whatever its exit, so it's 0 there as always.
pub const RESTART_EXIT: i32 = if cfg!(windows) { 75 } else { 0 };

/// The subcommand of `maxd` that the logon task runs: the launcher.
pub const LAUNCH_SUBCOMMAND: &str = "launch";

/// The logon task's name for the user with the security ID `sid`.
pub fn task_name(sid: &str) -> String {
    format!("{TASK_PREFIX} {sid}")
}

/// The logon task's name for the user this process runs as. Reading the
/// SID fails only off Windows, or when Windows won't tell a process its
/// own user, and then the name has no SID.
pub fn task_name_here() -> String {
    match maxhq_os::home::user_sid() {
        Ok(sid) => task_name(&sid),
        Err(_) => TASK_PREFIX.to_owned(),
    }
}

/// The launcher the logon task runs, in `app`, MaxHQ's folder
/// ([`crate::paths::windows_app_dir`]): `bin\maxdw.exe`, a windowless copy
/// of `maxd.exe` beside the `current` junction, which `maxd service
/// install` writes.
pub fn launcher_program(app: &Path) -> PathBuf {
    app.join("bin").join("maxdw.exe")
}

/// The log the launcher writes `maxd`'s output to, under `data`, `maxd`'s
/// data folder: `Logs\maxd.log`, as launchd's is `~/Library/Logs/MaxHQ`.
pub fn launcher_log(data: &Path) -> PathBuf {
    data.join("Logs").join("maxd.log")
}

/// The copy of the logon task `maxd service install` registered, in
/// `app`, MaxHQ's folder, beside the data as the LaunchAgent is. Task
/// Scheduler keeps its own where only admins can read it, so the app reads
/// this one.
pub fn task_file(app: &Path) -> PathBuf {
    app.join("maxd-task.xml")
}

/// The logon task for the user with the security ID `sid`, as Task
/// Scheduler's XML: at each logon of that user, run `launcher` with
/// `launch "<program>"`, as that user without elevation. Each run of the
/// task starts another launcher, which hands over to the running one, so
/// running the task restarts `maxd`. It never times out, runs on battery,
/// and starts again a minute after the launcher fails.
pub fn task_xml(sid: &str, launcher: &Path, program: &Path) -> String {
    let sid = xml_text(sid);
    let command = xml_text(&launcher.to_string_lossy());
    let arguments = xml_text(&format!(
        "{LAUNCH_SUBCOMMAND} \"{}\"",
        program.to_string_lossy()
    ));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Runs maxd, MaxHQ's background service, while you're signed in.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{sid}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// `xml` as Task Scheduler reads a task file: UTF-16, little end first,
/// after a byte order mark.
pub fn task_file_bytes(xml: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// The text of a task file [`task_file_bytes`] wrote. A file without the
/// byte order mark is read as UTF-8.
pub fn task_file_text(bytes: &[u8]) -> Option<String> {
    match bytes.strip_prefix(&[0xff, 0xfe]) {
        Some(units) if units.len() % 2 == 0 => {
            let units: Vec<u16> = units
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).ok()
        }
        Some(_) => None,
        None => String::from_utf8(bytes.to_vec()).ok(),
    }
}

/// The program a logon task's `xml` has its launcher run: the quoted path
/// after `launch` in its `Arguments`.
pub fn program_in_task(xml: &str) -> Option<PathBuf> {
    let (_, rest) = xml.split_once("<Arguments>")?;
    let (arguments, _) = rest.split_once("</Arguments>")?;
    let arguments = unxml(arguments);
    let quoted = arguments
        .strip_prefix(LAUNCH_SUBCOMMAND)?
        .trim_start()
        .strip_prefix('"')?;
    let (program, _) = quoted.split_once('"')?;
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// The program the logon task registered in `app`, MaxHQ's folder, runs,
/// from [`task_file`]. `None` when there's none.
pub fn task_program(app: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(task_file(app)).ok()?;
    program_in_task(&task_file_text(&bytes)?)
}

/// The program this user's logon task runs, from [`task_file`] in MaxHQ's
/// folder. `None` off Windows, or when there's no task.
pub fn task_program_here() -> Option<PathBuf> {
    crate::paths::windows_app_dir().and_then(|app| task_program(&app))
}

/// `text` with what XML reads as markup written as entities.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The inverse of [`xml_text`], and of the entities Task Scheduler writes.
fn unxml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// What starts and stops services on a system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    /// macOS's launchd, through a LaunchAgent.
    Launchd,
    /// systemd's per-user manager, through a user unit.
    Systemd,
    /// Windows's Task Scheduler, through a logon task of the user's own
    /// and the launcher it runs.
    TaskScheduler,
}

impl Manager {
    /// The manager on this system: launchd on macOS, systemd on Linux,
    /// Task Scheduler on Windows, and none anywhere else.
    pub fn here() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Self::Systemd)
        } else if cfg!(windows) {
            Some(Self::TaskScheduler)
        } else {
            None
        }
    }
}

/// The program and arguments that restart `maxd`'s service under
/// `manager`. `uid` names your launchd session, and the others don't need
/// it. Under Task Scheduler it runs the logon task, whose new launcher
/// asks the running `maxd` to restart, or starts one should none run.
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
        Manager::TaskScheduler => (
            "schtasks",
            vec!["/Run".to_owned(), "/TN".to_owned(), task_name_here()],
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

/// [`remote_restart_command`] for a Windows device, whose SSH login shell
/// is `cmd.exe` or PowerShell. `cmd` expands `%LOCALAPPDATA%` either way,
/// and the device's own `maxd` finds its logon task, whose name holds a
/// SID the app doesn't know.
pub fn remote_restart_command_windows() -> String {
    r#"cmd /c "%LOCALAPPDATA%\MaxHQ\bin\current\maxd.exe" service restart"#.to_owned()
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
        let (program, args) = restart_command(Manager::TaskScheduler, 0);
        assert_eq!(program, "schtasks");
        assert_eq!(args[..2], ["/Run", "/TN"]);
        assert!(args[2].starts_with("MaxHQ maxd"), "{}", args[2]);
    }

    #[test]
    fn this_system_has_its_manager() {
        let expected = if cfg!(target_os = "macos") {
            Some(Manager::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Manager::Systemd)
        } else if cfg!(windows) {
            Some(Manager::TaskScheduler)
        } else {
            None
        };
        assert_eq!(Manager::here(), expected);
    }

    #[test]
    fn each_user_has_a_task_of_their_own() {
        assert_eq!(
            task_name("S-1-5-21-1-2-3-1001"),
            "MaxHQ maxd S-1-5-21-1-2-3-1001"
        );
        let here = task_name_here();
        if cfg!(windows) {
            assert!(here.starts_with("MaxHQ maxd S-1-5-"), "{here}");
        } else {
            assert_eq!(here, TASK_PREFIX);
        }
    }

    #[test]
    fn the_task_runs_the_launcher_at_logon_as_you() {
        let app = Path::new(r"C:\Users\<user>\AppData\Local\Max & HQ");
        let launcher = launcher_program(app);
        assert_eq!(launcher, app.join("bin").join("maxdw.exe"));
        assert_eq!(task_file(app), app.join("maxd-task.xml"));
        let data = app.join("data");
        assert_eq!(launcher_log(&data), data.join("Logs").join("maxd.log"));
        let program = app.join("bin").join("current").join("maxd.exe");
        let xml = task_xml("S-1-5-21-1-2-3-1001", &launcher, &program);
        for part in [
            "<UserId>S-1-5-21-1-2-3-1001</UserId>\n    </LogonTrigger>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "Max &amp; HQ",
            "<Arguments>launch &quot;",
        ] {
            assert!(xml.contains(part), "{part} in {xml}");
        }
        assert!(!xml.contains("Max & HQ"));
        assert_eq!(program_in_task(&xml), Some(program));
        assert_eq!(program_in_task("<Task></Task>"), None);
        assert_eq!(
            program_in_task("<Arguments>run &quot;C:\\maxd.exe&quot;</Arguments>"),
            None
        );
        assert_eq!(
            program_in_task("<Arguments>launch &quot;C:\\maxd.exe</Arguments>"),
            None
        );
        // As Task Scheduler writes it back, with quotes left as they are.
        assert_eq!(
            program_in_task(r#"<Arguments>launch "C:\M&amp;H\maxd.exe"</Arguments>"#),
            Some(PathBuf::from(r"C:\M&H\maxd.exe"))
        );
    }

    #[test]
    fn the_task_file_is_utf16_and_reads_back() {
        let xml = task_xml(
            "S-1-5-21-1",
            Path::new("C:\\é\\maxdw.exe"),
            Path::new("C:\\é\\maxd.exe"),
        );
        let bytes = task_file_bytes(&xml);
        assert_eq!(bytes[..4], [0xff, 0xfe, b'<', 0]);
        assert_eq!(task_file_text(&bytes).as_deref(), Some(xml.as_str()));
        assert_eq!(
            task_file_text(xml.as_bytes()).as_deref(),
            Some(xml.as_str())
        );
        assert_eq!(task_file_text(&[0xff, 0xfe, b'<']), None);

        let dir = std::env::temp_dir().join(format!("maxhq-task-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(task_program(&dir), None);
        std::fs::write(task_file(&dir), &bytes).unwrap();
        assert_eq!(task_program(&dir), Some(PathBuf::from("C:\\é\\maxd.exe")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_asks_the_windows_launcher_to_start_again() {
        if cfg!(windows) {
            assert_eq!(RESTART_EXIT, 75);
        } else {
            assert_eq!(RESTART_EXIT, 0);
        }
    }

    #[test]
    fn the_windows_remote_restart_is_fixed() {
        assert_eq!(
            remote_restart_command_windows(),
            r#"cmd /c "%LOCALAPPDATA%\MaxHQ\bin\current\maxd.exe" service restart"#
        );
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
