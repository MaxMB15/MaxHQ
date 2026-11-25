// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The files in a session's folder and the helpers that write them safely.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_os::fsmode;
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::time::Timestamp;
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::secret::{zero, Secret};
use crate::sys;
use crate::{Framing, Launch, Limits, RunAs};

/// What to run, written before the holder starts.
pub(crate) const LAUNCH: &str = "launch.json";
/// The holder's and the CLI's process IDs and the socket.
pub(crate) const HOLDER: &str = "holder.json";
/// Locked for as long as the holder runs.
pub(crate) const LOCK: &str = "holder.lock";
/// The output log.
pub(crate) const EVENTS: &str = "events.log";
/// The output log before the last rotation.
pub(crate) const EVENTS_OLD: &str = "events.log.1";
/// The prompts the CLI waits on.
pub(crate) const PENDING: &str = "pending.json";
/// The end of the CLI's stderr.
pub(crate) const STDERR: &str = "stderr.log";
/// How the CLI exited.
pub(crate) const EXIT: &str = "exit.json";
/// Made once a client has seen the exit.
pub(crate) const SEEN: &str = "exit.seen";
/// The holder's own messages once it's running.
pub(crate) const HOLDER_LOG: &str = "holder.log";
/// The key a client shows the holder in its hello.
pub(crate) const KEY: &str = "holder.key";

/// A path or other OS string in JSON: a plain string when it's UTF-8, and
/// `{"bytes": [...]}` when it isn't, so nothing is lost either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OsBytes(pub OsString);

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum OsRepr {
    Text(String),
    Bytes { bytes: Vec<u8> },
}

impl Serialize for OsBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.to_str() {
            Some(text) => OsRepr::Text(text.to_owned()),
            None => OsRepr::Bytes {
                bytes: sys::os_bytes(&self.0).into_owned(),
            },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OsBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(match OsRepr::deserialize(deserializer)? {
            OsRepr::Text(text) => text.into(),
            OsRepr::Bytes { bytes } => sys::os_from_bytes(bytes)
                .ok_or_else(|| D::Error::custom("the bytes aren't a name this OS takes"))?,
        }))
    }
}

impl From<&Path> for OsBytes {
    fn from(path: &Path) -> Self {
        Self(path.as_os_str().to_owned())
    }
}

/// `launch.json`: the launch plus what the holder needs to know about where
/// to put things.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LaunchFile {
    pub session: SessionId,
    pub program: OsBytes,
    pub args: Vec<OsBytes>,
    pub env: Vec<(OsBytes, OsBytes)>,
    pub env_remove: Vec<OsBytes>,
    /// The names of the secret variables, whose values come on the
    /// holder's stdin and are never written here.
    #[serde(default)]
    pub secret_env: Vec<OsBytes>,
    #[serde(default)]
    pub run_as: Option<RunAsFile>,
    /// Whether the holder's key comes on its stdin ahead of the secret
    /// variables, so the holder turns away clients that don't show it. A
    /// spawner from before keys leaves it out, and its holder takes
    /// clients as holders did then.
    #[serde(default)]
    pub keyed: bool,
    pub cwd: OsBytes,
    pub framing: Framing,
    /// The folder to put the socket in.
    pub sockets: OsBytes,
    pub rotate_at: u64,
    pub stderr_cap: u64,
    pub stderr_tail: usize,
    pub memory_tail: usize,
    pub linger_ms: u64,
}

impl LaunchFile {
    /// The file for `launch`, with sockets in `sockets` and the holder's
    /// key on its stdin.
    pub fn new(launch: &Launch, sockets: &Path, limits: &Limits) -> Self {
        let os = |value: &OsString| OsBytes(value.clone());
        Self {
            session: launch.session,
            program: launch.program.as_path().into(),
            args: launch.args.iter().map(os).collect(),
            env: launch.env.iter().map(|(k, v)| (os(k), os(v))).collect(),
            env_remove: launch.env_remove.iter().map(os).collect(),
            secret_env: launch.secret_env.iter().map(|(k, _)| os(k)).collect(),
            run_as: launch.run_as.as_ref().map(RunAsFile::new),
            keyed: true,
            cwd: launch.cwd.as_path().into(),
            framing: launch.framing,
            sockets: sockets.into(),
            rotate_at: limits.rotate_at,
            stderr_cap: limits.stderr_cap,
            stderr_tail: limits.stderr_tail,
            memory_tail: limits.memory_tail,
            linger_ms: u64::try_from(limits.linger.as_millis()).unwrap_or(u64::MAX),
        }
    }

    /// The limits the holder runs with.
    pub fn limits(&self) -> Limits {
        Limits {
            rotate_at: self.rotate_at,
            stderr_cap: self.stderr_cap,
            stderr_tail: self.stderr_tail,
            memory_tail: self.memory_tail,
            linger: Duration::from_millis(self.linger_ms),
        }
    }
}

/// The user in `launch.json` for the CLI to run as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunAsFile {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub home: OsBytes,
}

impl RunAsFile {
    fn new(run_as: &RunAs) -> Self {
        Self {
            user: run_as.user.clone(),
            uid: run_as.uid,
            gid: run_as.gid,
            home: run_as.home.as_path().into(),
        }
    }

    /// The user as the launch gave it.
    pub fn run_as(&self) -> RunAs {
        RunAs {
            user: self.user.clone(),
            uid: self.uid,
            gid: self.gid,
            home: PathBuf::from(self.home.0.clone()),
        }
    }
}

/// `holder.json`, written once the CLI is running.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct HolderFile {
    pub holder_pid: u32,
    pub child_pid: u32,
    pub socket: OsBytes,
    pub started_at: Timestamp,
    /// When the holder's process started, as `maxhq_os::proc::started`
    /// gives it, so a process that later gets its ID isn't taken for it.
    /// Holders from before it leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_started: Option<u64>,
    /// The same for the program the holder runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_started: Option<u64>,
    /// The holder couldn't leave its spawner's job on Windows, so ending
    /// that job ends it too, and it won't outlive a `maxd` whose job goes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_spawner_job: bool,
}

impl HolderFile {
    /// The socket path.
    pub fn socket(&self) -> PathBuf {
        PathBuf::from(self.socket.0.clone())
    }
}

/// One entry in `pending.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingEntry {
    /// The request ID the answer has to name.
    pub key: String,
    pub seq: u64,
    pub at: Timestamp,
    pub line: String,
}

/// The folder for `session` under `sessions`.
pub(crate) fn session_dir(sessions: &Path, session: &SessionId) -> PathBuf {
    sessions.join(session.to_string())
}

/// Folders readable only by this user.
pub(crate) use maxhq_os::fsmode::{private_dir, private_dir_all};

/// Deletes a session's folder. It's first moved to a hidden name, so a
/// client still writing to it, such as one marking the exit seen, can't
/// leave a file behind under the session's name.
pub(crate) fn remove_session(dir: &Path) -> io::Result<()> {
    let mut hidden = OsString::from(".");
    hidden.push(dir.file_name().unwrap_or_default());
    hidden.push(format!(".gone-{}", std::process::id()));
    let hidden = dir.with_file_name(hidden);
    match fs::rename(dir, &hidden) {
        Ok(()) => fs::remove_dir_all(&hidden),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        // Windows won't move a folder while a file in it is open, as the
        // holder's own logs are when it forgets its session. It still
        // deletes them where they are.
        #[cfg(windows)]
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => fs::remove_dir_all(dir),
        Err(err) => Err(err),
    }
}

/// Opens `path` for appending, making it readable only by this user.
pub(crate) fn append(path: &Path) -> io::Result<File> {
    fsmode::set_open_mode(OpenOptions::new().create(true).append(true), 0o600).open(path)
}

/// Empties `path`, or makes it empty, then opens it for appending as
/// [`append`] does. Windows won't let a handle that only appends cut a file
/// short, so the file is emptied through one that writes.
pub(crate) fn append_empty(path: &Path) -> io::Result<File> {
    fsmode::set_open_mode(
        OpenOptions::new().create(true).write(true).truncate(true),
        0o600,
    )
    .open(path)?;
    append(path)
}

/// Writes `value` as JSON, synced and renamed into place, so a reader sees
/// the old file or the new one and the new one survives a power cut once
/// this returns.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes, Atomic::PRIVATE)
}

/// Writes the holder's key to `holder.key` in `dir`, readable only by this
/// user, before the holder starts.
pub(crate) fn write_key(dir: &Path, key: &Secret) -> io::Result<()> {
    write_atomic(&dir.join(KEY), key.expose().as_bytes(), Atomic::PRIVATE)
}

/// The holder's key from `holder.key` in `dir`, or `None` when there's no
/// such file, as for a holder from before keys.
pub(crate) fn read_key(dir: &Path) -> io::Result<Option<Secret>> {
    match fs::read(dir.join(KEY)) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Some(Secret::new(text))),
            Err(err) => {
                zero(err.into_bytes());
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{KEY} isn't text"),
                ))
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Reads JSON from `path`, or `None` when there's no such file.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Whether a holder runs for the session in `dir`. The holder keeps
/// `holder.lock` locked, and the lock goes with the process however it
/// ends.
pub(crate) fn holder_alive(dir: &Path) -> io::Result<bool> {
    let lock = match File::open(dir.join(LOCK)) {
        Ok(lock) => lock,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    match lock.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

/// The end of the file at `path`, at most `limit` bytes, as text.
pub(crate) fn read_tail(path: &Path, limit: usize) -> io::Result<String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(err) => return Err(err),
    };
    let len = file.metadata()?.len();
    let limit = u64::try_from(limit).unwrap_or(u64::MAX);
    file.seek(SeekFrom::Start(len.saturating_sub(limit)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The sessions under `sessions` with a running holder, or finished and
/// not yet seen by any client.
pub(crate) fn list(sessions: &Path) -> io::Result<Vec<SessionId>> {
    let entries = match fs::read_dir(sessions) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(session) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        let dir = entry.path();
        let unseen = dir.join(HOLDER).exists() && !dir.join(SEEN).exists();
        if unseen || holder_alive(&dir)? {
            found.push(session);
        }
    }
    found.sort();
    Ok(found)
}

/// Removes each session's folder under `sessions` that `keep` turns down
/// and whose holder has gone, and what a removal cut short left behind.
/// Gives how many went. A folder that can't be read or removed stays.
pub(crate) fn sweep(sessions: &Path, keep: &mut dyn FnMut(SessionId) -> bool) -> io::Result<usize> {
    let entries = match fs::read_dir(sessions) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        let went = match entry.file_name().to_str() {
            Some(name) if name.starts_with('.') && name.contains(".gone-") => {
                fs::remove_dir_all(&dir).is_ok()
            }
            Some(name) => {
                name.parse::<SessionId>()
                    .is_ok_and(|session| !keep(session))
                    && holder_alive(&dir).is_ok_and(|alive| !alive)
                    && remove_session(&dir).is_ok()
            }
            None => false,
        };
        removed += usize::from(went);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_strings_survive_json_whether_or_not_they_are_utf8() {
        let text = OsBytes(OsString::from("/usr/bin/claude"));
        assert_eq!(serde_json::to_string(&text).unwrap(), "\"/usr/bin/claude\"");
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStringExt;
            OsBytes(OsString::from_vec(vec![b'a', 0xff, b'b']))
        };
        #[cfg(windows)]
        let bytes = {
            use std::os::windows::ffi::OsStringExt;
            OsBytes(OsString::from_wide(&[97, 0xD800, 98]))
        };
        let json = serde_json::to_string(&bytes).unwrap();
        #[cfg(unix)]
        assert_eq!(json, r#"{"bytes":[97,255,98]}"#);
        #[cfg(windows)]
        assert_eq!(json, r#"{"bytes":[97,237,160,128,98]}"#);
        let back: OsBytes = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn the_key_file_is_private_and_a_missing_one_means_no_key() {
        let id = SessionId::generate().0.simple().to_string();
        let dir = std::env::temp_dir().join(format!("hold-key-{}", &id[20..]));
        private_dir_all(&dir).unwrap();
        assert!(read_key(&dir).unwrap().is_none());
        let key = crate::secret::new_key().unwrap();
        write_key(&dir, &key).unwrap();
        // Windows keeps the folder's own ACL, which `private_dir_all` set.
        #[cfg(unix)]
        assert_eq!(
            fsmode::mode_of(&fs::metadata(dir.join(KEY)).unwrap()) & 0o777,
            0o600
        );
        assert_eq!(read_key(&dir).unwrap(), Some(key));
        fs::write(dir.join(KEY), [0xff]).unwrap();
        let err = read_key(&dir).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_launch_file_from_before_keys_reads_as_unkeyed() {
        let launch = Launch {
            session: SessionId::generate(),
            program: "agent".into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            secret_env: Vec::new(),
            run_as: None,
            cwd: "/".into(),
            framing: Framing::ClaudeStreamJson,
        };
        let file = LaunchFile::new(&launch, Path::new("/sockets"), &Limits::default());
        assert!(file.keyed);
        let mut json = serde_json::to_value(&file).unwrap();
        json.as_object_mut().unwrap().remove("keyed");
        let old: LaunchFile = serde_json::from_value(json).unwrap();
        assert!(!old.keyed);
    }

    #[test]
    fn a_removed_session_leaves_nothing_behind() {
        let id = SessionId::generate().0.simple().to_string();
        let sessions = std::env::temp_dir().join(format!("hold-rm-{}", &id[20..]));
        let dir = sessions.join("session");
        private_dir_all(&dir).unwrap();
        write_json(&dir.join(EXIT), &1).unwrap();
        remove_session(&dir).unwrap();
        remove_session(&dir).unwrap();
        assert_eq!(fs::read_dir(&sessions).unwrap().count(), 0);
        assert!(write_json(&dir.join(EXIT), &2).is_err());
        fs::remove_dir(&sessions).unwrap();
    }

    #[test]
    fn a_sweep_takes_the_sessions_nobody_keeps_whose_holder_has_gone() {
        let id = SessionId::generate().0.simple().to_string();
        let sessions = std::env::temp_dir().join(format!("hold-sweep-{}", &id[20..]));
        let [kept, live, gone, never] = [(); 4].map(|()| SessionId::generate());
        for session in [kept, live, gone, never] {
            let dir = session_dir(&sessions, &session);
            private_dir_all(&dir).unwrap();
            fs::write(dir.join(EXIT), "{}").unwrap();
        }
        fs::write(session_dir(&sessions, &never).join(LOCK), "").unwrap();
        let lock = File::create(session_dir(&sessions, &live).join(LOCK)).unwrap();
        lock.try_lock().unwrap();
        let cut_short = sessions.join(format!(".{gone}.gone-1"));
        private_dir_all(&cut_short.join("sub")).unwrap();
        fs::write(sessions.join("notes.txt"), "").unwrap();

        let removed = sweep(&sessions, &mut |session| session == kept).unwrap();
        assert_eq!(removed, 3);
        let mut left: Vec<String> = fs::read_dir(&sessions)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut want = vec![kept.to_string(), live.to_string(), "notes.txt".to_owned()];
        want.sort();
        assert_eq!(left, want);
        drop(lock);
        assert_eq!(sweep(&sessions.join("missing"), &mut |_| false).unwrap(), 0);
        fs::remove_dir_all(&sessions).unwrap();
    }
}
