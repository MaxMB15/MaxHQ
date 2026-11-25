// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's own SSH files, which let an editor's Remote-SSH reach a
//! container workspace with no sshd in the container. The folder holds the
//! app's key, a `config` with a `Host` entry for each running container,
//! and a `known_hosts` with each container's host key. Each entry's
//! `ProxyCommand` runs `maxd proxy --guest-ssh` on the container's device,
//! which carries the session to the guest.
//!
//! Your own `~/.ssh/config` reaches the entries through one `Include` line
//! at its top, which the app adds only once you say yes. It never changes
//! any other line there.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_core::shell::quoted;
use maxhq_os::fsmode;
use maxhq_protocol::ids::{DeviceId, WorkspaceId};
use maxhq_protocol::link::Route;
use maxhq_protocol::model::{ManagedWorkspace, Workspace, WorkspaceLocation, WorkspaceState};
use maxhq_protocol::state::{Entity, EntityKey};
use serde::Serialize;
use specta::Type;
use ssh_key::private::Ed25519Keypair;
use ssh_key::{Algorithm, LineEnding, PrivateKey, PublicKey};
use zeroize::Zeroizing;

use crate::devices::{capabilities, device_links};
use crate::merge::Merge;

/// The app's private key, which signs it in to every guest.
const KEY: &str = "id_ed25519";
/// The `Host` entries.
const CONFIG: &str = "config";
/// The guests' host keys.
const KNOWN_HOSTS: &str = "known_hosts";
/// Left in the folder once you say no to the `Include`, so the app doesn't
/// ask again.
const DECLINED: &str = "include-declined";
/// The comment the app's key carries, so you can tell it apart.
const KEY_COMMENT: &str = "maxhq";
/// The user every session signs in as.
const USER: &str = "agent";
/// The longest Incus instance name.
const MAX_INSTANCE: usize = 63;
/// The longest host key line read. An ed25519 line is about 80 bytes.
const MAX_HOST_KEY: usize = 1024;
/// The longest key file read. An ed25519 key file is about 400 bytes.
const MAX_KEY_FILE: u64 = 16 * 1024;
/// The longest `~/.ssh/config` the app reads or changes.
const MAX_USER_CONFIG: u64 = 1 << 20;
/// What the device runs, as `maxd`'s own SSH link runs it, so `maxd` is
/// found in `~/.local/bin` where the installer puts it. A forced command
/// accepts exactly this form.
const REMOTE_START: &str =
    r#"sh -c 'PATH="$PATH:$HOME/.local/bin"; export PATH; exec maxd proxy --guest-ssh "#;
/// What follows the workspace in [`REMOTE_START`]'s command.
const REMOTE_END: &str = "'";

/// The host name an editor reaches a running container workspace by, such
/// as `mhq-devbox.<device>.maxhq`: the container's instance name, then its
/// device's whole ID in hex. None for any other workspace, and for one whose
/// guest hasn't reported its host key yet.
pub fn alias(workspace: &Workspace) -> Option<String> {
    checked(workspace).ok().map(|(alias, _)| alias)
}

/// Where an editor stands with a container workspace over SSH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SshIncludeState {
    /// Your `~/.ssh/config` includes MaxHQ's, so an editor reaches the
    /// workspace as `alias`.
    Included { alias: String },
    /// The app hasn't asked yet. `line` is what it adds at the top of your
    /// `~/.ssh/config` once you say yes.
    Ask { alias: String, line: String },
    /// You said no. `block` is the entry to copy into your own config.
    Declined { alias: String, block: String },
    /// No editor reaches the workspace now, with why.
    Unavailable { reason: String },
}

/// One container's entry in the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    alias: String,
    workspace: WorkspaceId,
    /// The `ProxyCommand`, as the config line takes it.
    proxy: String,
    /// The guest's host key, as `ssh-ed25519 <base64>`.
    host_key: String,
}

/// MaxHQ's SSH folder, the app's key in it, and the home whose
/// `~/.ssh/config` can include it.
pub struct SshFiles {
    dir: PathBuf,
    /// `dir` as the config lines spell it.
    spelled: String,
    home: PathBuf,
    /// The `maxd` a device on this machine's own link runs the proxy with.
    maxd: Option<PathBuf>,
    /// The app's public key, as `ssh-ed25519 <base64>`.
    public_key: String,
    /// What `config` and `known_hosts` hold, as the last write left them.
    /// Held through each write, so the writes go one at a time.
    written: Mutex<Option<(String, String)>>,
    /// Whether the files are done with, as the app deletes its data, so
    /// no write puts a file back in the folder.
    stopped: AtomicBool,
}

impl SshFiles {
    /// Opens the folder at `dir`, made private, with the app's key in it,
    /// made the first time. The folder's path goes into config lines, so
    /// one with a character ssh would read differently is refused.
    pub fn open(dir: PathBuf, home: PathBuf, maxd: Option<PathBuf>) -> io::Result<Self> {
        let spelled = plain_path(&dir).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the SSH folder's path has characters ssh would read differently",
            )
        })?;
        fsmode::private_dir_all(&dir)?;
        fsmode::set_mode(&dir, 0o700)?;
        let public_key = client_key(&dir.join(KEY))?;
        Ok(Self {
            dir,
            spelled,
            home,
            maxd,
            public_key,
            written: Mutex::new(None),
            stopped: AtomicBool::new(false),
        })
    }

    /// Ends the writes, once any write under way is done, so the folder
    /// can go.
    pub(crate) fn stop(&self) {
        let _written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        self.stopped.store(true, Ordering::Release);
    }

    /// The app's public key, as `ssh-ed25519 <base64>`.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// An entry for every running container workspace the view shows that
    /// an editor can reach, in alias order.
    pub(crate) fn entries(&self, merge: &Merge) -> Vec<Entry> {
        let mut entries: Vec<_> = merge
            .store()
            .entities()
            .filter_map(|entity| match entity {
                Entity::Workspace(workspace) => self.entry_of(merge, workspace).ok(),
                _ => None,
            })
            .collect();
        entries.sort_by(|a, b| a.alias.cmp(&b.alias));
        entries
    }

    /// The workspace's entry, or why an editor can't reach it. None when
    /// the view has no such workspace.
    pub(crate) fn entry(
        &self,
        merge: &Merge,
        workspace: WorkspaceId,
    ) -> Option<Result<Entry, &'static str>> {
        match merge.store().get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(found)) => Some(self.entry_of(merge, found)),
            _ => None,
        }
    }

    fn entry_of(&self, merge: &Merge, workspace: &Workspace) -> Result<Entry, &'static str> {
        let (alias, host_key) = checked(workspace)?;
        let takes_ssh = capabilities(merge, workspace.device).is_some_and(|does| does.guest_ssh);
        if !takes_ssh {
            return Err("Update maxd on this device to open its containers in an editor.");
        }
        let proxy = self
            .proxy(merge, workspace.device, workspace.id)
            .ok_or("No link reaches this device in a way an editor can use.")?;
        Ok(Entry {
            alias,
            workspace: workspace.id,
            proxy,
            host_key,
        })
    }

    /// The `ProxyCommand` that reaches `workspace` on `device`. A link on
    /// this machine runs the proxy itself, and any other runs it over the
    /// link's SSH host, the lowest link ID first so the files stay put.
    fn proxy(&self, merge: &Merge, device: DeviceId, workspace: WorkspaceId) -> Option<String> {
        let mut links: Vec<_> = device_links(merge, device).collect();
        links.sort_by_key(|link| (link.route != Route::Local, link.id));
        links.iter().find_map(|link| match &link.route {
            Route::Local => local_proxy(self.maxd.as_deref()?, workspace),
            Route::Ssh { host } => ssh_proxy(host, workspace),
            // Containers run under Incus, which a distro set up as a
            // device doesn't host.
            Route::Wsl { .. } => None,
        })
    }

    /// Writes `config` and `known_hosts` with what `entries` gives, unless
    /// they hold it already or the files are stopped. `entries` runs once
    /// the write's turn comes, so the last write has the latest entries.
    pub(crate) fn write_with(&self, entries: impl FnOnce() -> Vec<Entry>) -> io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let entries = entries();
        let now = (self.config(&entries), known_hosts(&entries));
        if written.as_ref() == Some(&now) {
            return Ok(());
        }
        // The keys go first, so an entry never comes before its key.
        write_atomic(
            &self.dir.join(KNOWN_HOSTS),
            now.1.as_bytes(),
            Atomic::PRIVATE,
        )?;
        write_atomic(&self.dir.join(CONFIG), now.0.as_bytes(), Atomic::PRIVATE)?;
        *written = Some(now);
        Ok(())
    }

    /// Where `entry` stands with your `~/.ssh/config`.
    pub(crate) fn include_state(&self, entry: &Entry) -> io::Result<SshIncludeState> {
        let alias = entry.alias.clone();
        Ok(if self.included()? {
            SshIncludeState::Included { alias }
        } else if self.dir.join(DECLINED).exists() {
            SshIncludeState::Declined {
                alias,
                block: self.host_block(entry),
            }
        } else {
            SshIncludeState::Ask {
                alias,
                line: self.include_line(),
            }
        })
    }

    /// Adds the `Include` line to your `~/.ssh/config`, or with `add`
    /// false remembers that you said no, then says where `entry` stands.
    pub(crate) fn set_include(&self, entry: &Entry, add: bool) -> io::Result<SshIncludeState> {
        let declined = self.dir.join(DECLINED);
        if add {
            self.add_include()?;
            match fs::remove_file(&declined) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        } else {
            write_atomic(&declined, b"", Atomic::PRIVATE)?;
        }
        self.include_state(entry)
    }

    /// The line that brings MaxHQ's config into yours.
    fn include_line(&self) -> String {
        format!("Include \"{}/{CONFIG}\"", self.spelled)
    }

    /// Your `~/.ssh/config`.
    fn user_config(&self) -> PathBuf {
        self.home.join(".ssh").join("config")
    }

    /// Whether your `~/.ssh/config` has the `Include` line.
    fn included(&self) -> io::Result<bool> {
        match read_limited(&self.user_config(), MAX_USER_CONFIG) {
            Ok(text) => Ok(has_line(&text, &self.include_line())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Puts the `Include` line at the top of your `~/.ssh/config`, ahead
    /// of every `Host`, unless it's there already. The rest of the file
    /// stays as it was, byte for byte, and keeps its mode. A config that
    /// links to another file, as a dotfile manager makes, changes where it
    /// points.
    fn add_include(&self) -> io::Result<()> {
        let ssh = self.home.join(".ssh");
        fsmode::private_dir(&ssh).or_else(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
        let target = self.config_target()?;
        let (text, mode) = match read_limited(&target, MAX_USER_CONFIG) {
            Ok(text) => (text, fsmode::mode_of(&fs::metadata(&target)?) & 0o7777),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (String::new(), 0o600),
            Err(error) => return Err(error),
        };
        let line = self.include_line();
        if has_line(&text, &line) {
            return Ok(());
        }
        let changed = format!("{line}\n{text}");
        write_atomic(&target, changed.as_bytes(), Atomic::exact(mode))
    }

    /// Takes the `Include` line back out of your `~/.ssh/config`, as the
    /// app deletes its data. Every other line stays as it was, byte for
    /// byte, and the file keeps its mode. With no config, or no line in
    /// it, nothing changes.
    pub(crate) fn remove_include(&self) -> io::Result<()> {
        let target = match self.config_target() {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let text = match read_limited(&target, MAX_USER_CONFIG) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let line = self.include_line();
        if !has_line(&text, &line) {
            return Ok(());
        }
        let kept: String = text
            .split_inclusive('\n')
            .filter(|each| each.trim() != line)
            .collect();
        let mode = fsmode::mode_of(&fs::metadata(&target)?) & 0o7777;
        write_atomic(&target, kept.as_bytes(), Atomic::exact(mode))
    }

    /// The file your `~/.ssh/config` is: the one it links to, when a
    /// dotfile manager made it a link, or else itself.
    fn config_target(&self) -> io::Result<PathBuf> {
        let path = self.user_config();
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(&path),
            _ => Ok(path),
        }
    }

    /// MaxHQ's `config`, with an entry for each of `entries`.
    fn config(&self, entries: &[Entry]) -> String {
        let mut text = String::from(
            "# MaxHQ writes this file, and rewrites it as its containers change.\n\
             # Changes made here don't last.\n",
        );
        for entry in entries {
            text.push('\n');
            text.push_str(&self.host_block(entry));
        }
        text
    }

    /// One container's `Host` entry.
    fn host_block(&self, entry: &Entry) -> String {
        let Entry {
            alias,
            workspace,
            proxy,
            ..
        } = entry;
        let dir = &self.spelled;
        format!(
            "Host {alias}\n\
             \x20 User {USER}\n\
             \x20 ProxyCommand {proxy}\n\
             \x20 IdentityFile \"{dir}/{KEY}\"\n\
             \x20 IdentitiesOnly yes\n\
             \x20 UserKnownHostsFile \"{dir}/{KNOWN_HOSTS}\"\n\
             \x20 HostKeyAlias {}\n\
             \x20 StrictHostKeyChecking yes\n\
             \x20 ForwardAgent no\n",
            key_alias(*workspace),
        )
    }
}

/// The workspace's alias and its guest's host key, or why an editor can't
/// reach it.
fn checked(workspace: &Workspace) -> Result<(String, String), &'static str> {
    let WorkspaceLocation::Container { instance, .. } = &workspace.location else {
        return Err("Only a container workspace opens in an editor over SSH.");
    };
    let Some(managed) = &workspace.managed else {
        return Err("MaxHQ didn't make this container, so it can't reach it over SSH.");
    };
    if managed.state != WorkspaceState::Running {
        return Err("Start the container to open it in an editor.");
    }
    let host_key = host_key(managed).ok_or("The container hasn't reported its SSH key yet.")?;
    let instance = instance_name(instance).ok_or("The container's name can't be an SSH host.")?;
    let alias = format!("mhq-{instance}.{}.maxhq", device_slug(workspace.device));
    Ok((alias, host_key))
}

/// The guest's host key as one `ssh-ed25519 <base64>` line, or none when
/// it hasn't reported one that reads as an ed25519 key.
fn host_key(managed: &ManagedWorkspace) -> Option<String> {
    let line = managed.ssh_host_key.as_deref()?;
    if line.len() > MAX_HOST_KEY {
        return None;
    }
    let mut key = PublicKey::from_openssh(line.trim()).ok()?;
    if key.algorithm() != Algorithm::Ed25519 {
        return None;
    }
    key.set_comment("");
    key.to_openssh().ok()
}

/// The Incus instance name in lower case, if it's one a host name can hold.
fn instance_name(instance: &str) -> Option<String> {
    let fits = !instance.is_empty()
        && instance.len() <= MAX_INSTANCE
        && !instance.starts_with('-')
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    fits.then(|| instance.to_ascii_lowercase())
}

/// The device's whole ID in hex, which tells apart containers of the same
/// name on different devices. A device can't pick an ID that ends like
/// another's and so take its alias.
fn device_slug(device: DeviceId) -> String {
    device.0.simple().to_string()
}

/// The name `known_hosts` keeps the workspace's host key under.
fn key_alias(workspace: WorkspaceId) -> String {
    format!("maxhq-{workspace}")
}

/// MaxHQ's `known_hosts`, with each entry's host key.
fn known_hosts(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{} {}\n", key_alias(entry.workspace), entry.host_key))
        .collect()
}

/// The proxy over an SSH link: ssh to the link's host, which runs
/// `maxd proxy --guest-ssh` there. None for a host that isn't plainly a
/// host name, which the shell or ssh could read as something else.
fn ssh_proxy(host: &str, workspace: WorkspaceId) -> Option<String> {
    let plain = !host.is_empty()
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@._:-[]".contains(&byte));
    if !plain {
        return None;
    }
    let remote = format!("{REMOTE_START}{workspace}{REMOTE_END}");
    Some(format!(
        "ssh -o BatchMode=yes -o ControlPath=none {host} {}",
        proxy_word(&remote)
    ))
}

/// The proxy for this machine's own `maxd`, which runs `maxd` itself.
fn local_proxy(maxd: &Path, workspace: WorkspaceId) -> Option<String> {
    let maxd = plain_path(maxd)?;
    Some(format!(
        "{} proxy --guest-ssh {workspace}",
        proxy_word(&maxd)
    ))
}

/// `text` as one word of a `ProxyCommand`. On Unix ssh runs the command
/// through `sh`, so it goes in single quotes. Windows' ssh starts it by
/// Windows' own rules, which group words in double quotes and read `\"`
/// as a quote inside them. Only for this module's own words, which hold no
/// backslash and none of cmd's `&|<>^%`.
fn proxy_word(text: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        quoted(text)
    }
}

/// `path` as text that reads the same inside double quotes in an ssh
/// config and inside single quotes in a shell, with nothing ssh expands.
/// None for a path that doesn't.
fn plain_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    // ssh on Windows takes `/` between a path's folders too, so its `\`
    // is spelled that way rather than read as an escape. A `\\?\` or
    // network path isn't one it reads plainly.
    let text = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    };
    let plain = path.is_absolute()
        && !(cfg!(windows) && text.starts_with("//"))
        && !text
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\'' | '\\' | '$' | '%' | '`'));
    plain.then_some(text)
}

/// Whether one of `text`'s lines is `line`, give or take the space around
/// it.
fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|each| each.trim() == line)
}

/// The app's key at `path`, made the first time, and its public line. The
/// key is kept private to you, as ssh wants it.
fn client_key(path: &Path) -> io::Result<String> {
    let key = match read_limited(path, MAX_KEY_FILE) {
        Ok(text) => {
            let text = Zeroizing::new(text);
            let key = PrivateKey::from_openssh(text.as_bytes()).map_err(|_| unreadable_key())?;
            if key.algorithm() != Algorithm::Ed25519 || key.is_encrypted() {
                return Err(unreadable_key());
            }
            fsmode::private_file(path)?;
            key
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => make_key(path)?,
        Err(error) => return Err(error),
    };
    let mut public = key.public_key().clone();
    public.set_comment("");
    public.to_openssh().map_err(io::Error::other)
}

/// Makes a new ed25519 key from the system's random source and writes it
/// to `path` as OpenSSH does, readable only by you.
fn make_key(path: &Path) -> io::Result<PrivateKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut()).map_err(io::Error::other)?;
    let mut key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
    key.set_comment(KEY_COMMENT);
    let text = key.to_openssh(LineEnding::LF).map_err(io::Error::other)?;
    write_atomic(path, text.as_bytes(), Atomic::PRIVATE)?;
    Ok(key)
}

/// The error for a key file that isn't one unencrypted ed25519 key.
fn unreadable_key() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "the app's SSH key isn't one it can read",
    )
}

/// The text at `path`, refused past `limit` bytes.
fn read_limited(path: &Path, limit: u64) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is too large", path.display()),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    use maxhq_protocol::ids::LinkId;
    use maxhq_protocol::link::{DeviceLink, LinkState};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::{EgressCheck, GuestState};
    use maxhq_protocol::time::Timestamp;

    /// A folder under the system's temp folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-ssh-{}", LinkId::generate()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A made-up host key line, with every key byte `byte`.
    fn host_key_line(byte: u8) -> String {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let mut blob = Vec::new();
        for part in [b"ssh-ed25519".as_slice(), &[byte; 32]] {
            blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
            blob.extend_from_slice(part);
        }
        format!("ssh-ed25519 {}", STANDARD.encode(blob))
    }

    fn device() -> DeviceId {
        DeviceId::derive("devbox")
    }

    /// A running container workspace on [`device`], with a host key.
    fn container(name: &str) -> Workspace {
        Workspace {
            id: WorkspaceId::derive(name),
            project: None,
            device: device(),
            location: WorkspaceLocation::Container {
                instance: name.into(),
                path: "/work".into(),
            },
            managed: Some(ManagedWorkspace {
                state: WorkspaceState::Running,
                start_at_boot: false,
                guest: GuestState::Installing,
                egress: EgressCheck::NotRun,
                recipe: 1,
                created_at: Timestamp(1),
                ssh_host_key: Some(format!("{} root@{name}", host_key_line(7))),
                unsaved: None,
                memory_soft: None,
            }),
            clone: None,
        }
    }

    /// A view with `workspaces` and a link to [`device`] over `route`.
    fn merge(route: Route, guest_ssh: bool, workspaces: Vec<Workspace>) -> Merge {
        let mut merge = Merge::new();
        let spec = maxhq_client::LinkSpec {
            id: LinkId::derive("devbox"),
            name: "devbox".into(),
            route: route.clone(),
            fallbacks: Vec::new(),
            last_good: Default::default(),
        };
        merge.add_link(&spec);
        let link = DeviceLink {
            id: spec.id,
            name: spec.name.clone(),
            device: Some(device()),
            route,
            state: LinkState::Connected {
                since: Timestamp(1),
            },
            network: None,
            peer: None,
            via: Vec::new(),
            capabilities: Some(Capabilities {
                guest_ssh,
                ..Capabilities::default()
            }),
            protocol: None,
            protocol_mismatch: None,
            waiting: Vec::new(),
            routes: Vec::new(),
            claim: None,
        };
        merge.link_event(spec.id, maxhq_client::LinkEvent::Link(Box::new(link)));
        let entities = workspaces.into_iter().map(Entity::Workspace).collect();
        merge.link_event(spec.id, maxhq_client::LinkEvent::Synced(entities));
        merge
    }

    /// The folder and home the files go in, both under `temp`.
    fn files(temp: &TempDir) -> SshFiles {
        let dir = temp.0.join("Application Support/MaxHQ/ssh");
        let home = temp.0.join("home");
        fs::create_dir_all(&home).unwrap();
        SshFiles::open(
            dir,
            home,
            Some(PathBuf::from("/Applications/MaxHQ.app/maxd")),
        )
        .unwrap()
    }

    fn ssh_route() -> Route {
        Route::Ssh {
            host: "dev@devbox".into(),
        }
    }

    fn mode(path: &Path) -> u32 {
        fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777
    }

    #[test]
    fn devices_whose_ids_end_alike_get_their_own_aliases() {
        let mut one = container("web");
        let mut two = container("web");
        one.device = DeviceId(uuid::Uuid::from_u128(
            0x1111_0000_0000_0000_0000_0000_1a2b_3c4d,
        ));
        two.device = DeviceId(uuid::Uuid::from_u128(
            0x2222_0000_0000_0000_0000_0000_1a2b_3c4d,
        ));
        let (one, two) = (alias(&one).unwrap(), alias(&two).unwrap());
        assert_ne!(one, two);
        assert!(one.ends_with("00000000000000001a2b3c4d.maxhq"), "{one}");
    }

    #[test]
    fn a_running_container_with_a_host_key_has_an_alias() {
        let workspace = container("Web-1");
        let slug = device_slug(device());
        assert_eq!(slug.len(), 32);
        assert_eq!(alias(&workspace), Some(format!("mhq-web-1.{slug}.maxhq")));

        let mut stopped = workspace.clone();
        stopped.managed.as_mut().unwrap().state = WorkspaceState::Stopped;
        let mut keyless = workspace.clone();
        keyless.managed.as_mut().unwrap().ssh_host_key = None;
        let mut rsa = workspace.clone();
        rsa.managed.as_mut().unwrap().ssh_host_key = Some("ssh-rsa AAAAB3NzaC1yc2E=".into());
        let mut folder = workspace.clone();
        folder.location = WorkspaceLocation::Folder {
            path: "/src/web".into(),
        };
        let mut unmanaged = workspace.clone();
        unmanaged.managed = None;
        let mut odd = workspace.clone();
        odd.location = WorkspaceLocation::Container {
            instance: "web;rm -rf".into(),
            path: "/work".into(),
        };
        for other in [stopped, keyless, rsa, folder, unmanaged, odd] {
            assert_eq!(alias(&other), None, "{:?}", other.location);
        }
    }

    #[test]
    fn the_key_is_made_once_private_and_kept() {
        let temp = TempDir::new();
        let files = files(&temp);
        let key = files.dir.join(KEY);
        assert_eq!(mode(&files.dir), 0o700);
        assert_eq!(mode(&key), 0o600);
        assert!(files.public_key().starts_with("ssh-ed25519 "));
        assert_eq!(files.public_key().split(' ').count(), 2, "no comment");
        let text = fs::read_to_string(&key).unwrap();
        assert!(text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));

        fsmode::set_mode(&key, 0o644).unwrap();
        let again = SshFiles::open(files.dir.clone(), files.home.clone(), None).unwrap();
        assert_eq!(again.public_key(), files.public_key());
        assert_eq!(fs::read_to_string(&key).unwrap(), text);
        assert_eq!(mode(&key), 0o600, "a key others can read is made private");

        fs::write(&key, "not a key").unwrap();
        let broken = SshFiles::open(files.dir.clone(), files.home.clone(), None);
        assert_eq!(
            broken.err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        assert_eq!(
            fs::read_to_string(&key).unwrap(),
            "not a key",
            "never replaced"
        );
    }

    #[test]
    fn a_folder_ssh_would_misread_is_refused() {
        let temp = TempDir::new();
        for name in ["has\"quote", "has$HOME", "has%h", "has\nline"] {
            let opened = SshFiles::open(temp.0.join(name), temp.0.clone(), None);
            assert_eq!(
                opened.err().map(|e| e.kind()),
                Some(io::ErrorKind::InvalidInput),
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_files_hold_an_entry_for_each_reachable_container() {
        let temp = TempDir::new();
        let files = files(&temp);
        let mut stopped = container("old");
        stopped.managed.as_mut().unwrap().state = WorkspaceState::Stopped;
        let view = merge(
            ssh_route(),
            true,
            vec![container("web"), stopped, container("api")],
        );
        files.write_with(|| files.entries(&view)).unwrap();

        let dir = &files.spelled;
        let slug = device_slug(device());
        let api = WorkspaceId::derive("api");
        let web = WorkspaceId::derive("web");
        let block = |name: &str, id: WorkspaceId| {
            format!(
                "Host mhq-{name}.{slug}.maxhq\n\
                 \x20 User agent\n\
                 \x20 ProxyCommand ssh -o BatchMode=yes -o ControlPath=none dev@devbox \
                 'sh -c '\\''PATH=\"$PATH:$HOME/.local/bin\"; export PATH; \
                 exec maxd proxy --guest-ssh {id}'\\'''\n\
                 \x20 IdentityFile \"{dir}/id_ed25519\"\n\
                 \x20 IdentitiesOnly yes\n\
                 \x20 UserKnownHostsFile \"{dir}/known_hosts\"\n\
                 \x20 HostKeyAlias maxhq-{id}\n\
                 \x20 StrictHostKeyChecking yes\n\
                 \x20 ForwardAgent no\n"
            )
        };
        let config = fs::read_to_string(files.dir.join(CONFIG)).unwrap();
        assert_eq!(
            config,
            format!(
                "# MaxHQ writes this file, and rewrites it as its containers change.\n\
                 # Changes made here don't last.\n\n{}\n{}",
                block("api", api),
                block("web", web)
            )
        );
        assert!(dir.contains("Application Support"), "the space is quoted");
        let key = host_key_line(7);
        assert_eq!(
            fs::read_to_string(files.dir.join(KNOWN_HOSTS)).unwrap(),
            format!("maxhq-{api} {key}\nmaxhq-{web} {key}\n"),
        );
        assert_eq!(mode(&files.dir.join(CONFIG)), 0o600);
        assert_eq!(mode(&files.dir.join(KNOWN_HOSTS)), 0o600);
    }

    #[test]
    fn the_files_change_when_a_host_key_or_route_does() {
        let temp = TempDir::new();
        let files = files(&temp);
        let config = files.dir.join(CONFIG);
        let known = files.dir.join(KNOWN_HOSTS);
        let view = merge(ssh_route(), true, vec![container("web")]);
        files.write_with(|| files.entries(&view)).unwrap();

        // An unchanged view leaves the files alone.
        fs::write(&config, "edited").unwrap();
        files.write_with(|| files.entries(&view)).unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), "edited");

        let mut moved = container("web");
        moved.managed.as_mut().unwrap().ssh_host_key = Some(host_key_line(9));
        let view = merge(ssh_route(), true, vec![moved]);
        files.write_with(|| files.entries(&view)).unwrap();
        assert!(fs::read_to_string(&known)
            .unwrap()
            .ends_with(&format!("{}\n", host_key_line(9))));

        let local = merge(Route::Local, true, vec![container("web")]);
        files.write_with(|| files.entries(&local)).unwrap();
        let id = WorkspaceId::derive("web");
        let line =
            format!("  ProxyCommand '/Applications/MaxHQ.app/maxd' proxy --guest-ssh {id}\n");
        assert!(fs::read_to_string(&config).unwrap().contains(&line));

        let old = merge(ssh_route(), false, vec![container("web")]);
        files.write_with(|| files.entries(&old)).unwrap();
        assert!(!fs::read_to_string(&config).unwrap().contains("Host "));
        assert_eq!(fs::read_to_string(&known).unwrap(), "");
    }

    #[test]
    fn a_stopped_folder_takes_no_more_writes() {
        let temp = TempDir::new();
        let files = files(&temp);
        files.stop();
        fs::remove_dir_all(&files.dir).unwrap();
        let view = merge(ssh_route(), true, vec![container("web")]);
        files.write_with(|| files.entries(&view)).unwrap();
        assert!(!files.dir.exists());
    }

    #[test]
    fn a_host_that_could_be_misread_gets_no_entry() {
        let id = WorkspaceId::derive("web");
        for host in [
            "-oProxyCommand=evil",
            "dev@devbox;rm",
            "dev@devbox evil",
            "",
            "a'b",
            "%h",
        ] {
            assert_eq!(ssh_proxy(host, id), None, "{host:?}");
        }
        assert!(ssh_proxy("dev@devbox.example.com", id).is_some());
        assert_eq!(local_proxy(Path::new("/odd'path/maxd"), id), None);
        assert_eq!(local_proxy(Path::new("relative/maxd"), id), None);
    }

    #[test]
    fn a_workspace_says_why_an_editor_cant_reach_it() {
        let temp = TempDir::new();
        let files = files(&temp);
        let mut stopped = container("web");
        stopped.managed.as_mut().unwrap().state = WorkspaceState::Stopped;
        let view = merge(ssh_route(), true, vec![stopped]);
        let id = WorkspaceId::derive("web");
        assert_eq!(
            files.entry(&view, id),
            Some(Err("Start the container to open it in an editor."))
        );
        assert_eq!(files.entry(&view, WorkspaceId::derive("gone")), None);
        let old = merge(ssh_route(), false, vec![container("web")]);
        assert!(matches!(files.entry(&old, id), Some(Err(why)) if why.contains("Update maxd")));
    }

    #[test]
    fn the_include_goes_at_the_top_once_and_nothing_else_changes() {
        let temp = TempDir::new();
        let files = files(&temp);
        let view = merge(ssh_route(), true, vec![container("web")]);
        let entry = files.entries(&view).remove(0);
        let alias = entry.alias.clone();
        let line = format!("Include \"{}/config\"", files.spelled);

        // No ~/.ssh yet: the app asks, and a yes makes it private.
        assert_eq!(
            files.include_state(&entry).unwrap(),
            SshIncludeState::Ask {
                alias: alias.clone(),
                line: line.clone()
            }
        );
        let ssh = files.home.join(".ssh");
        assert!(!ssh.exists(), "asking changes nothing");
        assert_eq!(
            files.set_include(&entry, true).unwrap(),
            SshIncludeState::Included {
                alias: alias.clone()
            }
        );
        assert_eq!(mode(&ssh), 0o700);
        let config = ssh.join("config");
        assert_eq!(fs::read_to_string(&config).unwrap(), format!("{line}\n"));
        assert_eq!(mode(&config), 0o600);

        // An existing config keeps every line and its mode.
        let mine = "Host devbox\n  HostName devbox.example.com\n  User dev\r\n\nHost *\n  ServerAliveInterval 30";
        fs::write(&config, mine).unwrap();
        fsmode::set_mode(&config, 0o644).unwrap();
        let had = mode(&config);
        files.set_include(&entry, true).unwrap();
        files.set_include(&entry, true).unwrap();
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            format!("{line}\n{mine}")
        );
        assert_eq!(mode(&config), had);
    }

    // Making a link on Windows needs Developer Mode.
    #[cfg(unix)]
    #[test]
    fn a_config_that_links_elsewhere_changes_where_it_points() {
        let temp = TempDir::new();
        let files = files(&temp);
        let view = merge(ssh_route(), true, vec![container("web")]);
        let entry = files.entries(&view).remove(0);
        let ssh = files.home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        let real = temp.0.join("dotfiles-ssh-config");
        fs::write(&real, "Host devbox\n").unwrap();
        maxhq_core::fs_ext::symlink(&real, ssh.join("config")).unwrap();
        files.set_include(&entry, true).unwrap();
        assert!(fs::symlink_metadata(ssh.join("config"))
            .unwrap()
            .file_type()
            .is_symlink());
        let text = fs::read_to_string(&real).unwrap();
        assert!(text.starts_with("Include \"") && text.ends_with("\nHost devbox\n"));
    }

    #[test]
    fn deleting_takes_the_include_out_and_nothing_else() {
        let temp = TempDir::new();
        let files = files(&temp);
        let view = merge(ssh_route(), true, vec![container("web")]);
        let entry = files.entries(&view).remove(0);
        let ssh = files.home.join(".ssh");

        // No config: nothing to take out, and nothing made.
        files.remove_include().unwrap();
        assert!(!ssh.exists());

        fs::create_dir_all(&ssh).unwrap();
        let config = ssh.join("config");
        let mine = "Host devbox\r\n  User dev\n\nHost *\n  ServerAliveInterval 30";
        fs::write(&config, mine).unwrap();
        fsmode::set_mode(&config, 0o644).unwrap();
        let had = mode(&config);
        files.set_include(&entry, true).unwrap();
        files.remove_include().unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), mine);
        assert_eq!(mode(&config), had);
        files.remove_include().unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), mine);

        // A config that links elsewhere changes where it points, and one
        // that links to nothing is left alone. Making a link on Windows
        // needs Developer Mode.
        if cfg!(windows) {
            return;
        }
        fs::remove_file(&config).unwrap();
        let real = temp.0.join("dotfiles-ssh-config");
        let line = files.include_line();
        fs::write(&real, format!("Host devbox\n  {line}  \nHost *\n")).unwrap();
        maxhq_core::fs_ext::symlink(&real, &config).unwrap();
        files.remove_include().unwrap();
        assert!(fs::symlink_metadata(&config)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "Host devbox\nHost *\n");
        fs::remove_file(&real).unwrap();
        files.remove_include().unwrap();
    }

    #[test]
    fn a_no_is_remembered_and_shows_the_entry_to_copy() {
        let temp = TempDir::new();
        let files = files(&temp);
        let view = merge(ssh_route(), true, vec![container("web")]);
        let entry = files.entries(&view).remove(0);
        let declined = files.set_include(&entry, false).unwrap();
        let SshIncludeState::Declined { alias, block } = &declined else {
            panic!("not declined: {declined:?}");
        };
        assert_eq!(alias, &entry.alias);
        assert!(block.starts_with(&format!("Host {alias}\n")));
        assert!(block.contains("  HostKeyAlias maxhq-"));
        assert!(
            !files.home.join(".ssh").exists(),
            "a no changes nothing of yours"
        );

        let again = SshFiles::open(files.dir.clone(), files.home.clone(), None).unwrap();
        assert_eq!(again.include_state(&entry).unwrap(), declined);
        assert!(matches!(
            again.set_include(&entry, true).unwrap(),
            SshIncludeState::Included { .. }
        ));
    }
}
