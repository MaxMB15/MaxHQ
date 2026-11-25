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
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
/// `%LOCALAPPDATA%\MaxHQ\data\maxd.sock`, where no file is made: the path
/// names the named pipe `maxhq_os::ipc` serves, so each user gets their
/// own.
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
/// Windows it's `data` in [`windows_app_dir`], `%LOCALAPPDATA%\MaxHQ\data`:
/// the local folder rather than the roaming one, so a roaming profile
/// never syncs the database, and a folder of its own, so deleting the data
/// leaves the app the per-user setup installed beside it. In a guest it's
/// [`GUEST_DATA_DIR`].
pub fn data_dir() -> Option<PathBuf> {
    data_dir_for(Role::from_env())
}

fn data_dir_for(role: Role) -> Option<PathBuf> {
    if role == Role::Guest {
        return Some(GUEST_DATA_DIR.into());
    }
    if cfg!(windows) {
        return windows_app_dir().map(|dir| dir.join("data"));
    }
    let base = BaseDirs::new()?;
    Some(if cfg!(target_os = "macos") {
        base.data_dir().join("MaxHQ")
    } else {
        base.data_dir().join("maxhq")
    })
}

/// MaxHQ's own folder on Windows, `%LOCALAPPDATA%\MaxHQ`. The per-user
/// setup installs the app there, and it holds `maxd`'s builds, the
/// junction its service runs ([`MaxdLayout::windows`]), the service's
/// launcher and its logon task's file, with `maxd`'s data in `data`
/// ([`data_dir`]). `None` off Windows, where those live under the home
/// folder and the app elsewhere.
pub fn windows_app_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    BaseDirs::new().map(|base| base.data_local_dir().join("MaxHQ"))
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

/// Where `maxd`'s builds live, and the link its service runs the current
/// one through. An update copies a build into a folder of its own and
/// moves the link, so nothing ever runs half of one, and a running build
/// is never replaced.
///
/// - On Unix, under the home folder: `~/.local/lib/maxhq/versions/<build>/maxd`,
///   and `~/.local/bin/maxd`, a symbolic link to the binary.
/// - On Windows, in MaxHQ's folder, `%LOCALAPPDATA%\MaxHQ`
///   ([`windows_app_dir`]), beside the data rather than in it:
///   `versions\<build>\maxd.exe`, and `bin\current`, a junction to the
///   build's folder, since a symbolic link there needs admin rights or
///   developer mode. The service runs `bin\current\maxd.exe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaxdLayout {
    /// The folder with `versions` and the update record.
    lib: PathBuf,
    /// The link the service runs through.
    link: PathBuf,
    /// Whether the link is a junction to a build's folder rather than a
    /// symbolic link to its binary.
    junction: bool,
}

impl MaxdLayout {
    /// Unix's layout under the home folder `home`.
    pub fn new(home: impl AsRef<Path>) -> Self {
        let home = home.as_ref();
        Self {
            lib: home.join(".local/lib/maxhq"),
            link: home.join(".local/bin").join(MAXD_PROGRAM),
            junction: false,
        }
    }

    /// Windows's layout in `app`, MaxHQ's folder.
    pub fn windows(app: impl AsRef<Path>) -> Self {
        let app = app.as_ref();
        Self {
            lib: app.to_owned(),
            link: app.join("bin").join("current"),
            junction: true,
        }
    }

    /// This system's: Unix's under `HOME`, or Windows's in MaxHQ's folder.
    /// `None` without either.
    pub fn from_env() -> Option<Self> {
        if cfg!(windows) {
            return windows_app_dir().map(Self::windows);
        }
        maxhq_os::home::home_dir().map(Self::new)
    }

    /// The link the service runs through: `~/.local/bin/maxd`, or the
    /// `bin\current` junction.
    pub fn link(&self) -> PathBuf {
        self.link.clone()
    }

    /// The program the service runs: the link itself on Unix, and
    /// `maxd.exe` through the junction on Windows.
    pub fn program(&self) -> PathBuf {
        if self.junction {
            self.link.join(MAXD_PROGRAM)
        } else {
            self.link.clone()
        }
    }

    /// The folder with one folder per build.
    pub fn versions(&self) -> PathBuf {
        self.lib.join("versions")
    }

    /// The binary of `build`.
    pub fn version(&self, build: &str) -> PathBuf {
        self.versions().join(build).join(MAXD_PROGRAM)
    }

    /// What the link points at to run `build`: its binary, or on Windows
    /// its folder.
    pub fn link_target(&self, build: &str) -> PathBuf {
        if self.junction {
            self.versions().join(build)
        } else {
            self.version(build)
        }
    }

    /// The build whose [`Self::link_target`] is `target`, if it's one of
    /// these versions.
    pub fn build_of_target(&self, target: &Path) -> Option<String> {
        self.build_in(&self.versions(), target)
    }

    /// The build the link leads to now, if it leads into these versions.
    /// Both sides are resolved first, so a home folder reached through a
    /// symbolic link, or a verbatim Windows path, still matches.
    pub fn linked_build(&self) -> Option<String> {
        let target = fs::canonicalize(&self.link).ok()?;
        let versions = fs::canonicalize(self.versions()).ok()?;
        self.build_in(&versions, &target)
    }

    /// [`Self::build_of_target`], with the versions folder given.
    fn build_in(&self, versions: &Path, target: &Path) -> Option<String> {
        let folder = if self.junction {
            target
        } else {
            target.parent()?
        };
        if folder.parent()? != versions {
            return None;
        }
        folder.file_name()?.to_str().map(str::to_owned)
    }

    /// Removes every build but those in `keep`. Only folders named like a
    /// build go, so anything else kept beside them stays. Returns the
    /// builds it couldn't remove and why, for the caller to log: Windows
    /// won't remove a program that still runs, and the next update tries
    /// again.
    pub fn prune(&self, keep: &[&str]) -> Vec<(String, io::Error)> {
        let Ok(entries) = fs::read_dir(self.versions()) else {
            return Vec::new();
        };
        let mut failed = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !is_build(name) || keep.contains(&name) {
                continue;
            }
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                failed.push((name.to_owned(), error));
            }
        }
        failed
    }

    /// `update.json`, the record of the last update.
    pub fn state(&self) -> PathBuf {
        self.lib.join("update.json")
    }
}

/// `maxd`'s file name on this system.
pub const MAXD_PROGRAM: &str = if cfg!(windows) { "maxd.exe" } else { "maxd" };

/// Where [`MaxdLayout::windows`] puts the program the service runs, under
/// `%LOCALAPPDATA%`. It stays there as updates come, since an update moves
/// the junction to the build it installs. Key lines and remote commands
/// for a Windows device name it, as the device itself works it out.
pub const WINDOWS_MAXD: &str = r"MaxHQ\bin\current\maxd.exe";

/// How many hex characters of a binary's sha256 name its build, which is
/// also its folder's name among the versions.
pub const BUILD_LEN: usize = 16;

/// Whether `name` is a build's: [`BUILD_LEN`] hex characters.
fn is_build(name: &str) -> bool {
    name.len() == BUILD_LEN && name.bytes().all(|b| b.is_ascii_hexdigit())
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
        if cfg!(windows) {
            assert!(data.ends_with(Path::new("MaxHQ").join("data")));
        } else {
            assert!(data.ends_with("MaxHQ") || data.ends_with("maxhq"));
        }
    }

    #[test]
    fn windows_keeps_the_data_apart_from_the_programs() {
        if cfg!(windows) {
            let app = windows_app_dir().expect("a local app data folder");
            assert!(app.ends_with("MaxHQ"));
            assert_eq!(data_dir(), Some(app.join("data")));
            assert_eq!(runtime_dir(), data_dir());
            let layout = MaxdLayout::from_env().unwrap();
            assert!(!layout.versions().starts_with(data_dir().unwrap()));
            assert!(!layout.link().starts_with(data_dir().unwrap()));
        } else {
            assert_eq!(windows_app_dir(), None);
        }
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
    fn unix_runs_the_build_through_a_link_to_its_binary() {
        let layout = MaxdLayout::new("/home/<user>");
        let link = Path::new("/home/<user>/.local/bin").join(MAXD_PROGRAM);
        assert_eq!(layout.link(), link);
        assert_eq!(layout.program(), link);
        let versions = Path::new("/home/<user>/.local/lib/maxhq/versions");
        assert_eq!(layout.versions(), versions);
        assert_eq!(
            layout.version("0123"),
            versions.join("0123").join(MAXD_PROGRAM)
        );
        assert_eq!(layout.link_target("0123"), layout.version("0123"));
        assert_eq!(
            layout
                .build_of_target(&layout.link_target("0123"))
                .as_deref(),
            Some("0123")
        );
        assert_eq!(layout.build_of_target(Path::new("/usr/bin/maxd")), None);
        assert_eq!(
            layout.state(),
            Path::new("/home/<user>/.local/lib/maxhq/update.json")
        );
    }

    #[test]
    fn windows_runs_the_build_through_a_junction_to_its_folder() {
        let data = Path::new("C:/Users/<user>/AppData/Local/MaxHQ");
        let layout = MaxdLayout::windows(data);
        assert_eq!(
            layout.program(),
            Path::new("C:/Users/<user>/AppData/Local/MaxHQ/bin/current").join(MAXD_PROGRAM)
        );
        assert_eq!(layout.link(), data.join("bin").join("current"));
        assert_eq!(
            layout.program(),
            data.join("bin").join("current").join(MAXD_PROGRAM)
        );
        assert_eq!(layout.versions(), data.join("versions"));
        assert_eq!(
            layout.link_target("0123"),
            data.join("versions").join("0123")
        );
        assert_eq!(
            layout.version("0123"),
            data.join("versions").join("0123").join(MAXD_PROGRAM)
        );
        assert_eq!(
            layout
                .build_of_target(&layout.link_target("0123"))
                .as_deref(),
            Some("0123")
        );
        assert_eq!(layout.build_of_target(&data.join("bin")), None);
        assert_eq!(layout.state(), data.join("update.json"));
    }

    #[test]
    fn windows_maxd_is_the_program_of_the_windows_layout() {
        let local = Path::new("C:/Users/<user>/AppData/Local");
        let layout = MaxdLayout::windows(local.join("MaxHQ"));
        // `maxd.exe` is the program's name only when this runs on Windows.
        let program = layout.link().join("maxd.exe");
        let parts: Vec<&str> = program
            .strip_prefix(local)
            .unwrap()
            .iter()
            .map(|part| part.to_str().unwrap())
            .collect();
        assert_eq!(parts.join("\\"), WINDOWS_MAXD);
        if cfg!(windows) {
            assert_eq!(layout.program(), program);
        }
    }

    /// This system's layout in a folder of its own.
    fn layout_in(dir: &Path) -> MaxdLayout {
        if cfg!(windows) {
            MaxdLayout::windows(dir)
        } else {
            MaxdLayout::new(dir)
        }
    }

    /// Puts a build in `layout`, without pointing the link at it.
    fn add_build(layout: &MaxdLayout, build: &str) {
        let program = layout.version(build);
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(program, build).unwrap();
    }

    #[test]
    fn the_linked_build_is_the_one_the_link_leads_to() {
        let dir = crate::test_dir::TempDir::new("layout-linked");
        let layout = layout_in(&dir.0);
        let (one, two) = ("0123456789abcdef", "fedcba9876543210");
        add_build(&layout, one);
        add_build(&layout, two);
        assert_eq!(layout.linked_build(), None, "there's no link yet");

        fs::create_dir_all(layout.link().parent().unwrap()).unwrap();
        maxhq_os::link::point(&layout.link(), &layout.link_target(one)).unwrap();
        assert_eq!(layout.linked_build().as_deref(), Some(one));
        assert_eq!(fs::read_to_string(layout.program()).unwrap(), one);
        maxhq_os::link::point(&layout.link(), &layout.link_target(two)).unwrap();
        assert_eq!(layout.linked_build().as_deref(), Some(two));

        // A link that leads out of the versions names no build.
        let elsewhere = dir.0.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let target = if cfg!(windows) {
            elsewhere.clone()
        } else {
            fs::write(elsewhere.join("maxd"), "").unwrap();
            elsewhere.join("maxd")
        };
        maxhq_os::link::point(&layout.link(), &target).unwrap();
        assert_eq!(layout.linked_build(), None);
    }

    #[test]
    fn prune_keeps_what_it_is_told_and_what_isnt_a_build() {
        let dir = crate::test_dir::TempDir::new("layout-prune");
        let layout = layout_in(&dir.0);
        let builds = ["0123456789abcdef", "fedcba9876543210", "00000000000000ff"];
        for build in builds {
            add_build(&layout, build);
        }
        // A folder that only looks like a build, and one that is no build.
        let others = ["0123456789abcdeg", "notes"];
        for other in others {
            fs::create_dir_all(layout.versions().join(other)).unwrap();
        }

        let failed = layout.prune(&[builds[0], builds[2]]);
        assert!(failed.is_empty(), "{failed:?}");
        let mut left: Vec<String> = fs::read_dir(layout.versions())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [builds[2], builds[0], others[0], others[1]]);

        // Without a versions folder there's nothing to do.
        let empty = layout_in(&dir.0.join("empty"));
        assert!(empty.prune(&[]).is_empty());
    }

    #[test]
    fn a_build_is_named_by_sixteen_hex_characters() {
        assert!(is_build("0123456789abcdef"));
        assert!(is_build("0123456789ABCDEF"));
        assert!(!is_build("0123456789abcde"));
        assert!(!is_build("0123456789abcdef0"));
        assert!(!is_build("0123456789abcdeg"));
        assert!(!is_build(".incoming-1-2"));
    }

    #[test]
    fn this_system_has_its_layout() {
        let layout = MaxdLayout::from_env().expect("a home folder");
        if cfg!(windows) {
            assert_eq!(layout, MaxdLayout::windows(windows_app_dir().unwrap()));
        } else {
            assert_eq!(layout.program(), layout.link());
        }
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
