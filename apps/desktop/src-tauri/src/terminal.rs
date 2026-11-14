// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Opens Terminal with one line to run, such as a host item's root fix.
//! The line goes into a `.command` file in the app's cache folder, which
//! Terminal runs in a new window and which removes itself as it starts.
//! That needs no permission to control Terminal, and you type any
//! password into Terminal itself, so the app never sees it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The script Terminal runs for `line`.
pub fn command_file(line: &str) -> String {
    format!("#!/bin/sh\nrm -f -- \"$0\"\n{line}\n")
}

/// Writes the script for `line` into `dir`, readable and runnable by you
/// alone, under a name nothing there has yet, and gives its path.
pub fn write_command_file(dir: &Path, line: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("maxhq-{}-{stamp}.command", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(&path)?;
    file.write_all(command_file(line).as_bytes())?;
    Ok(path)
}

/// Opens Terminal on `line`, through a script in `dir`.
pub async fn open(dir: PathBuf, line: String) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Terminal opens only on macOS".to_owned());
    }
    let path = tokio::task::spawn_blocking(move || write_command_file(&dir, &line))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("Couldn't ready the command for Terminal: {error}"))?;
    let status = tokio::process::Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(&path)
        .status()
        .await
        .map_err(|error| format!("Terminal didn't open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&path);
        Err(format!("Terminal didn't open: open ended with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "ssh -t dev@devbox 'sudo /home/dev/.local/bin/maxd host apply --system'";

    #[test]
    fn the_script_removes_itself_then_runs_the_line_as_given() {
        assert_eq!(
            command_file(LINE),
            format!("#!/bin/sh\nrm -f -- \"$0\"\n{LINE}\n")
        );
    }

    #[test]
    fn each_script_gets_its_own_file_for_you_alone() {
        let dir = std::env::temp_dir().join(format!("maxhq-terminal-{}", std::process::id()));
        let first = write_command_file(&dir, LINE).unwrap();
        let second = write_command_file(&dir, "true").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), command_file(LINE));
        assert_eq!(first.extension().unwrap(), "command");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
