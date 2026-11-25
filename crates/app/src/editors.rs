// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the Cursor and
// VS Code rows of the editor list, the macOS part of resolving an editor's
// command, the remote open URL, and the goto arguments for opening a file
// at a line, translated to Rust. The rest is MaxHQ's: which device a workspace opens on, and containers
// through MaxHQ's ssh alias.

//! The editors a workspace opens in. Each one is looked for once per run,
//! first in its own app in the Applications folders, since another editor
//! can install a command of the same name, as Cursor does with `code`. On
//! Linux its app is the folder its package installs, or an AppImage. Then
//! it's looked for on `PATH`, leaving out a command that leads into another
//! editor's app. On Windows its app is the folder its installer made, for
//! you or for everyone. A workspace on this machine opens
//! through the editor's command line, or its link when only the app is
//! there. On Windows it always opens through the link, since the command
//! lines there are batch files that only `cmd` can run. One on another device opens as a Remote-SSH link to it, and a
//! container's goes through MaxHQ's ssh alias for it.

use std::path::{Path, PathBuf};

use maxhq_os::which;
use maxhq_protocol::ids::WorkspaceId;
use maxhq_protocol::link::Route;
use maxhq_protocol::model::{Workspace, WorkspaceLocation};
use maxhq_protocol::state::{Entity, EntityKey};
use serde::Serialize;
use specta::Type;
use tokio::sync::OnceCell;

use crate::devices::Connector;
use crate::request::CommandError;
use crate::{ssh_config, App};

/// An editor MaxHQ can open a workspace in.
#[derive(Debug)]
pub struct EditorDef {
    /// What requests call it.
    pub id: &'static str,
    /// What the menu calls it.
    pub label: &'static str,
    /// Its command line's names, the usual one first.
    pub commands: &'static [&'static str],
    /// What every launch starts with.
    pub base_args: &'static [&'static str],
    /// How it takes a line and column.
    pub launch: LaunchStyle,
    /// The app's name in the Applications folder, without `.app`.
    pub mac_app: &'static str,
    /// The folders its Linux packages install it in, under `/usr/share` or
    /// `/opt`.
    pub linux_folders: &'static [&'static str],
    /// The folders its Windows installer puts it in, under
    /// `%LOCALAPPDATA%\Programs` when it's installed for you and
    /// `%ProgramFiles%` when it's for everyone.
    pub windows_folders: &'static [&'static str],
    /// How the file name of its AppImage starts, in lower case, for an
    /// editor that ships one.
    pub appimage: Option<&'static str>,
    /// Its Flatpak's app ID, which Flatpak also puts on a command folder as
    /// a command that runs it.
    pub flatpak: Option<&'static str>,
    /// The scheme of its links, which open a folder over Remote-SSH.
    pub remote_scheme: Option<&'static str>,
}

/// How an editor's command line takes a line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStyle {
    /// `--goto <path>:<line>:<column>`, as VS Code takes it.
    Goto,
}

/// The editors, in the order the menu lists them.
pub const EDITORS: &[EditorDef] = &[
    EditorDef {
        id: "cursor",
        label: "Cursor",
        commands: &["cursor"],
        // Opens go to the editor even when Cursor's agents window is up.
        base_args: &["--classic"],
        launch: LaunchStyle::Goto,
        mac_app: "Cursor",
        linux_folders: &["cursor", "Cursor"],
        windows_folders: &["cursor"],
        appimage: Some("cursor"),
        flatpak: None,
        remote_scheme: Some("cursor"),
    },
    EditorDef {
        id: "vscode",
        label: "VS Code",
        commands: &["code"],
        base_args: &[],
        launch: LaunchStyle::Goto,
        mac_app: "Visual Studio Code",
        linux_folders: &["code", "visual-studio-code"],
        windows_folders: &["Microsoft VS Code"],
        appimage: None,
        flatpak: Some("com.visualstudio.code"),
        remote_scheme: Some("vscode"),
    },
];

/// The editors this machine has, for the menu, and every one MaxHQ knows,
/// for the default editor setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct Editors {
    /// In the order the menu lists them.
    pub available: Vec<EditorChoice>,
    /// Every entry of [`EDITORS`], installed or not, in the same order.
    pub known: Vec<EditorChoice>,
}

/// One editor, as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct EditorChoice {
    /// What [`crate::Request::OpenInEditor`] calls it.
    pub id: String,
    /// What the menu calls it.
    pub label: String,
}

impl From<&EditorDef> for EditorChoice {
    fn from(editor: &EditorDef) -> Self {
        Self {
            id: editor.id.to_owned(),
            label: editor.label.to_owned(),
        }
    }
}

/// Where to look for editors.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// The folders on `PATH`, in order.
    pub path: Vec<PathBuf>,
    /// The folders apps are installed in, in the order they're tried.
    pub apps: Vec<PathBuf>,
    /// The folders that hold AppImages, which are each one file.
    pub appimages: Vec<PathBuf>,
    /// How the apps in them are laid out.
    pub layout: Layout,
}

/// How editors sit on disk, which differs by system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Layout {
    /// macOS: each editor is an app, `<mac_app>.app`, with its command line
    /// in `Contents/Resources/app/bin`.
    #[default]
    MacApps,
    /// Linux: each editor is a folder its package names, such as
    /// `/usr/share/code`, with its command line in `bin`, or in
    /// `resources/app/bin` when it was unpacked from an archive. Cursor
    /// also comes as an AppImage, one file that is both app and command
    /// line.
    Linux,
    /// Windows: each editor is a folder its installer names, such as
    /// `Microsoft VS Code`, with its command line, a `.cmd` file, in `bin`
    /// or `resources\app\bin`.
    Windows,
}

impl Layout {
    /// This system's. Linux and Windows have their own, and the rest look
    /// for apps as macOS does until they get theirs.
    fn here() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::MacApps
        }
    }

    /// The names `editor`'s app folder goes by.
    fn folders(self, editor: &EditorDef) -> Vec<String> {
        match self {
            Self::MacApps => vec![app_name(editor)],
            Self::Linux => editor
                .linux_folders
                .iter()
                .map(|&folder| folder.to_owned())
                .collect(),
            Self::Windows => editor
                .windows_folders
                .iter()
                .map(|&folder| folder.to_owned())
                .collect(),
        }
    }

    /// Where an app folder keeps its command line.
    fn bins(self) -> &'static [&'static str] {
        match self {
            Self::MacApps => &["Contents/Resources/app/bin"],
            Self::Linux | Self::Windows => &["bin", "resources/app/bin"],
        }
    }

    /// The names `editor`'s command line goes by in its app's folder.
    fn app_commands(self, editor: &EditorDef) -> Vec<String> {
        editor
            .commands
            .iter()
            .map(|&command| self.file_name(command))
            .collect()
    }

    /// The names `editor`'s command line goes by on `PATH`.
    fn commands(self, editor: &EditorDef) -> Vec<String> {
        let mut commands = self.app_commands(editor);
        if self == Self::Linux {
            commands.extend(editor.flatpak.map(str::to_owned));
        }
        commands
    }

    /// The file a command is in: `code.cmd` on Windows, where the editors
    /// install their command lines as batch files.
    fn file_name(self, command: &str) -> String {
        match self {
            Self::Windows => format!("{command}.cmd"),
            Self::MacApps | Self::Linux => command.to_owned(),
        }
    }

    /// Whether a workspace on this machine opens through the editor's link
    /// even when its command line is there. On Windows the command line is
    /// a batch file, which only `cmd` runs, with its own reading of the
    /// arguments, so the link is the safer way in.
    fn opens_by_link(self) -> bool {
        self == Self::Windows
    }
}

impl Places {
    /// This machine's: the absolute folders on its `PATH`, then where its
    /// system installs apps.
    pub fn of_this_machine() -> Self {
        let path = std::env::var_os("PATH")
            .map(|path| absolute_folders(&path))
            .unwrap_or_default();
        if cfg!(windows) {
            let folder = |name| {
                std::env::var_os(name)
                    .map(PathBuf::from)
                    .filter(|dir| dir.is_absolute())
            };
            return Self::on_windows(path, folder("LOCALAPPDATA"), folder("ProgramFiles"));
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        Self::on(Layout::here(), path, home, data_home)
    }

    /// The places `layout` looks in, after the folders on `PATH`. On macOS
    /// that's `/Applications` and `~/Applications`. On Linux it's
    /// `/usr/share` and `/opt` for packages, `~/Applications` and
    /// `~/AppImages` for AppImages, and Snap's and Flatpak's command
    /// folders after `PATH`, since an app started from the desktop doesn't
    /// always have them there.
    fn on(
        layout: Layout,
        mut path: Vec<PathBuf>,
        home: Option<PathBuf>,
        data_home: Option<PathBuf>,
    ) -> Self {
        match layout {
            Layout::MacApps => {
                let mut apps = vec![PathBuf::from("/Applications")];
                if let Some(home) = &home {
                    apps.push(home.join("Applications"));
                }
                Self {
                    path,
                    apps,
                    appimages: Vec::new(),
                    layout,
                }
            }
            Layout::Linux => {
                let data_home =
                    data_home.or_else(|| home.as_ref().map(|home| home.join(".local/share")));
                let commands = [
                    Some(PathBuf::from("/snap/bin")),
                    Some(PathBuf::from("/var/lib/flatpak/exports/bin")),
                    data_home.map(|data| data.join("flatpak/exports/bin")),
                ];
                for dir in commands.into_iter().flatten() {
                    if !path.contains(&dir) {
                        path.push(dir);
                    }
                }
                let appimages = home
                    .map(|home| vec![home.join("Applications"), home.join("AppImages")])
                    .unwrap_or_default();
                Self {
                    path,
                    apps: vec![PathBuf::from("/usr/share"), PathBuf::from("/opt")],
                    appimages,
                    layout,
                }
            }
            Layout::Windows => Self::on_windows(path, None, None),
        }
    }

    /// The places Windows looks in, after the folders on `PATH`: the
    /// installs for you in `local_app_data\Programs`, then the ones for
    /// everyone in `program_files`.
    fn on_windows(
        path: Vec<PathBuf>,
        local_app_data: Option<PathBuf>,
        program_files: Option<PathBuf>,
    ) -> Self {
        let apps = [
            local_app_data.map(|dir| dir.join("Programs")),
            program_files,
        ];
        Self {
            path,
            apps: apps.into_iter().flatten().collect(),
            appimages: Vec::new(),
            layout: Layout::Windows,
        }
    }

    /// Where `editor`'s app folder would be, in the order they're tried.
    fn bundles(&self, editor: &EditorDef) -> Vec<PathBuf> {
        let folders = self.layout.folders(editor);
        self.apps
            .iter()
            .flat_map(|root| folders.iter().map(|folder| root.join(folder)))
            .collect()
    }
}

/// The absolute folders on `path`. A relative one would depend on where the
/// app started.
fn absolute_folders(path: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .collect()
}

/// The editors this machine has, looked for the first time they're asked
/// about and kept for the rest of the run.
#[derive(Debug)]
pub struct Installed {
    places: Places,
    found: OnceCell<Vec<Found>>,
}

/// An editor that's installed.
#[derive(Debug)]
struct Found {
    editor: &'static EditorDef,
    /// Its command line, when one turned up. Without one it opens through
    /// its link.
    cli: Option<PathBuf>,
}

impl Installed {
    /// The editors in `places`.
    pub fn new(places: Places) -> Self {
        Self {
            places,
            found: OnceCell::new(),
        }
    }

    async fn found(&self) -> &[Found] {
        self.found
            .get_or_init(|| {
                let places = self.places.clone();
                async move {
                    tokio::task::spawn_blocking(move || find(&places))
                        .await
                        .expect("looking for editors doesn't panic")
                }
            })
            .await
    }
}

/// The editors in `places` that are installed.
fn find(places: &Places) -> Vec<Found> {
    let layout = places.layout;
    EDITORS
        .iter()
        .filter_map(|editor| {
            let bundles = places.bundles(editor);
            let commands = layout.app_commands(editor);
            let in_app = bundles.iter().find_map(|bundle| {
                layout
                    .bins()
                    .iter()
                    .flat_map(|bin| {
                        let bin = bundle.join(bin);
                        commands.iter().map(move |command| bin.join(command))
                    })
                    .find(|cli| which::runnable(cli))
            });
            let cli =
                in_app
                    .or_else(|| appimage(editor, &places.appimages))
                    .or_else(|| {
                        layout.commands(editor).iter().find_map(|command| {
                            places.path.iter().map(|dir| dir.join(command)).find(|cli| {
                                which::runnable(cli) && !in_other_app(editor, cli, places)
                            })
                        })
                    });
            let installed = cli.is_some() || bundles.iter().any(|bundle| bundle.is_dir());
            installed.then_some(Found { editor, cli })
        })
        .collect()
}

/// The folder name of `editor`'s app.
fn app_name(editor: &EditorDef) -> String {
    format!("{}.app", editor.mac_app)
}

/// The newest of `editor`'s AppImages in `dirs`, by when each last
/// changed, since an update often leaves the old file beside the new one.
fn appimage(editor: &EditorDef, dirs: &[PathBuf]) -> Option<PathBuf> {
    let prefix = editor.appimage?;
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (is_appimage_of(prefix, &path) && which::runnable(&path)).then_some(path)
        })
        .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
}

/// Whether `path` is named as an AppImage whose name starts with `prefix`,
/// in any case, as in `Cursor-1.2.3-x86_64.AppImage`.
fn is_appimage_of(prefix: &str, path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".appimage"))
}

/// Whether `cli`, once its links are followed, is inside an editor's app
/// other than `editor`'s. On macOS that's wherever the app is. On Linux
/// it's the other editor's package folders in `places`, or its AppImage.
fn in_other_app(editor: &EditorDef, cli: &Path, places: &Places) -> bool {
    let target = cli.canonicalize().unwrap_or_else(|_| cli.to_owned());
    EDITORS
        .iter()
        .filter(|other| other.id != editor.id)
        .any(|other| match places.layout {
            Layout::MacApps => {
                let app = app_name(other);
                target
                    .ancestors()
                    .any(|dir| dir.file_name() == Some(app.as_ref()))
            }
            Layout::Linux | Layout::Windows => {
                let in_folder = places.bundles(other).iter().any(|bundle| {
                    target.starts_with(bundle.canonicalize().unwrap_or_else(|_| bundle.clone()))
                });
                in_folder
                    || other
                        .appimage
                        .is_some_and(|prefix| is_appimage_of(prefix, &target))
            }
        })
}

/// Whether the shell may open `url`: what a page's link may open, or a link
/// to one of the editors.
pub fn url_allowed(url: &str) -> bool {
    if page_link_allowed(url) {
        return true;
    }
    let Some((scheme, _)) = url.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    EDITORS
        .iter()
        .any(|editor| editor.remote_scheme == Some(scheme.as_str()))
}

/// Whether a link clicked on a page may open: only a web page or a new email.
/// What a page shows can come from an agent or a pull request, so its links
/// can't reach an editor's scheme or any other app. The editor buttons go
/// through [`url_allowed`].
pub fn page_link_allowed(url: &str) -> bool {
    crate::phone::is_web(url)
        || url
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
}

/// Where a workspace opens.
#[derive(Debug, PartialEq, Eq)]
enum Reach {
    /// On this machine.
    Here,
    /// Over ssh to `host`.
    Ssh { host: String },
    /// In WSL distro `distro` on this Windows machine.
    Wsl { distro: String },
}

/// Where `workspace` opens, given how this machine reaches its device.
fn reach(workspace: &Workspace, route: Option<Route>) -> Result<Reach, CommandError> {
    if let WorkspaceLocation::Container { .. } = workspace.location {
        return match ssh_config::alias(workspace) {
            Some(host) => Ok(Reach::Ssh { host }),
            None => Err(failed("Containers can't open in an editor yet")),
        };
    }
    match route {
        Some(Route::Local) => Ok(Reach::Here),
        Some(Route::Ssh { host }) => Ok(Reach::Ssh { host }),
        Some(Route::Wsl { distro, via: None }) => Ok(Reach::Wsl { distro }),
        Some(Route::Wsl { via: Some(_), .. }) => Err(failed(
            "A WSL distro on another PC can't open in an editor from here yet",
        )),
        None => Err(failed("No route to this device")),
    }
}

/// `path` in the checkout at `root`. A full one is taken as it is.
fn in_checkout(root: &str, path: Option<String>) -> String {
    match path {
        None => root.to_owned(),
        Some(path) if is_full_path(&path) => path,
        Some(path) => format!("{}/{path}", root.trim_end_matches(['/', '\\'])),
    }
}

/// Whether `path` is a full path on the device that reported it, whichever
/// system that is: `/src/w`, or on Windows `C:\src\w`, `C:/src/w` or a
/// share's `\\server\w`. A path from another device can't be checked
/// with this machine's rules.
fn is_full_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("\\\\") || drive_letter(path).is_some()
}

/// The drive at the start of a full Windows path such as `C:\src`, with
/// its colon.
fn drive_letter(path: &str) -> Option<&str> {
    let bytes = path.as_bytes();
    let full = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    full.then(|| &path[..2])
}

/// The arguments that open `path` at `at`, a line and column, in `editor`.
fn launch_args(editor: &EditorDef, path: String, at: Option<(u32, u32)>) -> Vec<String> {
    let mut args: Vec<String> = editor.base_args.iter().map(|&arg| arg.to_owned()).collect();
    match (editor.launch, at) {
        (LaunchStyle::Goto, Some((line, column))) => {
            args.push("--goto".to_owned());
            args.push(format!("{path}:{line}:{column}"));
        }
        (LaunchStyle::Goto, None) => args.push(path),
    }
    args
}

/// The link that opens `path` on this machine in the editor with `scheme`.
fn file_url(scheme: &str, path: &str, at: Option<(u32, u32)>) -> String {
    format!("{scheme}://file{}{}", encode_path(path), position(at))
}

/// The link that opens `path` on `host` over Remote-SSH in the editor with
/// `scheme`. The host goes as it is, since ssh config fills in the user and
/// port. A file needs a position for the editor to open it as one, so it
/// gets the first line when it has none.
fn remote_url(scheme: &str, host: &str, path: &str, file: bool, at: Option<(u32, u32)>) -> String {
    let remote = format!("ssh-remote+{}", encode_component(host));
    remote_link(scheme, &remote, path, file, at)
}

/// The link that opens `path` in WSL distro `distro` on this machine, as
/// [`remote_url`] does over SSH.
fn wsl_url(scheme: &str, distro: &str, path: &str, file: bool, at: Option<(u32, u32)>) -> String {
    let remote = format!("wsl+{}", encode_component(distro));
    remote_link(scheme, &remote, path, file, at)
}

/// The link that opens `path` on the editor's `remote`.
fn remote_link(
    scheme: &str,
    remote: &str,
    path: &str,
    file: bool,
    at: Option<(u32, u32)>,
) -> String {
    let at = at.or(file.then_some((1, 1)));
    format!(
        "{scheme}://vscode-remote/{remote}{}{}",
        encode_path(path),
        position(at)
    )
}

/// `:line:column`, or nothing.
fn position(at: Option<(u32, u32)>) -> String {
    at.map(|(line, column)| format!(":{line}:{column}"))
        .unwrap_or_default()
}

/// An absolute path for a link, each part encoded. A Windows path keeps
/// its drive as it is and goes with forward slashes, as in `/C:/src/w`,
/// the way the editors write it.
fn encode_path(path: &str) -> String {
    if let Some(drive) = drive_letter(path) {
        let rest = path[drive.len()..].replace('\\', "/");
        return format!("/{drive}{}", encode_path(&rest));
    }
    let rooted = path.strip_prefix('/').unwrap_or(path);
    rooted
        .split('/')
        .map(|part| format!("/{}", encode_component(part)))
        .collect()
}

/// `text` with everything but letters, digits and `-_.!~*'()` escaped, as
/// JavaScript's `encodeURIComponent` does.
fn encode_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn failed(message: &str) -> CommandError {
    CommandError::Failed {
        message: message.to_owned(),
    }
}

impl<C: Connector> App<C> {
    /// The editors this machine has, and every one MaxHQ knows.
    pub async fn list_editors(&self) -> Editors {
        let available = self
            .editors
            .found()
            .await
            .iter()
            .map(|found| found.editor.into())
            .collect();
        let known = EDITORS.iter().map(EditorChoice::from).collect();
        Editors { available, known }
    }

    /// Opens `workspace` in `editor`, or the file at `path` in it, at
    /// `line` and `column` when they're given. A relative path is in the
    /// workspace's checkout.
    pub async fn open_in_editor(
        &self,
        editor: &str,
        workspace: WorkspaceId,
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<(), CommandError> {
        let found = self
            .editors
            .found()
            .await
            .iter()
            .find(|found| found.editor.id == editor);
        let Some(found) = found else {
            return Err(match EDITORS.iter().find(|known| known.id == editor) {
                Some(known) => CommandError::Failed {
                    message: format!("{} isn't installed on this computer", known.label),
                },
                None => CommandError::NotFound {
                    what: "This editor".to_owned(),
                },
            });
        };
        let workspace =
            self.view.read(
                |merge| match merge.store().get(&EntityKey::Workspace(workspace)) {
                    Some(Entity::Workspace(found)) => Ok(found.clone()),
                    _ => Err(CommandError::NotFound {
                        what: "This workspace".to_owned(),
                    }),
                },
            )?;
        let reach = reach(&workspace, self.devices.route(workspace.device))?;
        let root = match &workspace.location {
            WorkspaceLocation::Folder { path } | WorkspaceLocation::Container { path, .. } => path,
        };
        let file = path.is_some();
        let path = in_checkout(root, path);
        // A device reports the folder, and an editor's command line would read
        // anything but a full path as an option, such as one to install an extension.
        if matches!(reach, Reach::Here) && !is_full_path(&path) {
            return Err(failed("This workspace's folder isn't a full path"));
        }
        let at = line.map(|line| (line, column.unwrap_or(1)));
        let scheme = found.editor.remote_scheme;
        let by_link = self.editors.places.layout.opens_by_link() && scheme.is_some();
        let opening = match (reach, &found.cli, scheme) {
            (Reach::Here, Some(cli), _) if !by_link => self
                .shell
                .spawn_editor(cli.clone(), launch_args(found.editor, path, at)),
            (Reach::Here, _, Some(scheme)) => self.shell.open_url(file_url(scheme, &path, at)),
            (Reach::Ssh { host }, _, Some(scheme)) => self
                .shell
                .open_url(remote_url(scheme, &host, &path, file, at)),
            (Reach::Wsl { distro }, _, Some(scheme)) => self
                .shell
                .open_url(wsl_url(scheme, &distro, &path, file, at)),
            (_, _, None) => {
                return Err(CommandError::Failed {
                    message: format!("{} can't open this workspace", found.editor.label),
                })
            }
        };
        opening
            .await
            .map_err(|message| CommandError::Failed { message })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use maxhq_protocol::ids::{DeviceId, LinkId};

    use super::*;
    use crate::devices::tests::{two_devices, Harness};
    use crate::tests::{block_on, FakeShell, Opened};
    use maxhq_client::LinkEvent;

    #[cfg(unix)]
    #[test]
    fn relative_folders_on_path_are_left_out() {
        assert_eq!(
            absolute_folders(std::ffi::OsStr::new("bin:/usr/bin:.::../up:/opt/bin")),
            [PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn relative_folders_on_path_are_left_out() {
        assert_eq!(
            absolute_folders(std::ffi::OsStr::new(
                r"bin;C:\Windows;.;;..\up;\tools;D:\tools"
            )),
            [PathBuf::from(r"C:\Windows"), PathBuf::from(r"D:\tools")]
        );
    }

    /// A folder for one test's fake editors, gone when it drops.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-editors-{}", LinkId::generate()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// Puts a program at `path` under the folder, or a plain file when
        /// `runs` is false, and gives where it went. Windows has no run
        /// bit, so there every file at a command's name runs.
        #[cfg_attr(not(unix), allow(unused_variables))]
        pub(crate) fn program(&self, path: &str, runs: bool) -> PathBuf {
            let program = self.0.join(path);
            fs::create_dir_all(program.parent().unwrap()).unwrap();
            fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if runs { 0o755 } else { 0o644 };
                fs::set_permissions(&program, fs::Permissions::from_mode(mode)).unwrap();
            }
            program
        }

        /// Puts a link at `path` under the folder to `target`, as a
        /// command line's installer does, and gives where it went.
        #[cfg(unix)]
        fn link(&self, path: &str, target: &Path) -> PathBuf {
            let link = self.0.join(path);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, &link).unwrap();
            link
        }

        /// Places with `bin` as the whole `PATH`, and `Applications` then
        /// `home/Applications` for apps.
        pub(crate) fn places(&self) -> Places {
            Places {
                path: vec![self.0.join("bin")],
                apps: vec![
                    self.0.join("Applications"),
                    self.0.join("home/Applications"),
                ],
                ..Places::default()
            }
        }

        /// Places laid out as on Linux: `bin` as the whole `PATH`,
        /// `usr/share` then `opt` for packages, and `home/Applications` for
        /// AppImages.
        fn linux_places(&self) -> Places {
            Places {
                path: vec![self.0.join("bin")],
                apps: vec![self.0.join("usr/share"), self.0.join("opt")],
                appimages: vec![self.0.join("home/Applications")],
                layout: Layout::Linux,
            }
        }

        /// Places laid out as on Windows: `bin` as the whole `PATH`, then
        /// `Local/Programs` for the installs for you and `Program Files`
        /// for the ones for everyone.
        fn windows_places(&self) -> Places {
            Places {
                path: vec![self.0.join("bin")],
                ..Places::on_windows(
                    Vec::new(),
                    Some(self.0.join("Local")),
                    Some(self.0.join("Program Files")),
                )
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ids(found: &[Found]) -> Vec<(&str, Option<&Path>)> {
        found
            .iter()
            .map(|found| (found.editor.id, found.cli.as_deref()))
            .collect()
    }

    #[test]
    fn an_editor_is_found_in_either_applications_folder_then_on_path() {
        let scratch = Scratch::new();
        assert!(ids(&find(&scratch.places())).is_empty());

        // An app alone opens through its link.
        fs::create_dir_all(scratch.0.join("home/Applications/Cursor.app")).unwrap();
        assert_eq!(ids(&find(&scratch.places())), [("cursor", None)]);

        let on_path = scratch.program("bin/cursor", true);
        assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*on_path))]);
        let in_home = scratch.program(
            "home/Applications/Cursor.app/Contents/Resources/app/bin/cursor",
            true,
        );
        assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*in_home))]);
        let in_apps = scratch.program(
            "Applications/Cursor.app/Contents/Resources/app/bin/cursor",
            true,
        );
        assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*in_apps))]);

        // A file that doesn't run isn't a command line. Windows has no run
        // bit to leave off.
        if cfg!(unix) {
            scratch.program("bin/code", false);
            assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*in_apps))]);
        }
        let code = scratch.program(
            "Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
            true,
        );
        assert_eq!(
            ids(&find(&scratch.places())),
            [("cursor", Some(&*in_apps)), ("vscode", Some(&*code))]
        );
    }

    /// Cursor's app as its installer leaves it, with the `cursor` and
    /// `code` it links onto `PATH`.
    #[cfg(unix)]
    fn install_cursor(scratch: &Scratch) -> PathBuf {
        let bin = "Applications/Cursor.app/Contents/Resources/app/bin";
        let cursor = scratch.program(&format!("{bin}/cursor"), true);
        let code = scratch.program(&format!("{bin}/code"), true);
        scratch.link("bin/cursor", &code);
        scratch.link("bin/code", &code);
        cursor
    }

    #[cfg(unix)]
    #[test]
    fn a_code_command_that_leads_into_cursor_is_not_vs_code() {
        let scratch = Scratch::new();
        let cursor = install_cursor(&scratch);
        assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*cursor))]);

        // Wherever Cursor's app is, and through a link to a link.
        let elsewhere = Scratch::new();
        let code = elsewhere.program("Downloads/Cursor.app/Contents/Resources/app/bin/code", true);
        let hop = elsewhere.link("opt/code", &code);
        elsewhere.link("bin/code", &hop);
        assert!(ids(&find(&elsewhere.places())).is_empty());
    }

    #[test]
    fn vs_code_with_no_command_on_path_runs_the_one_in_its_app() {
        let scratch = Scratch::new();
        let code = scratch.program(
            "home/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
            true,
        );
        assert_eq!(ids(&find(&scratch.places())), [("vscode", Some(&*code))]);
    }

    #[cfg(unix)]
    #[test]
    fn with_both_editors_each_runs_its_own_command_line() {
        let scratch = Scratch::new();
        let cursor = install_cursor(&scratch);
        // VS Code's app without its command line opens through its link,
        // not through the `code` Cursor put on `PATH`.
        let vs_code = scratch.0.join("Applications/Visual Studio Code.app");
        fs::create_dir_all(&vs_code).unwrap();
        assert_eq!(
            ids(&find(&scratch.places())),
            [("cursor", Some(&*cursor)), ("vscode", None)]
        );

        let code = scratch.program(
            "Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
            true,
        );
        assert_eq!(
            ids(&find(&scratch.places())),
            [("cursor", Some(&*cursor)), ("vscode", Some(&*code))]
        );

        let (app, shell) = app_with(scratch.places());
        let w = WorkspaceId::derive("w");
        for editor in ["cursor", "vscode"] {
            block_on(app.open_in_editor(editor, w, None, None, None)).unwrap();
        }
        assert_eq!(
            *shell.1.lock().unwrap(),
            [
                Opened::Editor(cursor, vec!["--classic".into(), "/src/w".into()]),
                Opened::Editor(code, vec!["/src/w".into()]),
            ]
        );
    }

    #[test]
    fn without_either_app_a_command_on_path_is_taken_as_it_is() {
        let scratch = Scratch::new();
        assert!(ids(&find(&scratch.places())).is_empty());
        // As with a command line installed on its own, outside any app.
        let code = scratch.program("bin/code", true);
        let cursor = scratch.program("bin/cursor", true);
        assert_eq!(
            ids(&find(&scratch.places())),
            [("cursor", Some(&*cursor)), ("vscode", Some(&*code))]
        );
    }

    #[test]
    fn the_editors_are_looked_for_once_a_run() {
        let scratch = Scratch::new();
        let installed = Installed::new(scratch.places());
        assert!(block_on(installed.found()).is_empty());
        scratch.program("bin/code", true);
        assert!(block_on(installed.found()).is_empty());
        assert_eq!(find(&scratch.places()).len(), 1);
    }

    #[test]
    fn every_known_editor_is_listed_even_when_none_is_installed() {
        let (app, _) = app_with(Scratch::new().places());
        let editors = block_on(app.list_editors());
        assert!(editors.available.is_empty());
        let known: Vec<(&str, &str)> = editors
            .known
            .iter()
            .map(|e| (e.id.as_str(), e.label.as_str()))
            .collect();
        let all: Vec<(&str, &str)> = EDITORS.iter().map(|e| (e.id, e.label)).collect();
        assert_eq!(known, all);
        assert_eq!(known, [("cursor", "Cursor"), ("vscode", "VS Code")]);
    }

    #[test]
    fn linux_looks_in_package_snap_flatpak_and_appimage_folders() {
        let home = PathBuf::from("/home/<user>");
        let path = vec![PathBuf::from("/usr/bin"), PathBuf::from("/snap/bin")];
        let linux = Places::on(Layout::Linux, path.clone(), Some(home.clone()), None);
        // Snap's folder is already on this `PATH`, so it isn't added twice.
        assert_eq!(
            linux.path,
            [
                "/usr/bin",
                "/snap/bin",
                "/var/lib/flatpak/exports/bin",
                "/home/<user>/.local/share/flatpak/exports/bin",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(linux.apps, ["/usr/share", "/opt"].map(PathBuf::from));
        assert_eq!(
            linux.appimages,
            ["/home/<user>/Applications", "/home/<user>/AppImages"].map(PathBuf::from)
        );
        // A data folder of the user's own moves Flatpak's user installs.
        let moved = Places::on(
            Layout::Linux,
            Vec::new(),
            Some(home.clone()),
            Some(PathBuf::from("/data/<user>")),
        );
        assert_eq!(
            moved.path.last(),
            Some(&PathBuf::from("/data/<user>/flatpak/exports/bin"))
        );

        // macOS looks where it always has.
        let mac = Places::on(Layout::MacApps, path.clone(), Some(home), None);
        assert_eq!(mac.path, path);
        assert_eq!(
            mac.apps,
            ["/Applications", "/home/<user>/Applications"].map(PathBuf::from)
        );
        assert!(mac.appimages.is_empty());
    }

    #[test]
    fn on_linux_the_package_folder_comes_before_path() {
        let scratch = Scratch::new();
        assert!(ids(&find(&scratch.linux_places())).is_empty());

        // A package's folder alone opens through its link, as an app does.
        fs::create_dir_all(scratch.0.join("usr/share/code")).unwrap();
        assert_eq!(ids(&find(&scratch.linux_places())), [("vscode", None)]);

        let on_path = scratch.program("bin/code", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("vscode", Some(&*on_path))]
        );
        // An archive unpacked into `/opt` keeps its command line deeper in.
        let unpacked = scratch.program("opt/visual-studio-code/resources/app/bin/code", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("vscode", Some(&*unpacked))]
        );
        // The deb and rpm put it in `/usr/share/code/bin`.
        let packaged = scratch.program("usr/share/code/bin/code", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("vscode", Some(&*packaged))]
        );
        let cursor = scratch.program("usr/share/cursor/bin/cursor", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("cursor", Some(&*cursor)), ("vscode", Some(&*packaged))]
        );
    }

    #[test]
    fn on_linux_flatpak_runs_vs_code_by_its_app_id() {
        let scratch = Scratch::new();
        let flatpak = scratch.program("bin/com.visualstudio.code", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("vscode", Some(&*flatpak))]
        );
        // The plain name comes first when both are there.
        let code = scratch.program("bin/code", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("vscode", Some(&*code))]
        );

        // macOS has no Flatpak, so it doesn't look for the ID.
        let mac = Scratch::new();
        mac.program("bin/com.visualstudio.code", true);
        assert!(ids(&find(&mac.places())).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn on_linux_the_newest_cursor_appimage_runs_it() {
        use std::time::{Duration, SystemTime};

        let scratch = Scratch::new();
        let older = scratch.program("home/Applications/Cursor-1.6.0-x86_64.AppImage", true);
        let newer = scratch.program("home/Applications/cursor-1.7.2-x86_64.AppImage", true);
        // Neither a file that doesn't run nor one named otherwise counts.
        scratch.program("home/Applications/Cursor-1.8.0-x86_64.AppImage", false);
        scratch.program("home/Applications/Cursor-notes.txt", true);
        scratch.program("home/Applications/Other-1.0.0-x86_64.AppImage", true);
        let set_changed = |path: &Path, ago: u64| {
            let when = SystemTime::now() - Duration::from_secs(ago);
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };
        set_changed(&older, 60);
        set_changed(&newer, 30);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("cursor", Some(&*newer))]
        );
        set_changed(&older, 0);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("cursor", Some(&*older))]
        );

        // An AppImage comes before a command on `PATH`, as an app does.
        scratch.program("bin/cursor", true);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("cursor", Some(&*older))]
        );
    }

    #[cfg(unix)]
    #[test]
    fn on_linux_a_code_command_that_leads_into_cursor_is_not_vs_code() {
        let scratch = Scratch::new();
        let cursor = scratch.program("usr/share/cursor/bin/cursor", true);
        let code = scratch.program("usr/share/cursor/bin/code", true);
        scratch.link("bin/code", &code);
        assert_eq!(
            ids(&find(&scratch.linux_places())),
            [("cursor", Some(&*cursor))]
        );

        // Nor one that leads to Cursor's AppImage, wherever it is.
        let elsewhere = Scratch::new();
        let image = elsewhere.program("Downloads/Cursor-1.7.2-x86_64.AppImage", true);
        elsewhere.link("bin/code", &image);
        assert!(ids(&find(&elsewhere.linux_places())).is_empty());
    }

    #[test]
    fn windows_looks_in_the_installs_for_you_then_for_everyone() {
        let local = PathBuf::from(r"C:\Users\<user>\AppData\Local");
        let program_files = PathBuf::from(r"C:\Program Files");
        let path = vec![PathBuf::from(r"C:\Windows\System32")];
        let windows = Places::on_windows(
            path.clone(),
            Some(local.clone()),
            Some(program_files.clone()),
        );
        assert_eq!(windows.path, path);
        assert_eq!(windows.apps, [local.join("Programs"), program_files]);
        assert!(windows.appimages.is_empty());
        assert_eq!(windows.layout, Layout::Windows);
        // Without the folders, it still looks on `PATH`.
        assert!(Places::on_windows(path, None, None).apps.is_empty());
    }

    #[test]
    fn on_windows_each_installer_s_cmd_file_is_found() {
        let scratch = Scratch::new();
        assert!(ids(&find(&scratch.windows_places())).is_empty());

        // An install folder alone opens through its link.
        fs::create_dir_all(scratch.0.join("Program Files/Microsoft VS Code")).unwrap();
        assert_eq!(ids(&find(&scratch.windows_places())), [("vscode", None)]);

        // A command without `.cmd` isn't Windows' command line.
        scratch.program("bin/code", true);
        assert_eq!(ids(&find(&scratch.windows_places())), [("vscode", None)]);
        let on_path = scratch.program("bin/code.cmd", true);
        assert_eq!(
            ids(&find(&scratch.windows_places())),
            [("vscode", Some(&*on_path))]
        );
        // The install for everyone keeps it in `bin`, and one for you comes
        // first.
        let everyone = scratch.program("Program Files/Microsoft VS Code/bin/code.cmd", true);
        assert_eq!(
            ids(&find(&scratch.windows_places())),
            [("vscode", Some(&*everyone))]
        );
        let yours = scratch.program("Local/Programs/Microsoft VS Code/bin/code.cmd", true);
        assert_eq!(
            ids(&find(&scratch.windows_places())),
            [("vscode", Some(&*yours))]
        );
        // Cursor keeps it deeper in.
        let cursor = scratch.program("Local/Programs/cursor/resources/app/bin/cursor.cmd", true);
        assert_eq!(
            ids(&find(&scratch.windows_places())),
            [("cursor", Some(&*cursor)), ("vscode", Some(&*yours))]
        );
    }

    #[test]
    fn on_windows_the_code_cmd_in_cursor_s_folder_is_not_vs_code() {
        let scratch = Scratch::new();
        // Cursor's installer puts its own folder on `PATH`, with a `code`
        // beside its `cursor`.
        let bin = "Local/Programs/cursor/resources/app/bin";
        let cursor = scratch.program(&format!("{bin}/cursor.cmd"), true);
        scratch.program(&format!("{bin}/code.cmd"), true);
        let places = Places {
            path: vec![scratch.0.join(bin)],
            ..scratch.windows_places()
        };
        assert_eq!(ids(&find(&places)), [("cursor", Some(&*cursor))]);
    }

    #[test]
    fn on_windows_a_workspace_here_opens_through_the_link() {
        let scratch = Scratch::new();
        scratch.program("Local/Programs/cursor/resources/app/bin/cursor.cmd", true);
        scratch.program("Local/Programs/Microsoft VS Code/bin/code.cmd", true);
        let (app, shell) = app_with(scratch.windows_places());
        let w = WorkspaceId::derive("w");
        block_on(app.open_in_editor("cursor", w, None, None, None)).unwrap();
        block_on(app.open_in_editor("vscode", w, Some("a.rs".into()), Some(3), None)).unwrap();
        assert_eq!(
            *shell.1.lock().unwrap(),
            [
                Opened::Url("cursor://file/src/w".into()),
                Opened::Url("vscode://file/src/w/a.rs:3:1".into()),
            ]
        );
        assert!(shell.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_windows_path_is_full_and_goes_into_links_with_its_drive() {
        for path in [r"C:\src\w", "c:/src/w", r"\\server\w", "/src/w"] {
            assert!(is_full_path(path), "{path}");
        }
        for path in ["C:", "C:src", "src/w", "--install-extension=x", ""] {
            assert!(!is_full_path(path), "{path}");
        }
        assert_eq!(
            in_checkout(r"C:\src\w\", Some("a/b.rs".into())),
            r"C:\src\w/a/b.rs"
        );
        assert_eq!(
            in_checkout(r"C:\src\w", Some(r"D:\x.rs".into())),
            r"D:\x.rs"
        );
        assert_eq!(
            file_url("vscode", r"C:\Users\dev\my app\a.rs", Some((3, 7))),
            "vscode://file/C:/Users/dev/my%20app/a.rs:3:7"
        );
        assert_eq!(
            file_url("cursor", r"C:\src\w/a/b.rs", None),
            "cursor://file/C:/src/w/a/b.rs"
        );
        assert_eq!(
            remote_url("vscode", "<pc>", r"C:\src\w", false, None),
            "vscode://vscode-remote/ssh-remote+%3Cpc%3E/C:/src/w"
        );
    }

    #[test]
    fn goto_takes_the_line_and_column_after_the_base_args() {
        let [cursor, vscode] = EDITORS else {
            unreachable!("two editors")
        };
        assert_eq!(
            launch_args(cursor, "/src/a b.rs".into(), Some((3, 7))),
            ["--classic", "--goto", "/src/a b.rs:3:7"]
        );
        assert_eq!(
            launch_args(cursor, "/src/w".into(), None),
            ["--classic", "/src/w"]
        );
        // A colon in a path with no position stays part of the path.
        assert_eq!(launch_args(vscode, "/src/a:1".into(), None), ["/src/a:1"]);
        assert_eq!(
            launch_args(vscode, "/src/a.rs".into(), Some((1, 1))),
            ["--goto", "/src/a.rs:1:1"]
        );
    }

    #[test]
    fn links_encode_each_part_of_the_path_and_the_host() {
        assert_eq!(
            remote_url(
                "cursor",
                "dev@devbox",
                "/home/dev/my app/#1/é.rs",
                true,
                None
            ),
            "cursor://vscode-remote/ssh-remote+dev%40devbox/home/dev/my%20app/%231/%C3%A9.rs:1:1"
        );
        assert_eq!(
            remote_url("vscode", "devbox", "/src/w", false, None),
            "vscode://vscode-remote/ssh-remote+devbox/src/w"
        );
        assert_eq!(
            remote_url("vscode", "devbox", "/src/w/a.rs", true, Some((12, 4))),
            "vscode://vscode-remote/ssh-remote+devbox/src/w/a.rs:12:4"
        );
        assert_eq!(
            wsl_url("vscode", "Ubuntu-24.04", "/home/dev/w", false, None),
            "vscode://vscode-remote/wsl+Ubuntu-24.04/home/dev/w"
        );
        assert_eq!(
            file_url("cursor", "/Users/dev/it's (new)/a?.rs", Some((2, 1))),
            "cursor://file/Users/dev/it's%20(new)/a%3F.rs:2:1"
        );
        assert_eq!(encode_component("a-_.!~*'()z09"), "a-_.!~*'()z09");
        assert_eq!(encode_component("a/b:c+d&e"), "a%2Fb%3Ac%2Bd%26e");
    }

    #[test]
    fn a_page_link_only_reaches_the_web_or_mail() {
        for url in [
            "https://example.com/a",
            "HTTP://localhost:5173",
            "mailto:dev@example.com?subject=Hi",
            "MAILTO:dev@example.com",
        ] {
            assert!(page_link_allowed(url), "{url}");
        }
        for url in [
            "vscode://file/etc/passwd",
            "cursor://anysphere.cursor-deeplink/mcp/install?name=x",
            "file:///etc/passwd",
            "smb://host/share",
            "x-apple.systempreferences://",
            "javascript:alert(1)",
            "mailtox:dev@example.com",
            "mail",
        ] {
            assert!(!page_link_allowed(url), "{url}");
        }
    }

    #[test]
    fn only_web_and_editor_links_open() {
        for url in [
            "https://example.com",
            "http://localhost:5173",
            "cursor://vscode-remote/ssh-remote+devbox/src",
            "VSCode://file/src/a.rs",
            "mailto:dev@example.com",
        ] {
            assert!(url_allowed(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://devbox",
            "x-apple.systempreferences://",
            "cursor:",
            "",
        ] {
            assert!(!url_allowed(url), "{url}");
        }
    }

    #[test]
    fn a_relative_path_is_in_the_checkout() {
        assert_eq!(in_checkout("/src/w", None), "/src/w");
        assert_eq!(
            in_checkout("/src/w/", Some("a/b.rs".into())),
            "/src/w/a/b.rs"
        );
        assert_eq!(
            in_checkout("/src/w", Some("/etc/hosts".into())),
            "/etc/hosts"
        );
    }

    #[test]
    fn a_container_waits_for_its_ssh_alias() {
        let container = Workspace {
            id: WorkspaceId::derive("c"),
            project: None,
            device: DeviceId::derive("nl"),
            location: WorkspaceLocation::Container {
                instance: "mhq-c".into(),
                path: "/home/agent/c".into(),
            },
            managed: None,
            clone: None,
        };
        assert_eq!(
            reach(&container, Some(Route::Local)),
            Err(failed("Containers can't open in an editor yet"))
        );
        let folder = Workspace {
            location: WorkspaceLocation::Folder {
                path: "/src/w".into(),
            },
            ..container
        };
        assert_eq!(reach(&folder, Some(Route::Local)), Ok(Reach::Here));
        assert_eq!(
            reach(
                &folder,
                Some(Route::Ssh {
                    host: "dev@devbox".into()
                })
            ),
            Ok(Reach::Ssh {
                host: "dev@devbox".into()
            })
        );
        assert_eq!(
            reach(
                &folder,
                Some(Route::Wsl {
                    distro: "Ubuntu".into(),
                    via: None
                })
            ),
            Ok(Reach::Wsl {
                distro: "Ubuntu".into()
            })
        );
        assert!(reach(
            &folder,
            Some(Route::Wsl {
                distro: "Ubuntu".into(),
                via: Some("dev@pc".into())
            })
        )
        .is_err());
        assert_eq!(reach(&folder, None), Err(failed("No route to this device")));
    }

    /// An app on the two-device harness whose editors are in `places`.
    pub(crate) fn app_with(places: Places) -> (App<crate::devices::tests::Fake>, FakeShell) {
        app_on(two_devices().0, places)
    }

    fn app_on(harness: Harness, places: Places) -> (App<crate::devices::tests::Fake>, FakeShell) {
        let Harness { devices, view, .. } = harness;
        let shell = FakeShell::default();
        let mut app = App::new(view, devices, shell.clone());
        app.editors = Installed::new(places);
        (app, shell)
    }

    #[test]
    fn a_workspace_here_opens_through_the_command_line_or_the_link() {
        let scratch = Scratch::new();
        fs::create_dir_all(scratch.0.join("Applications/Visual Studio Code.app")).unwrap();
        let cursor = scratch.program("bin/cursor", true);
        let (app, shell) = app_with(scratch.places());
        assert_eq!(
            block_on(app.list_editors()),
            Editors {
                available: vec![
                    EditorChoice {
                        id: "cursor".into(),
                        label: "Cursor".into()
                    },
                    EditorChoice {
                        id: "vscode".into(),
                        label: "VS Code".into()
                    },
                ],
                known: EDITORS.iter().map(EditorChoice::from).collect(),
            }
        );

        let w = WorkspaceId::derive("w");
        let open = |editor, path: Option<&str>, line, column| {
            block_on(app.open_in_editor(editor, w, path.map(str::to_owned), line, column))
        };
        open("cursor", None, None, None).unwrap();
        open("cursor", Some("src/main.rs"), Some(4), None).unwrap();
        open("vscode", Some("src/main.rs"), Some(4), Some(2)).unwrap();
        assert_eq!(
            *shell.1.lock().unwrap(),
            [
                Opened::Editor(cursor.clone(), vec!["--classic".into(), "/src/w".into()]),
                Opened::Editor(
                    cursor,
                    vec![
                        "--classic".into(),
                        "--goto".into(),
                        "/src/w/src/main.rs:4:1".into()
                    ]
                ),
                Opened::Url("vscode://file/src/w/src/main.rs:4:2".into()),
            ]
        );
        assert!(shell.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_folder_here_has_to_be_a_full_path() {
        let scratch = Scratch::new();
        fs::create_dir_all(scratch.0.join("Applications/Visual Studio Code.app")).unwrap();
        scratch.program("bin/cursor", true);
        let (h, _, _) = two_devices();
        let w = WorkspaceId::derive("w");
        let Some(Entity::Workspace(mut found)) = h
            .view
            .read(|merge| merge.store().get(&EntityKey::Workspace(w)).cloned())
        else {
            panic!("no workspace");
        };
        found.location = WorkspaceLocation::Folder {
            path: "--install-extension=publisher.name".into(),
        };
        h.report(
            h.local().id,
            LinkEvent::Synced(vec![Entity::Workspace(found)]),
        );
        let (app, shell) = app_on(h, scratch.places());
        for editor in ["cursor", "vscode"] {
            for path in [None, Some("src/main.rs".to_owned())] {
                assert_eq!(
                    block_on(app.open_in_editor(editor, w, path, None, None)),
                    Err(failed("This workspace's folder isn't a full path"))
                );
            }
        }
        assert!(shell.0.lock().unwrap().is_empty());
        assert!(shell.1.lock().unwrap().is_empty());
    }

    #[test]
    fn a_workspace_on_another_device_opens_over_ssh() {
        let scratch = Scratch::new();
        scratch.program("bin/code", true);
        let (app, shell) = app_with(scratch.places());
        let nl = WorkspaceId::derive("w-nl");
        block_on(app.open_in_editor("vscode", nl, None, None, None)).unwrap();
        block_on(app.open_in_editor("vscode", nl, Some("a.rs".into()), None, None)).unwrap();
        assert_eq!(
            *shell.1.lock().unwrap(),
            [
                Opened::Url("vscode://vscode-remote/ssh-remote+devbox/src/w-nl".into()),
                Opened::Url("vscode://vscode-remote/ssh-remote+devbox/src/w-nl/a.rs:1:1".into()),
            ]
        );

        assert_eq!(
            block_on(app.open_in_editor("cursor", nl, None, None, None)),
            Err(failed("Cursor isn't installed on this computer"))
        );
        assert_eq!(
            block_on(app.open_in_editor("zed", nl, None, None, None)),
            Err(CommandError::NotFound {
                what: "This editor".into()
            })
        );
        assert_eq!(
            block_on(app.open_in_editor("vscode", WorkspaceId::derive("gone"), None, None, None)),
            Err(CommandError::NotFound {
                what: "This workspace".into()
            })
        );
        assert_eq!(shell.1.lock().unwrap().len(), 2);
    }
}
