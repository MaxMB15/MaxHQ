// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The account folders a launch names, made ready before its holder
//! starts. Accounts come after the recipe, so the guest does this at each
//! spawn: a missing folder is made for the user the CLI runs as, and the
//! recipe's keys are merged into the config there (see
//! [`crate::recipe::accounts`]).
//!
//! The guest runs as root in a home that user can change, so nothing here
//! follows a link. The folder opens with [`Dir`], and a link, or anything
//! but a folder or a plain file, where the folder or its config should be
//! is refused.

use std::fs::Permissions;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use maxhq_transfer::beneath::{Dir, FileType};
use thiserror::Error;

use crate::recipe::accounts::{self, MergeError};
use crate::recipe::{AGENT_HOME, AGENT_UID};
use crate::session::holders::{Launch, SpawnError};

/// Gives `name` in a folder to a user and group, without following a
/// link.
pub(crate) type Chown = Arc<dyn Fn(&Dir, &str, u32, u32) -> io::Result<()> + Send + Sync>;

/// Merges the recipe's keys into a config's text.
type Merge = fn(&str, Option<&str>) -> Result<String, MergeError>;

/// Each variable that names an account folder, with the config in it and
/// that config's merge.
const FOLDERS: &[(&str, &str, Merge)] = &[
    (
        "CLAUDE_CONFIG_DIR",
        accounts::CLAUDE_CONFIG,
        accounts::claude_config,
    ),
    ("CODEX_HOME", accounts::CODEX_CONFIG, accounts::codex_config),
];

/// Why an account folder isn't ready.
#[derive(Debug, Error)]
enum Unready {
    #[error(transparent)]
    Merge(#[from] MergeError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Kind(String),
}

/// Where the account folders are, and how they're given to the CLI's
/// user.
#[derive(Clone)]
pub(crate) struct Folders {
    /// What [`AGENT_HOME`] is on this disk.
    pub(crate) home: PathBuf,
    /// Gives a new folder or config to the CLI's user. Only root may.
    pub(crate) chown: Chown,
}

impl Folders {
    /// The container's agent home.
    pub(crate) fn container() -> Self {
        Self {
            home: PathBuf::from(AGENT_HOME),
            chown: Arc::new(|dir, name, uid, gid| dir.chown(name, uid, gid)),
        }
    }

    /// Makes ready each account folder `launch` names.
    pub(crate) async fn ready(&self, launch: &Launch) -> Result<(), SpawnError> {
        let named: Vec<(String, &'static str, Merge)> = launch
            .env
            .iter()
            .filter_map(|(name, value)| {
                let (_, file, merge) = FOLDERS.iter().find(|(var, ..)| name == var)?;
                Some((value.to_string_lossy().into_owned(), *file, *merge))
            })
            .collect();
        if named.is_empty() {
            return Ok(());
        }
        // A guest that runs CLIs as its own user, as in tests, gives them
        // to the agent's IDs all the same.
        let owner = launch
            .run_as
            .as_ref()
            .map_or((AGENT_UID, AGENT_UID), |user| (user.uid, user.gid));
        let folders = self.clone();
        let done = tokio::task::spawn_blocking(move || {
            named.iter().try_for_each(|(folder, file, merge)| {
                folders
                    .prepare(folder, file, *merge, owner)
                    .map_err(|err| format!("couldn't get {folder} ready: {err}"))
            })
        })
        .await
        .unwrap_or_else(|_| Err("getting the account folder ready stopped".to_owned()));
        done.map_err(|message| SpawnError::Holder { message })
    }

    /// Makes `folder` if it's missing, then merges its config `file`.
    fn prepare(
        &self,
        folder: &str,
        file: &str,
        merge: Merge,
        owner: (u32, u32),
    ) -> Result<(), Unready> {
        let name = accounts::check_folder(folder)?;
        let home = Dir::open(&self.home)?;
        match home.kind(name)? {
            None => {
                home.make_dir(name, 0o700)?;
                (self.chown)(&home, name, owner.0, owner.1)?;
            }
            Some(FileType::Directory) => {}
            Some(_) => return Err(Unready::Kind("it isn't a folder".to_owned())),
        }
        let dir = home.dir(&[name.to_owned()])?;
        let existing = match dir.kind(file)? {
            None => None,
            Some(FileType::RegularFile) => {
                let mut text = String::new();
                dir.read_file(file)?.read_to_string(&mut text)?;
                Some(text)
            }
            Some(_) => return Err(Unready::Kind(format!("its {file} isn't a plain file"))),
        };
        let text = merge(folder, existing.as_deref())?;
        if existing.as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        let temp = format!("{file}.maxhq-new");
        let written = self.write(&dir, &temp, &text, owner);
        if written.is_err() {
            let _ = dir.remove_file(&temp);
        }
        written?;
        Ok(dir.rename(&temp, file)?)
    }

    /// Writes `text` to a new file `temp` in `dir`, owned by `owner` with
    /// mode 0600.
    fn write(&self, dir: &Dir, temp: &str, text: &str, owner: (u32, u32)) -> Result<(), Unready> {
        dir.remove_file(temp)?;
        let mut out = dir.open_file(temp, true)?;
        // The user can put a file of its own there between the two calls.
        // One that's empty and has no other name is as good as new.
        let meta = out.metadata()?;
        if meta.nlink() != 1 || meta.len() != 0 {
            return Err(Unready::Kind(format!("something else is at {temp}")));
        }
        out.set_permissions(Permissions::from_mode(0o600))?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        Ok((self.chown)(dir, temp, owner.0, owner.1)?)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::sync::Mutex;

    use maxhq_protocol::guest::Framing;
    use maxhq_protocol::ids::SessionId;

    use super::*;
    use crate::guestlink::wire;
    use crate::testing::TempDir;

    /// Each name given away, with its user and group.
    type Owned = Arc<Mutex<Vec<(String, u32, u32)>>>;

    /// Folders in a home under `root`, recording each chown in `owned`.
    fn folders(root: &Path) -> (Folders, Owned) {
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let owned = Arc::new(Mutex::new(Vec::new()));
        let chown: Chown = Arc::new({
            let owned = owned.clone();
            move |_, name, uid, gid| {
                owned.lock().unwrap().push((name.to_owned(), uid, gid));
                Ok(())
            }
        });
        (Folders { home, chown }, owned)
    }

    fn launch(env: &[(&str, &str)]) -> Launch {
        Launch {
            session: SessionId::generate(),
            program: PathBuf::from("/usr/local/bin/codex"),
            args: Vec::new(),
            env: env
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
                .collect(),
            env_remove: Vec::new(),
            secret_env: Vec::new(),
            run_as: None,
            cwd: PathBuf::from("/home/agent/work"),
            framing: wire::framing_in(Framing::JsonRpc),
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    #[tokio::test]
    async fn a_codex_spawn_makes_its_folder_and_trusts_the_work_folder() {
        let temp = TempDir::new();
        let (folders, owned) = folders(temp.path());
        let codex = launch(&[("CODEX_HOME", "/home/agent/.codex-acct")]);
        folders.ready(&codex).await.unwrap();

        let folder = folders.home.join(".codex-acct");
        assert_eq!(mode(&folder), 0o700);
        let config = folder.join("config.toml");
        assert_eq!(mode(&config), 0o600);
        let text = fs::read_to_string(&config).unwrap();
        assert!(text.contains("[projects.\"/home/agent/work\"]"), "{text}");
        assert!(text.contains("trust_level = \"trusted\""), "{text}");
        assert_eq!(
            *owned.lock().unwrap(),
            [
                (".codex-acct".to_owned(), AGENT_UID, AGENT_UID),
                ("config.toml.maxhq-new".to_owned(), AGENT_UID, AGENT_UID),
            ]
        );
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);

        // Once ready, a second spawn leaves the file be.
        let before = fs::metadata(&config).unwrap();
        folders.ready(&codex).await.unwrap();
        let after = fs::metadata(&config).unwrap();
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mtime_nsec(), after.mtime_nsec());
        assert_eq!(owned.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_claude_spawn_answers_onboarding_and_keeps_the_other_keys() {
        let temp = TempDir::new();
        let (folders, _) = folders(temp.path());
        let folder = folders.home.join(".claude-acct");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join(".claude.json"), "{\"theme\": \"dark\"}").unwrap();
        let claude = launch(&[("CLAUDE_CONFIG_DIR", "/home/agent/.claude-acct")]);
        folders.ready(&claude).await.unwrap();

        let text = fs::read_to_string(folder.join(".claude.json")).unwrap();
        let config: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(config["hasCompletedOnboarding"], true);
        assert_eq!(config["theme"], "dark");
    }

    #[tokio::test]
    async fn a_launch_without_an_account_folder_touches_nothing() {
        let temp = TempDir::new();
        let (folders, owned) = folders(temp.path());
        folders
            .ready(&launch(&[("HOME", "/home/agent")]))
            .await
            .unwrap();
        assert_eq!(fs::read_dir(&folders.home).unwrap().count(), 0);
        assert!(owned.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_folder_outside_the_home_or_a_link_is_refused() {
        let temp = TempDir::new();
        let (folders, owned) = folders(temp.path());
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        for folder in [
            "/etc",
            "/home/agent/../root",
            "/home/agent/a/b",
            "/home/agent",
        ] {
            let err = folders
                .ready(&launch(&[("CODEX_HOME", folder)]))
                .await
                .unwrap_err();
            assert!(err.to_string().contains(folder), "{err}");
        }

        // A link in place of the folder.
        symlink(&outside, folders.home.join(".codex-link")).unwrap();
        let err = folders
            .ready(&launch(&[("CODEX_HOME", "/home/agent/.codex-link")]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't a folder"), "{err}");

        // A link in place of the config.
        let folder = folders.home.join(".claude-acct");
        fs::create_dir(&folder).unwrap();
        fs::write(outside.join("target.json"), "{}").unwrap();
        symlink(outside.join("target.json"), folder.join(".claude.json")).unwrap();
        let err = folders
            .ready(&launch(&[(
                "CLAUDE_CONFIG_DIR",
                "/home/agent/.claude-acct",
            )]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't a plain file"), "{err}");

        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
        assert_eq!(
            fs::read_to_string(outside.join("target.json")).unwrap(),
            "{}"
        );
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&folders.home).unwrap().count(), 2);
        assert!(owned.lock().unwrap().is_empty());
    }
}
