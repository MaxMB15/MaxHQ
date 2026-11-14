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
//! can install a command of the same name, as Cursor does with `code`. Then
//! it's looked for on `PATH`, leaving out a command that leads into another
//! editor's app. A workspace on this machine opens
//! through the editor's command line, or its link when only the app is
//! there. One on another device opens as a Remote-SSH link to it, and a
//! container's goes through MaxHQ's ssh alias for it.

use std::path::{Path, PathBuf};

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
        remote_scheme: Some("cursor"),
    },
    EditorDef {
        id: "vscode",
        label: "VS Code",
        commands: &["code"],
        base_args: &[],
        launch: LaunchStyle::Goto,
        mac_app: "Visual Studio Code",
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
}

impl Places {
    /// This machine's: the absolute folders on its `PATH`, then
    /// `/Applications` and `~/Applications`.
    pub fn of_this_machine() -> Self {
        let path = std::env::var_os("PATH")
            .map(|path| absolute_folders(&path))
            .unwrap_or_default();
        let mut apps = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            apps.push(Path::new(&home).join("Applications"));
        }
        Self { path, apps }
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
    EDITORS
        .iter()
        .filter_map(|editor| {
            let bundles: Vec<PathBuf> = places
                .apps
                .iter()
                .map(|root| root.join(app_name(editor)))
                .collect();
            let in_app = bundles.iter().find_map(|bundle| {
                let bin = bundle.join("Contents/Resources/app/bin");
                editor
                    .commands
                    .iter()
                    .map(|command| bin.join(command))
                    .find(|cli| runs(cli))
            });
            let cli = in_app.or_else(|| {
                editor.commands.iter().find_map(|command| {
                    places
                        .path
                        .iter()
                        .map(|dir| dir.join(command))
                        .find(|cli| runs(cli) && !in_other_app(editor, cli))
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

/// Whether `cli`, once its links are followed, is inside an editor's app
/// other than `editor`'s, wherever that app is.
fn in_other_app(editor: &EditorDef, cli: &Path) -> bool {
    let target = cli.canonicalize().unwrap_or_else(|_| cli.to_owned());
    EDITORS
        .iter()
        .filter(|other| other.id != editor.id)
        .any(|other| {
            let app = app_name(other);
            target
                .ancestors()
                .any(|dir| dir.file_name() == Some(app.as_ref()))
        })
}

/// Whether `path` is a program this machine can run.
fn runs(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
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
        None => Err(failed("No route to this device")),
    }
}

/// `path` in the checkout at `root`. One that starts with `/` is taken as
/// it is.
fn in_checkout(root: &str, path: Option<String>) -> String {
    match path {
        None => root.to_owned(),
        Some(path) if path.starts_with('/') => path,
        Some(path) => format!("{}/{path}", root.trim_end_matches('/')),
    }
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
    let at = at.or(file.then_some((1, 1)));
    format!(
        "{scheme}://vscode-remote/ssh-remote+{}{}{}",
        encode_component(host),
        encode_path(path),
        position(at)
    )
}

/// `:line:column`, or nothing.
fn position(at: Option<(u32, u32)>) -> String {
    at.map(|(line, column)| format!(":{line}:{column}"))
        .unwrap_or_default()
}

/// An absolute path for a link, each part encoded.
fn encode_path(path: &str) -> String {
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
        if matches!(reach, Reach::Here) && !path.starts_with('/') {
            return Err(failed("This workspace's folder isn't a full path"));
        }
        let at = line.map(|line| (line, column.unwrap_or(1)));
        let scheme = found.editor.remote_scheme;
        let opening = match (reach, &found.cli, scheme) {
            (Reach::Here, Some(cli), _) => self
                .shell
                .spawn_editor(cli.clone(), launch_args(found.editor, path, at)),
            (Reach::Here, None, Some(scheme)) => self.shell.open_url(file_url(scheme, &path, at)),
            (Reach::Ssh { host }, _, Some(scheme)) => self
                .shell
                .open_url(remote_url(scheme, &host, &path, file, at)),
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

    #[test]
    fn relative_folders_on_path_are_left_out() {
        assert_eq!(
            absolute_folders(std::ffi::OsStr::new("bin:/usr/bin:.::../up:/opt/bin")),
            [PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")]
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
        /// `runs` is false, and gives where it went.
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

        // A file that doesn't run isn't a command line.
        scratch.program("bin/code", false);
        assert_eq!(ids(&find(&scratch.places())), [("cursor", Some(&*in_apps))]);
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
        // As on Linux, where editors have no app folder.
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
