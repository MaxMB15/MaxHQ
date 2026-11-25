// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminal on Windows: a PowerShell window that runs the line, then
//! waits for Enter so you can read what it printed.
//!
//! A line for another device is `ssh -t <host> <command>`, worded for a
//! POSIX shell, and `sudo` asks for the password on the other end. It
//! opens without admin rights, in PowerShell 7 when it's installed and
//! Windows PowerShell otherwise, and hands `ssh` its arguments on a
//! command line built here, since Windows PowerShell passes quotes inside
//! an argument on wrong. A line that runs on this PC needs an
//! administrator, as a root fix does, so Windows PowerShell runs it
//! through the UAC prompt.
//!
//! The script goes to PowerShell encoded on its command line, so there's
//! no file for anything to change before an elevated window runs it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// What PowerShell runs for a line, and whether it needs an
/// administrator.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    script: String,
    admin: bool,
}

/// The plan for `line`: `ssh` with the line's own arguments, or else the
/// line as it is, as an administrator.
fn plan(line: &str) -> Result<Plan, String> {
    if line.starts_with("ssh ") {
        let words =
            posix_words(line).ok_or_else(|| format!("The line has a quote left open: {line}"))?;
        let arguments: Vec<String> = words[1..].iter().map(|word| windows_arg(word)).collect();
        let body = format!(
            "Start-Process -FilePath 'ssh' -ArgumentList {} -NoNewWindow -Wait",
            quote(&arguments.join(" "))
        );
        return Ok(Plan {
            script: script(&body),
            admin: false,
        });
    }
    Ok(Plan {
        script: script(line),
        admin: true,
    })
}

/// The script for `body`. It runs `body` as a script block of its own, so
/// a mistake in it prints instead of closing the window before you read
/// it, then waits for Enter.
fn script(body: &str) -> String {
    format!(
        "try {{ & ([scriptblock]::Create({})) }} catch {{ Write-Host $_ -ForegroundColor Red }}\n\
         Write-Host ''\n\
         Read-Host 'Press Enter to close this window' | Out-Null\n",
        quote(body)
    )
}

/// `text` as a PowerShell string that holds it as it is: in single quotes,
/// with each single quote doubled, curly ones too, since PowerShell reads
/// those as quotes as well.
fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for each in text.chars() {
        if matches!(
            each,
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}'
        ) {
            quoted.push(each);
        }
        quoted.push(each);
    }
    quoted.push('\'');
    quoted
}

/// The words of `line` as a POSIX shell splits it: single quotes keep
/// everything, double quotes keep all but a backslash before `"`, `\`, `$`
/// or `` ` ``, and a backslash outside quotes keeps the next character.
/// `None` when a quote is left open.
fn posix_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(each) = chars.next() {
        match each {
            ' ' | '\t' | '\n' => words.extend(word.take()),
            '\'' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => word.push(other),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            kept @ ('"' | '\\' | '$' | '`') => word.push(kept),
                            '\n' => {}
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        },
                        other => word.push(other),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(other) => word.get_or_insert_default().push(other),
                None => word.get_or_insert_default().push('\\'),
            },
            other => word.get_or_insert_default().push(other),
        }
    }
    words.extend(word);
    Some(words)
}

/// `arg` on a Windows command line, as a program built with Microsoft's C
/// runtime reads it back: as it is when it has no space, tab or quote, and
/// otherwise in double quotes, with a quote inside escaped and the
/// backslashes before a quote, or before the closing one, doubled.
fn windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
        return arg.to_owned();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for each in arg.chars() {
        match each {
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
        if each != '\\' {
            quoted.push(each);
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// The arguments that start PowerShell on `script`: without its logo or
/// your profile, which could change what runs, with scripts allowed, since
/// Windows refuses any script file the line runs by default, and the
/// script encoded as PowerShell takes it, base64 of its UTF-16.
fn arguments(script: &str) -> [String; 6] {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    [
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        STANDARD.encode(bytes),
    ]
}

/// Opens a PowerShell window on `line`.
#[cfg(windows)]
pub async fn open(line: &str) -> Result<(), String> {
    let plan = plan(line)?;
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
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{ShellExecuteW, SE_ERR_ACCESSDENIED};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |text: &std::ffi::OsStr| text.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let verb = wide("runas".as_ref());
    let program = wide(windows_powershell().as_os_str());
    let arguments = wide(arguments.as_ref());
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

    #[test]
    fn a_line_for_another_device_runs_ssh_without_admin_rights() {
        let line = format!("ssh -t 'devbox' '{ROOT}'");
        let plan = plan(&line).unwrap();
        assert!(!plan.admin);
        let body = format!(
            "Start-Process -FilePath 'ssh' -ArgumentList '-t devbox \"{ROOT}\"' -NoNewWindow -Wait"
        );
        assert_eq!(plan.script, script(&body));
    }

    #[test]
    fn a_line_for_this_pc_runs_as_it_is_as_an_administrator() {
        let line = "maxd host apply --system";
        assert_eq!(
            plan(line),
            Ok(Plan {
                script: script(line),
                admin: true,
            })
        );
        assert!(plan("ssh -t 'devbox' 'open").is_err());
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
    fn a_quoted_string_keeps_every_kind_of_quote() {
        assert_eq!(quote("it's"), "'it''s'");
        assert_eq!(quote("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
        assert_eq!(quote("$HOME `n \"x\""), "'$HOME `n \"x\"'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn the_words_come_back_as_a_posix_shell_reads_them() {
        let words = |line: &str| posix_words(line).map(|words| words.join("|"));
        // As the host check words a command with a quote in it.
        assert_eq!(
            words(r"ssh -t 'devbox' 'echo '\''hi'\'''").as_deref(),
            Some("ssh|-t|devbox|echo 'hi'")
        );
        assert_eq!(
            words(r#"a "b \"c\" \x $d" e\ f  ''"#).as_deref(),
            Some(r#"a|b "c" \x $d|e f|"#)
        );
        assert_eq!(words("  "), Some(String::new()));
        assert_eq!(words("'open"), None);
        assert_eq!(words("\"open"), None);
    }

    #[test]
    fn an_argument_reads_back_as_it_was_on_a_windows_command_line() {
        assert_eq!(windows_arg("plain"), "plain");
        assert_eq!(windows_arg(r"C:\path\"), r"C:\path\");
        assert_eq!(windows_arg(""), r#""""#);
        assert_eq!(windows_arg("two words"), r#""two words""#);
        assert_eq!(windows_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_arg(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(windows_arg(r"end \"), r#""end \\""#);
        assert_eq!(windows_arg(r"in\between x"), r#""in\between x""#);
    }

    #[test]
    fn powershell_gets_the_script_as_base64_of_its_utf16() {
        // What [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...)) gives.
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
