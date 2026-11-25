// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminal on Windows: a PowerShell window that runs a root fix, then
//! waits for Enter so you can read what it printed.
//!
//! A fix for another device runs `ssh -t <host> <command>`, and `sudo`
//! asks for the password on the other end. It opens without admin rights,
//! in PowerShell 7 when it's installed and Windows PowerShell otherwise,
//! and hands `ssh` its arguments on a command line built here, since
//! Windows PowerShell passes quotes inside an argument on wrong. A fix that
//! runs on this PC needs an administrator, so Windows PowerShell runs it
//! through the UAC prompt.
//!
//! The script goes to PowerShell encoded on its command line, so there's
//! no file for anything to change before an elevated window runs it.

use maxhq_app::host_fix::RootFix;
use maxhq_core::shell::powershell::{encoded, literal};

/// What PowerShell runs for a fix, and whether it needs an administrator.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    script: String,
    admin: bool,
}

/// The plan for `fix`: `ssh` to its host, or else its command as it is, as
/// an administrator.
fn plan(fix: &RootFix) -> Plan {
    match &fix.host {
        Some(host) => {
            let arguments = maxhq_os::proc::args::join_str(["-t", host, &fix.command]);
            let body = format!(
                "Start-Process -FilePath 'ssh' -ArgumentList {} -NoNewWindow -Wait",
                literal(&arguments)
            );
            Plan {
                script: script(&body),
                admin: false,
            }
        }
        None => Plan {
            script: script(&fix.command),
            admin: true,
        },
    }
}

/// The script for `body`. It runs `body` as a script block of its own, so
/// a mistake in it prints instead of closing the window before you read
/// it, then waits for Enter.
fn script(body: &str) -> String {
    format!(
        "try {{ & ([scriptblock]::Create({})) }} catch {{ Write-Host $_ -ForegroundColor Red }}\n\
         Write-Host ''\n\
         Read-Host 'Press Enter to close this window' | Out-Null\n",
        literal(body)
    )
}

/// The arguments that start PowerShell on `script`: without its logo or
/// your profile, which could change what runs, with scripts allowed, since
/// Windows refuses any script file the line runs by default, and the
/// script encoded as PowerShell takes it.
fn arguments(script: &str) -> [String; 6] {
    [
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        encoded(script),
    ]
}

/// Opens a PowerShell window on `fix`.
#[cfg(windows)]
pub async fn open(fix: &RootFix) -> Result<(), String> {
    let plan = plan(fix);
    let arguments = arguments(&plan.script).join(" ");
    let opened = tokio::task::spawn_blocking(move || {
        if plan.admin {
            elevated(&arguments)
        } else {
            plain(&arguments)
        }
    });
    opened.await.map_err(|error| error.to_string())?
}

/// Windows' own PowerShell, from the system folder.
#[cfg(windows)]
fn windows_powershell() -> std::path::PathBuf {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// Opens PowerShell 7, or Windows PowerShell without it, in a console
/// window of its own. It doesn't wait, since the window stays until you
/// press Enter.
#[cfg(windows)]
fn plain(arguments: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    /// Gives the program a console window of its own.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let program = maxhq_os::which::find("pwsh", &[]).unwrap_or_else(windows_powershell);
    std::process::Command::new(program)
        .raw_arg(arguments)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(drop)
        .map_err(|error| format!("PowerShell didn't open: {error}"))
}

/// Opens Windows PowerShell as an administrator, which shows the UAC
/// prompt. Returns once you've answered it.
#[cfg(windows)]
#[allow(unsafe_code)]
fn elevated(arguments: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{ShellExecuteW, SE_ERR_ACCESSDENIED};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide =
        |text: &std::ffi::OsStr| maxhq_os::win::wide(text).map_err(|error| error.to_string());
    let verb = wide("runas".as_ref())?;
    let program = wide(windows_powershell().as_os_str())?;
    let arguments = wide(arguments.as_ref())?;
    // SAFETY: each string is null-terminated UTF-16 that outlives the
    // call, and a null window and folder are allowed.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            program.as_ptr(),
            arguments.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as usize;
    match result {
        // Above 32 is success.
        33.. => Ok(()),
        code if code == SE_ERR_ACCESSDENIED as usize => {
            Err("PowerShell didn't open as an administrator: the prompt was declined".to_owned())
        }
        code => Err(format!(
            "PowerShell didn't open as an administrator: Windows said {code}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "sudo /home/<user>/.local/bin/maxd host apply --system";

    fn fix(host: Option<&str>, command: &str) -> RootFix {
        RootFix {
            host: host.map(str::to_owned),
            command: command.to_owned(),
        }
    }

    #[test]
    fn a_fix_for_another_device_runs_ssh_without_admin_rights() {
        let plan = plan(&fix(Some("devbox"), ROOT));
        assert!(!plan.admin);
        let body = format!(
            "Start-Process -FilePath 'ssh' -ArgumentList '-t devbox \"{ROOT}\"' -NoNewWindow -Wait"
        );
        assert_eq!(plan.script, script(&body));
    }

    #[test]
    fn ssh_gets_each_argument_whole() {
        // A double quote reaches ssh escaped for the C runtime, and the
        // single quote is doubled in the PowerShell literal around it all.
        let plan = plan(&fix(Some("dev@pc"), r#"echo "it's" \"#));
        let body = r#"Start-Process -FilePath 'ssh' -ArgumentList '-t dev@pc "echo \"it''s\" \\"' -NoNewWindow -Wait"#;
        assert_eq!(plan.script, script(body));
    }

    #[test]
    fn a_fix_for_this_pc_runs_as_it_is_as_an_administrator() {
        let command = "maxd host apply --system";
        assert_eq!(
            plan(&fix(None, command)),
            Plan {
                script: script(command),
                admin: true,
            }
        );
    }

    #[test]
    fn the_script_runs_the_line_as_a_block_then_waits_for_enter() {
        assert_eq!(
            script("Write-Host 'hi'"),
            "try { & ([scriptblock]::Create('Write-Host ''hi''')) } catch { Write-Host $_ -ForegroundColor Red }\n\
             Write-Host ''\n\
             Read-Host 'Press Enter to close this window' | Out-Null\n"
        );
    }

    #[test]
    fn powershell_gets_the_script_encoded() {
        assert_eq!(
            arguments("Write-Host é"),
            [
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                "VwByAGkAdABlAC0ASABvAHMAdAAgAOkA"
            ]
        );
    }
}
