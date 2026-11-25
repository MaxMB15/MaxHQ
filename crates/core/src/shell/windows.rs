// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The script a PowerShell or cmd wrapper runs. On Windows an agent starts
//! each command as `pwsh.exe -NoProfile -Command <script>` or
//! `cmd.exe /c <script>`, and shows it as one line, with the program in
//! quotes when its path holds a space and the script quoted as `sh`
//! quotes a word.

/// The Windows shells that run a script given on their command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shell {
    PowerShell,
    Cmd,
}

/// What a word after the program does.
enum Flag {
    /// The script follows.
    Script,
    /// Changes nothing about the script.
    Switch,
    /// Takes the next word as its value.
    Option,
}

/// PowerShell's switches that change nothing about what a script runs.
const POWERSHELL_SWITCHES: &[&str] = &[
    "-noprofile",
    "-nop",
    "-nologo",
    "-noninteractive",
    "-noni",
    "-mta",
    "-sta",
];

/// PowerShell's options that take a value.
const POWERSHELL_OPTIONS: &[&str] = &[
    "-executionpolicy",
    "-ep",
    "-ex",
    "-inputformat",
    "-outputformat",
    "-windowstyle",
    "-workingdirectory",
    "-wd",
];

/// The shell `program` is, from its file name: `pwsh`, `powershell` or
/// `cmd`, with or without `.exe`, in any case.
fn shell_of(program: &str) -> Option<Shell> {
    let name = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "pwsh" | "powershell" => Some(Shell::PowerShell),
        "cmd" => Some(Shell::Cmd),
        _ => None,
    }
}

/// What `word` does after `shell`'s program. None for a word that isn't a
/// switch this reads, such as `-File` or `-EncodedCommand`, whose script
/// isn't on the line.
fn flag_of(shell: Shell, word: &str) -> Option<Flag> {
    let word = word.to_ascii_lowercase();
    match shell {
        Shell::PowerShell => match word.as_str() {
            "-command" | "-c" => Some(Flag::Script),
            _ if POWERSHELL_SWITCHES.contains(&word.as_str()) => Some(Flag::Switch),
            _ if POWERSHELL_OPTIONS.contains(&word.as_str()) => Some(Flag::Option),
            _ => None,
        },
        Shell::Cmd => match word.as_str() {
            "/c" => Some(Flag::Script),
            "/d" | "/s" | "/q" => Some(Flag::Switch),
            _ if ["/e:", "/f:", "/v:"]
                .iter()
                .any(|prefix| word.starts_with(prefix)) =>
            {
                Some(Flag::Switch)
            }
            _ => None,
        },
    }
}

/// Reads past the program's switches up to the one before the script.
/// None when a word on the way isn't a switch this reads.
fn to_script(shell: Shell, mut next: impl FnMut() -> Option<String>) -> Option<()> {
    loop {
        match flag_of(shell, &next()?)? {
            Flag::Script => return Some(()),
            Flag::Switch => {}
            Flag::Option => {
                next()?;
            }
        }
    }
}

/// The script `line` runs when it starts PowerShell with `-Command` or
/// cmd with `/c`. None for any other line.
pub fn windows_script(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    let shell = shell_of(&next_word(&mut rest)?)?;
    to_script(shell, || next_word(&mut rest))?;
    let script = rest.trim();
    if script.is_empty() {
        return None;
    }
    // A script quoted as one word is that word. Anything else is the
    // script as the shell joins it.
    let mut words = script;
    match (next_word(&mut words), words.trim().is_empty()) {
        (Some(word), true) => Some(word),
        _ => Some(script.to_owned()),
    }
}

/// [`windows_script`] for an argument list, such as
/// `["pwsh.exe", "-NoProfile", "-Command", "git status"]`. The words after
/// the script's switch are joined with spaces, as both shells join them.
pub fn windows_script_argv(argv: &[&str]) -> Option<String> {
    let (program, rest) = argv.split_first()?;
    let shell = shell_of(program)?;
    let mut words = rest.iter();
    to_script(shell, || words.next().map(|word| (*word).to_owned()))?;
    let script: Vec<&str> = words.copied().collect();
    if script.is_empty() {
        return None;
    }
    Some(script.join(" "))
}

/// The next word of `rest`, unquoted, moving `rest` past it. Quotes group
/// as `sh` reads them, and `'\''` puts a quote in a single-quoted word,
/// but a backslash anywhere else stays, since it parts a Windows path.
fn next_word(rest: &mut &str) -> Option<String> {
    let text = rest.trim_start();
    if text.is_empty() {
        *rest = text;
        return None;
    }
    let mut word = String::new();
    let mut chars = text.char_indices().peekable();
    let mut end = text.len();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                end = at;
                break;
            }
            '\'' => word.extend(chars.by_ref().map(|(_, c)| c).take_while(|c| *c != '\'')),
            '"' => {
                while let Some((_, c)) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek().is_some_and(|(_, next)| *next == '"') => {
                            word.push('"');
                            chars.next();
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' if chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                word.push('\'');
                chars.next();
            }
            c => word.push(c),
        }
    }
    *rest = &text[end.min(text.len())..];
    Some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_gives_the_script_after_its_switches() {
        for line in [
            "pwsh.exe -NoProfile -Command 'git status'",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -Command 'git status'",
            r"'C:\Program Files\PowerShell\7\pwsh.exe' -NoLogo -NoProfile -c 'git status'",
            r#""C:\Program Files\PowerShell\7\PWSH.EXE" -ExecutionPolicy Bypass -Command "git status""#,
            "pwsh -NonInteractive -Command git status",
        ] {
            assert_eq!(
                windows_script(line).as_deref(),
                Some("git status"),
                "{line}"
            );
        }
        assert_eq!(
            windows_script(r"pwsh.exe -Command 'echo '\''hi'\'' > out.txt'").as_deref(),
            Some("echo 'hi' > out.txt")
        );
        assert_eq!(
            windows_script(r#"pwsh -Command "Get-Content src\main.rs""#).as_deref(),
            Some(r"Get-Content src\main.rs")
        );
    }

    #[test]
    fn cmd_gives_the_script_after_c() {
        assert_eq!(
            windows_script(r#"C:\Windows\system32\cmd.exe /d /s /c "dir /b""#).as_deref(),
            Some("dir /b")
        );
        assert_eq!(
            windows_script("CMD /C type notes.txt").as_deref(),
            Some("type notes.txt")
        );
        // Two quoted stretches stay as the shell reads them.
        assert_eq!(
            windows_script(r#"cmd /c "a.exe" && "b.exe""#).as_deref(),
            Some(r#""a.exe" && "b.exe""#)
        );
    }

    #[test]
    fn other_lines_are_no_wrapper() {
        for line in [
            "",
            "git status",
            "pwsh.exe",
            "pwsh.exe -NoProfile",
            "pwsh.exe -File build.ps1",
            "pwsh.exe -EncodedCommand ZABpAHIA",
            "pwsh.exe -Command",
            "cmd /k dir",
            "bash -lc 'git status'",
            "pwshx -Command 'git status'",
        ] {
            assert_eq!(windows_script(line), None, "{line}");
        }
    }

    #[test]
    fn an_argument_list_gives_its_script() {
        assert_eq!(
            windows_script_argv(&[
                r"C:\Program Files\PowerShell\7\pwsh.exe",
                "-NoProfile",
                "-Command",
                "git status",
            ])
            .as_deref(),
            Some("git status")
        );
        assert_eq!(
            windows_script_argv(&["cmd.exe", "/c", "dir", "/b"]).as_deref(),
            Some("dir /b")
        );
        assert_eq!(windows_script_argv(&["pwsh.exe", "-Command"]), None);
        assert_eq!(windows_script_argv(&["/bin/zsh", "-lc", "ls"]), None);
        assert_eq!(windows_script_argv(&[]), None);
    }
}
