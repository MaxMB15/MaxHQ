// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Starting other programs from the app: editors, the terminal and the
//! opener for links and files.
//!
//! An AppImage starts the app with variables that point inside its own
//! mount, so GTK and GIO load the modules it carries. A program the app
//! starts would load those too, and an AppImage's GTK modules can crash a
//! newer editor or browser. Electron editors also read `APPIMAGE` to decide
//! they are one, and would look for updates in MaxHQ's file. So the app
//! starts other programs with those variables as they were before the
//! AppImage set them. Outside an AppImage nothing changes.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::LazyLock;

/// The variables the AppImage runtime adds, which mean nothing to another
/// program.
const ADDED: [&str; 4] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

/// A variable's new value, or None to remove it.
type Change = (OsString, Option<OsString>);

/// The changes that take a program out of the AppImage's environment,
/// worked out once, since the app's own environment doesn't change.
static OUTSIDE: LazyLock<Vec<Change>> = LazyLock::new(|| {
    if !cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_none() {
        return Vec::new();
    }
    match std::env::var_os("APPDIR") {
        Some(mount) if !mount.is_empty() => outside(std::env::vars_os(), Path::new(&mount)),
        _ => Vec::new(),
    }
});

/// A command for `program` that runs outside the AppImage's environment.
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    leave_appimage(command.as_std_mut());
    command
}

/// Sets `command` to run outside the AppImage's environment.
fn leave_appimage(command: &mut std::process::Command) {
    for (name, value) in OUTSIDE.iter() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// The changes that undo what an AppImage mounted at `mount` did to
/// `vars`. The runtime's own variables go. A variable that lists a folder
/// inside the mount, alone or among others, keeps only the rest, and goes
/// when nothing is left. Every other variable stays as it is.
fn outside(vars: impl IntoIterator<Item = (OsString, OsString)>, mount: &Path) -> Vec<Change> {
    vars.into_iter()
        .filter_map(|(name, value)| {
            if ADDED.iter().any(|added| name == *added) {
                return Some((name, None));
            }
            let entries: Vec<_> = std::env::split_paths(&value).collect();
            if !entries.iter().any(|entry| entry.starts_with(mount)) {
                return None;
            }
            let kept: Vec<_> = entries
                .into_iter()
                .filter(|entry| !entry.as_os_str().is_empty() && !entry.starts_with(mount))
                .collect();
            if kept.is_empty() {
                return Some((name, None));
            }
            // The entries came apart at the separator, so they join again.
            Some((name, Some(std::env::join_paths(kept).ok()?)))
        })
        .collect()
}

/// How long an opener has to fail before it counts as having taken the
/// target. `xdg-open` and the rest hand it over and end within this, or
/// stay with the app they started.
#[cfg(target_os = "linux")]
const TAKES: std::time::Duration = std::time::Duration::from_secs(1);

/// Opens `target`, a link or a file, in the app it belongs to, through the
/// first of the desktop's openers that takes it: `xdg-open`, then GIO's and
/// KDE's. One that fails at once hands it to the next. One still running
/// has taken it, and the runtime reaps it when it ends.
#[cfg(target_os = "linux")]
pub async fn open(target: &str) -> std::io::Result<()> {
    use std::process::Stdio;

    let mut failed = std::io::Error::new(std::io::ErrorKind::NotFound, "no opener found");
    for opener in open::commands(target) {
        let mut opener = tokio::process::Command::from(opener);
        leave_appimage(opener.as_std_mut());
        let started = opener
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match started {
            Ok(child) => child,
            Err(error) => {
                failed = error;
                continue;
            }
        };
        match tokio::time::timeout(TAKES, child.wait()).await {
            Err(_) => return Ok(()),
            Ok(Ok(status)) if status.success() => return Ok(()),
            Ok(Ok(status)) => {
                failed = std::io::Error::other(format!(
                    "{} ended with {status}",
                    opener.as_std().get_program().to_string_lossy()
                ));
            }
            Ok(Err(error)) => failed = error,
        }
    }
    Err(failed)
}

/// Opens `target`, a link or a file, in the app it belongs to, through the
/// shell.
#[cfg(windows)]
pub async fn open(target: &str) -> std::io::Result<()> {
    let target = target.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        tauri_plugin_opener::open_url(target, None::<&str>).map_err(std::io::Error::other)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNT: &str = "/tmp/.mount_MaxHQab12";

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn changes(pairs: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
        outside(vars(pairs), Path::new(MOUNT))
            .into_iter()
            .map(|(name, value)| {
                (
                    name.into_string().unwrap(),
                    value.map(|value| value.into_string().unwrap()),
                )
            })
            .collect()
    }

    #[test]
    fn the_runtimes_own_variables_go() {
        assert_eq!(
            changes(&[
                ("APPDIR", MOUNT),
                ("APPIMAGE", "/home/<user>/Applications/MaxHQ.AppImage"),
                ("ARGV0", "MaxHQ.AppImage"),
                ("OWD", "/home/<user>"),
            ]),
            [
                ("APPDIR".into(), None),
                ("APPIMAGE".into(), None),
                ("ARGV0".into(), None),
                ("OWD".into(), None),
            ]
        );
    }

    #[test]
    fn a_list_keeps_the_folders_outside_the_mount() {
        assert_eq!(
            changes(&[
                (
                    "XDG_DATA_DIRS",
                    "/tmp/.mount_MaxHQab12/usr/share:/usr/share:/usr/local/share:"
                ),
                ("PATH", "/tmp/.mount_MaxHQab12/usr/bin:/usr/bin:/bin"),
            ]),
            [
                (
                    "XDG_DATA_DIRS".into(),
                    Some("/usr/share:/usr/local/share".into())
                ),
                ("PATH".into(), Some("/usr/bin:/bin".into())),
            ]
        );
    }

    #[test]
    fn a_variable_only_inside_the_mount_goes() {
        assert_eq!(
            changes(&[
                ("GTK_DATA_PREFIX", MOUNT),
                (
                    "GDK_PIXBUF_MODULE_FILE",
                    "/tmp/.mount_MaxHQab12/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
                ),
                (
                    "GIO_MODULE_DIR",
                    "/tmp/.mount_MaxHQab12/usr/lib/gio/modules"
                ),
            ]),
            [
                ("GTK_DATA_PREFIX".into(), None),
                ("GDK_PIXBUF_MODULE_FILE".into(), None),
                ("GIO_MODULE_DIR".into(), None),
            ]
        );
    }

    #[test]
    fn everything_else_stays_as_it_was() {
        assert!(changes(&[
            ("DISPLAY", ":0"),
            ("HOME", "/home/<user>"),
            ("PATH", "/usr/bin::/bin"),
            // A folder whose name only starts like the mount's.
            ("FOLDER", "/tmp/.mount_MaxHQab123/x"),
            ("TERMINAL", "kitty"),
        ])
        .is_empty());
    }
}
