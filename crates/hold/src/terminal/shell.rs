// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Shell integration: startup files that make zsh, bash and fish in the
//! drawer mark their prompts and commands, as `marks.rs` reads them, and
//! say their folder at each prompt. The user's own startup files run as
//! they would:
//!
//! - zsh starts with ZDOTDIR at a folder of MaxHQ's, whose `.zshenv`,
//!   `.zprofile`, `.zshrc` and `.zlogin` each run the user's with ZDOTDIR
//!   as the user had it. The hooks go in after the user's `.zshrc`.
//! - bash starts as `bash --init-file <file>`. The file does what a login
//!   shell would, `/etc/profile` and then the first of `~/.bash_profile`,
//!   `~/.bash_login` and `~/.profile`, then puts the hooks in
//!   `PROMPT_COMMAND` and the DEBUG trap around the user's.
//! - fish starts as `fish -l --init-command 'source <file>'`, which runs
//!   after the user's config.
//!
//! The marks carry a nonce for the terminal, which the shell gets in its
//! environment and keeps in a variable, out of what it starts. The files
//! go in a folder named by what they hold, so a new `maxd` never changes
//! the files a running shell reads.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::TermLaunch;
use crate::{files, secret};

/// The variable that gives the shell its nonce.
const NONCE: &str = "MAXHQ_SHELL_NONCE";

/// The variable that gives zsh the user's own ZDOTDIR.
const USER_ZDOTDIR: &str = "MAXHQ_USER_ZDOTDIR";

/// The files, by name.
const FILES: &[(&str, &str)] = &[
    (".zshenv", include_str!("shell/zshenv.zsh")),
    (".zprofile", include_str!("shell/zprofile.zsh")),
    (".zshrc", include_str!("shell/zshrc.zsh")),
    (".zlogin", include_str!("shell/zlogin.zsh")),
    ("bash.sh", include_str!("shell/bash.sh")),
    ("fish.fish", include_str!("shell/fish.fish")),
];

#[derive(Clone, Copy)]
enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    fn of(program: &Path) -> Option<Self> {
        match program.file_name()?.to_str()? {
            "zsh" => Some(Self::Zsh),
            "bash" => Some(Self::Bash),
            "fish" => Some(Self::Fish),
            _ => None,
        }
    }
}

/// Makes `launch`, a drawer shell, mark its prompts and commands when it's
/// zsh, bash or fish, with the files in a folder under `dir`. Returns
/// whether it will.
pub(super) fn integrate(launch: &mut TermLaunch, dir: &Path) -> io::Result<bool> {
    let Some(shell) = Shell::of(&launch.program).filter(|_| !launch.run) else {
        return Ok(false);
    };
    let folder = write_files(&std::path::absolute(dir)?)?;
    match shell {
        Shell::Zsh => {
            let user = env_value(launch, "ZDOTDIR").filter(|dir| !dir.is_empty());
            if let Some(user) = user {
                launch.env.push((USER_ZDOTDIR.into(), user));
            }
            launch.env.push(("ZDOTDIR".into(), folder.into()));
        }
        Shell::Bash => {
            launch.args = vec!["--init-file".into(), folder.join("bash.sh").into()];
        }
        Shell::Fish => {
            let Some(file) = folder.join("fish.fish").to_str().map(fish_quoted) else {
                return Ok(false);
            };
            launch.args = vec![
                "-l".into(),
                "--init-command".into(),
                format!("source {file}").into(),
            ];
        }
    }
    let nonce = secret::new_nonce()?;
    launch.env.push((NONCE.into(), nonce.clone().into()));
    launch.nonce = Some(nonce);
    Ok(true)
}

/// What `key` is for the program: as `launch` sets it, or as `maxd` has it.
fn env_value(launch: &TermLaunch, key: &str) -> Option<OsString> {
    (launch.env.iter().rev())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os(key))
}

/// `text` in single quotes, as fish reads them.
fn fish_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

/// The folder with the files under `dir`, written when it isn't there
/// yet.
fn write_files(dir: &Path) -> io::Result<PathBuf> {
    // FNV-1a over the names and contents.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (name, text) in FILES {
        for byte in [name.as_bytes(), &[0], text.as_bytes(), &[0]].concat() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    let folder = dir.join(format!("shell-{hash:016x}"));
    if folder.is_dir() {
        return Ok(folder);
    }
    files::private_dir_all(dir)?;
    let temp = dir.join(format!(".shell-{hash:016x}-{}", secret::new_nonce()?));
    files::private_dir(&temp)?;
    let written = FILES
        .iter()
        .try_for_each(|(name, text)| fs::write(temp.join(name), text))
        .and_then(|()| fs::rename(&temp, &folder));
    match written {
        Ok(()) => Ok(folder),
        Err(err) => {
            let _ = fs::remove_dir_all(&temp);
            // Another shell starting at the same time put it there first.
            if folder.is_dir() {
                Ok(folder)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::TerminalId;

    use super::*;

    fn launch(program: &str) -> TermLaunch {
        let mut launch = TermLaunch::shell(TerminalId::generate(), "/tmp".into(), 80, 24);
        launch.program = program.into();
        launch
    }

    fn scratch(name: &str) -> PathBuf {
        let id = TerminalId::generate().0.simple().to_string();
        std::env::temp_dir().join(format!("hold-{name}-{}", &id[20..]))
    }

    fn env<'a>(launch: &'a TermLaunch, key: &str) -> Option<&'a OsString> {
        (launch.env.iter())
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    #[test]
    fn each_shell_starts_with_its_files_and_a_nonce() {
        let dir = scratch("shell");
        let zsh = {
            let mut launch = launch("/bin/zsh");
            assert!(integrate(&mut launch, &dir).unwrap());
            launch
        };
        assert_eq!(zsh.args, [OsString::from("-l")]);
        let folder = PathBuf::from(env(&zsh, "ZDOTDIR").unwrap());
        for (name, text) in FILES {
            assert_eq!(fs::read_to_string(folder.join(name)).unwrap(), *text);
        }
        let nonce = zsh.nonce.clone().unwrap();
        assert_eq!(nonce.len(), 32);
        assert_eq!(env(&zsh, NONCE), Some(&OsString::from(&nonce)));

        let mut bash = launch("/usr/local/bin/bash");
        assert!(integrate(&mut bash, &dir).unwrap());
        assert_eq!(
            bash.args,
            [OsString::from("--init-file"), folder.join("bash.sh").into()]
        );
        assert_ne!(bash.nonce, zsh.nonce);

        let mut fish = launch("fish");
        assert!(integrate(&mut fish, &dir).unwrap());
        let source = format!("source '{}'", folder.join("fish.fish").display());
        assert_eq!(
            fish.args,
            [OsString::from("-l"), "--init-command".into(), source.into()]
        );
        // One folder, written once.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_programs_and_runs_start_as_they_were() {
        let dir = scratch("noshell");
        for mut launch in [
            launch("/bin/sh"),
            launch("/usr/bin/nu"),
            launch("/bin/zsh5"),
            TermLaunch::run(TerminalId::generate(), "/tmp".into(), "ls", 80, 24),
        ] {
            let was = launch.clone();
            assert!(!integrate(&mut launch, &dir).unwrap());
            assert_eq!(launch, was);
        }
        assert!(!dir.exists());
    }

    #[test]
    fn fish_reads_quotes_back_as_they_were() {
        assert_eq!(fish_quoted(r"/a b/it's\x"), r"'/a b/it\'s\\x'");
    }
}
