// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where `maxd` keeps its socket, its database and its holders. `maxd` and
//! every client on the device compute the same paths, so `maxd proxy` finds
//! the socket without being told.
//!
//! The guest service inside a workspace container runs as root and keeps
//! its files in system folders instead. [`ROLE_ENV`] set to `guest` picks
//! those.

use std::env;
use std::path::PathBuf;

use directories::BaseDirs;

/// Overrides the socket path, for tests and for running two `maxd`s side
/// by side.
pub const SOCKET_ENV: &str = "MAXD_SOCKET";

/// Says which part this process plays. `guest` makes it the guest service,
/// and anything else, or nothing, makes it `maxd` on a device.
pub const ROLE_ENV: &str = "MAXHQ_ROLE";

/// The name the new `maxd` symlink gets next to `~/.local/bin/maxd` before
/// a rename puts it in place. Setup and updates both use it.
pub const MAXD_LINK_TEMP: &str = "maxd.maxhq-new";

/// The guest's folder for files that outlive a restart.
pub const GUEST_DATA_DIR: &str = "/var/lib/maxhq";

/// The guest's folder for its sockets.
pub const GUEST_RUNTIME_DIR: &str = "/run/maxhq";

/// The guest's socket for the host's link, which the container's proxy
/// device connects to. Root owns it, with mode 0600.
pub const GUEST_HOST_SOCKET: &str = "/run/maxhq/host.sock";

/// The guest's socket for the token and credential helpers the agent runs.
pub const GUEST_AGENT_SOCKET: &str = "/run/maxhq/agent.sock";

/// Which part this process plays, which picks its folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// `maxd` on a device, running as the device's user.
    Device,
    /// The guest service inside a workspace container, running as root.
    Guest,
}

impl Role {
    /// The role [`ROLE_ENV`] names.
    pub fn from_env() -> Self {
        match env::var_os(ROLE_ENV) {
            Some(role) if role == "guest" => Self::Guest,
            _ => Self::Device,
        }
    }
}

/// The local endpoint `maxd` listens on, a Unix socket at this path.
///
/// On macOS it's `~/Library/Application Support/MaxHQ/maxd.sock`. On Linux
/// it's `$XDG_RUNTIME_DIR/maxhq/maxd.sock`, falling back to
/// `/run/user/<uid>/maxhq/maxd.sock` when a service starts without the
/// variable set. In a guest it's `/run/maxhq/maxd.sock`. On Windows it's
/// `%LOCALAPPDATA%\MaxHQ\maxd.sock`, where no file is made: the path names
/// the named pipe `maxhq_os::ipc` serves, so each user gets their own.
pub fn socket() -> Option<PathBuf> {
    if let Some(path) = env::var_os(SOCKET_ENV) {
        return Some(path.into());
    }
    runtime_dir().map(|dir| dir.join("maxd.sock"))
}

/// The folder for `maxd`'s database and other files that outlive a restart.
///
/// On macOS it's `~/Library/Application Support/MaxHQ`, and on Linux
/// `$XDG_DATA_HOME/maxhq`, which is usually `~/.local/share/maxhq`. On
/// Windows it's `%LOCALAPPDATA%\MaxHQ`, the local folder rather than the
/// roaming one, so a roaming profile never syncs the database. In a guest
/// it's [`GUEST_DATA_DIR`].
pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(Role::from_env())
}

fn data_dir_for(role: Role) -> Option<PathBuf> {
    if role == Role::Guest {
        return Some(GUEST_DATA_DIR.into());
    }
    let base = BaseDirs::new()?;
    Some(if cfg!(target_os = "macos") {
        base.data_dir().join("MaxHQ")
    } else if cfg!(windows) {
        base.data_local_dir().join("MaxHQ")
    } else {
        base.data_dir().join("maxhq")
    })
}

/// The folder with one folder per held agent session, named by its session
/// ID. Each holds what the holder records: the launch, the output log, the
/// pending prompts and how the agent exited.
///
/// It's `hold` under [`data_dir`], so a session's record survives a reboot.
pub fn hold_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("hold"))
}

/// The folder for the holders' sockets, beside [`socket`], so setting
/// `MAXD_SOCKET` moves both.
///
/// macOS limits a socket path to 104 bytes and the folder there is
/// `~/Library/Application Support/MaxHQ/holders`, so holders name their
/// sockets with a short part of the session ID rather than all of it.
/// Linux allows 108, and its longest folder is
/// `/run/user/<uid>/maxhq/holders`.
pub fn hold_runtime_dir() -> Option<PathBuf> {
    let socket = socket()?;
    Some(socket.parent()?.join("holders"))
}

/// The folder for sockets and other files that end with the session.
///
/// On macOS and Windows it's [`data_dir`], since there's nothing better.
/// On Linux it's `$XDG_RUNTIME_DIR/maxhq`, or `/run/user/<uid>/maxhq`
/// without the variable. In a guest it's [`GUEST_RUNTIME_DIR`].
pub fn runtime_dir() -> Option<PathBuf> {
    runtime_dir_for(Role::from_env())
}

fn runtime_dir_for(role: Role) -> Option<PathBuf> {
    if role == Role::Guest {
        return Some(GUEST_RUNTIME_DIR.into());
    }
    if cfg!(any(target_os = "macos", windows)) {
        return data_dir_for(role);
    }
    if let Some(dir) = BaseDirs::new().and_then(|base| base.runtime_dir().map(PathBuf::from)) {
        return Some(dir.join("maxhq"));
    }
    linux_uid().map(|uid| PathBuf::from(format!("/run/user/{uid}/maxhq")))
}

#[cfg(target_os = "linux")]
fn linux_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    // `/proc/self` belongs to whoever runs this process.
    std::fs::metadata("/proc/self").ok().map(|meta| meta.uid())
}

#[cfg(not(target_os = "linux"))]
fn linux_uid() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Linux's limit on a socket path, counting the closing zero byte.
    const LINUX_SOCKET_MAX: usize = 108;

    /// The longest holder socket name: 12 hex digits and a suffix.
    const HOLDER_SOCKET: &str = "0123456789ab-99.sock";

    fn fits(path: &str) {
        assert!(
            path.len() < LINUX_SOCKET_MAX,
            "{path} is {} bytes",
            path.len()
        );
    }

    #[test]
    fn paths_end_where_clients_look() {
        let socket = socket().expect("a home folder");
        assert!(socket.ends_with("maxd.sock"));
        let data = data_dir().expect("a home folder");
        assert!(data.ends_with("MaxHQ") || data.ends_with("maxhq"));
    }

    #[test]
    fn holders_keep_their_records_and_sockets_apart() {
        let records = hold_dir().expect("a home folder");
        let sockets = hold_runtime_dir().expect("a home folder");
        assert_eq!(records, data_dir().expect("a home folder").join("hold"));
        assert_eq!(sockets.parent(), socket().expect("a home folder").parent());
        assert_ne!(records, sockets);
    }

    #[test]
    fn the_link_temp_is_the_one_os_link_makes() {
        let temp = maxhq_os::link::temp_for(Path::new("/home/<user>/.local/bin/maxd"));
        assert_eq!(temp.file_name(), Some(MAXD_LINK_TEMP.as_ref()));
    }

    #[test]
    fn the_guest_uses_system_folders() {
        assert_eq!(data_dir_for(Role::Guest), Some(GUEST_DATA_DIR.into()));
        assert_eq!(runtime_dir_for(Role::Guest), Some(GUEST_RUNTIME_DIR.into()));
        let runtime = PathBuf::from(GUEST_RUNTIME_DIR);
        for socket in [GUEST_HOST_SOCKET, GUEST_AGENT_SOCKET] {
            assert_eq!(PathBuf::from(socket).parent(), Some(runtime.as_path()));
        }
        assert_ne!(data_dir_for(Role::Device), data_dir_for(Role::Guest));
    }

    #[test]
    fn guest_sockets_fit() {
        fits(GUEST_HOST_SOCKET);
        fits(GUEST_AGENT_SOCKET);
        fits(&format!("{GUEST_RUNTIME_DIR}/holders/{HOLDER_SOCKET}"));
    }

    #[test]
    fn linux_device_sockets_fit_for_any_uid() {
        let runtime = format!("/run/user/{}/maxhq", u32::MAX);
        fits(&format!("{runtime}/maxd.sock"));
        fits(&format!("{runtime}/holders/{HOLDER_SOCKET}"));
        // A proxy device's socket, for an instance name as long as Incus
        // allows.
        fits(&format!("{runtime}/guests/{}.sock", "m".repeat(63)));
    }
}
