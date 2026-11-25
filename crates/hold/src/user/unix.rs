// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Starting the CLI as another user, for a holder that runs as root.
//!
//! What `initgroups` does is split around the fork. The user's groups, its
//! shell and the CLI's folder are worked out first. The forked process then
//! only calls `setgroups`, `setgid`, `setuid` and `chdir`, in that order,
//! because a fork of a process with threads may only make plain system
//! calls before it runs the CLI. Entering the folder last means the CLI can
//! only start in a folder its user can reach.

use std::ffi::{CStr, CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::RunAs;

/// The shell for a user whose record names none.
const FALLBACK_SHELL: &str = "/bin/sh";

/// The largest buffer worth trying for one user's record.
const MAX_RECORD: usize = 1024 * 1024;

/// The most groups Linux lets a process have.
const MAX_GROUPS: libc::c_int = 65_536;

/// A user's record in the user database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub shell: Option<PathBuf>,
}

/// The record for `user`, or `None` when there's no such user.
pub(crate) fn lookup(user: &str) -> io::Result<Option<Entry>> {
    let name = CString::new(user)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a user name has a zero byte"))?;
    lookup_c(&name)
}

#[allow(unsafe_code)]
fn lookup_c(name: &CStr) -> io::Result<Option<Entry>> {
    let mut buf: Vec<libc::c_char> = vec![0; 4096];
    loop {
        // SAFETY: `passwd` is plain data, and all zeroes is a valid value
        // for every field.
        let mut record: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call, and the length is
        // the buffer's own.
        let code = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut record,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        if code == libc::ERANGE && buf.len() < MAX_RECORD {
            let len = buf.len() * 2;
            buf.resize(len, 0);
            continue;
        }
        // Some systems report a missing user as an error with no record.
        if found.is_null() && matches!(code, 0 | libc::ENOENT | libc::ESRCH) {
            return Ok(None);
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        let text = |ptr: *const libc::c_char| -> Option<PathBuf> {
            if ptr.is_null() {
                return None;
            }
            // SAFETY: on success the record's strings point into `buf`,
            // which is still alive, and each ends with a zero byte.
            let bytes = unsafe { CStr::from_ptr(ptr) }.to_bytes();
            (!bytes.is_empty()).then(|| OsString::from_vec(bytes.to_vec()).into())
        };
        return Ok(Some(Entry {
            uid: record.pw_uid,
            gid: record.pw_gid,
            home: text(record.pw_dir).unwrap_or_else(|| PathBuf::from("/")),
            shell: text(record.pw_shell),
        }));
    }
}

/// The groups `user` is in, with `gid` among them, as `initgroups` would
/// set them.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn group_list(user: &CStr, gid: u32) -> io::Result<Vec<libc::gid_t>> {
    let mut room: libc::c_int = 64;
    loop {
        let mut groups: Vec<libc::gid_t> = vec![0; usize::try_from(room).unwrap_or(0)];
        let mut count = room;
        // SAFETY: `groups` has room for `count` entries.
        let code =
            unsafe { libc::getgrouplist(user.as_ptr(), gid, groups.as_mut_ptr(), &mut count) };
        if code >= 0 {
            groups.truncate(usize::try_from(count).unwrap_or(0));
            return Ok(groups);
        }
        if room >= MAX_GROUPS {
            return Err(io::Error::other("the user is in too many groups"));
        }
        room = count.max(room * 2).min(MAX_GROUPS);
    }
}

/// The groups `user` is in, with `gid` among them. macOS passes them as
/// signed numbers.
#[cfg(not(target_os = "linux"))]
#[allow(unsafe_code)]
fn group_list(user: &CStr, gid: u32) -> io::Result<Vec<libc::gid_t>> {
    let mut room: libc::c_int = 64;
    loop {
        let mut groups: Vec<libc::c_int> = vec![0; usize::try_from(room).unwrap_or(0)];
        let mut count = room;
        // SAFETY: `groups` has room for `count` entries.
        let code = unsafe {
            libc::getgrouplist(
                user.as_ptr(),
                gid.cast_signed(),
                groups.as_mut_ptr(),
                &mut count,
            )
        };
        if code >= 0 {
            groups.truncate(usize::try_from(count).unwrap_or(0));
            return Ok(groups.into_iter().map(i32::cast_unsigned).collect());
        }
        if room >= MAX_GROUPS {
            return Err(io::Error::other("the user is in too many groups"));
        }
        room = count.max(room * 2).min(MAX_GROUPS);
    }
}

/// What the forked process needs to become the user, worked out before
/// the fork.
pub(crate) struct Prepared {
    uid: u32,
    gid: u32,
    groups: Vec<libc::gid_t>,
    cwd: CString,
    /// `HOME`, `USER`, `LOGNAME` and `SHELL` for the user.
    pub env: Vec<(OsString, OsString)>,
    /// The shell the user's record names, if it names one.
    pub shell: Option<PathBuf>,
}

/// Gets ready to start the CLI as `run_as` in `cwd`. Only a holder running
/// as root can.
pub(crate) fn prepare(run_as: &RunAs, cwd: &Path) -> Result<Prepared, String> {
    let user = &run_as.user;
    if !rustix::process::geteuid().is_root() {
        return Err(format!(
            "the agent can only run as {user} under a holder running as root"
        ));
    }
    let name = CString::new(user.as_str()).map_err(|_| "a user name has a zero byte".to_owned())?;
    let entry =
        lookup_c(&name).map_err(|err| format!("couldn't look up the user {user}: {err}"))?;
    let shell = match &entry {
        Some(entry) if entry.uid != run_as.uid => {
            return Err(format!(
                "the user {user} is uid {}, not {}",
                entry.uid, run_as.uid
            ));
        }
        Some(entry) => entry.shell.clone(),
        None => None,
    };
    let groups = match entry {
        Some(_) => group_list(&name, run_as.gid)
            .map_err(|err| format!("couldn't list the groups of {user}: {err}"))?,
        None => vec![run_as.gid],
    };
    let cwd = CString::new(cwd.as_os_str().as_bytes())
        .map_err(|_| format!("the folder {} has a zero byte", cwd.display()))?;
    let login = shell
        .clone()
        .unwrap_or_else(|| PathBuf::from(FALLBACK_SHELL));
    let env = vec![
        ("HOME".into(), run_as.home.clone().into_os_string()),
        ("USER".into(), user.into()),
        ("LOGNAME".into(), user.into()),
        ("SHELL".into(), login.into_os_string()),
    ];
    Ok(Prepared {
        uid: run_as.uid,
        gid: run_as.gid,
        groups,
        cwd,
        env,
        shell,
    })
}

impl Prepared {
    /// Has `command` drop to the user in the forked process, after its
    /// process group is set and before it runs the CLI. The command
    /// mustn't have a folder of its own, since this enters the CLI's.
    #[allow(unsafe_code)]
    pub(crate) fn apply(self, command: &mut Command) -> Result<(), String> {
        let Self {
            uid,
            gid,
            groups,
            cwd,
            ..
        } = self;
        // `setgroups` takes a `size_t` on Linux and a `c_int` on macOS.
        #[allow(clippy::useless_conversion)]
        let count = groups
            .len()
            .try_into()
            .map_err(|_| "the user is in too many groups".to_owned())?;
        let become_user = move || -> io::Result<()> {
            let check = |code: libc::c_int| {
                if code == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            };
            // SAFETY: each pointer is to memory the closure owns, and these
            // are plain system calls, which are safe in a forked process.
            unsafe {
                check(libc::setgroups(count, groups.as_ptr()))?;
                check(libc::setgid(gid))?;
                check(libc::setuid(uid))?;
                check(libc::chdir(cwd.as_ptr()))
            }
        };
        // SAFETY: the closure allocates nothing and takes no locks. It only
        // makes system calls and reads `errno`.
        unsafe {
            command.pre_exec(become_user);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_in_the_user_database() {
        let root = lookup("root").unwrap().expect("a root user");
        assert_eq!((root.uid, root.gid), (0, 0));
        assert!(root.home.is_absolute(), "{root:?}");
        let groups = group_list(&CString::new("root").unwrap(), 0).unwrap();
        assert!(groups.contains(&0), "{groups:?}");
    }

    #[test]
    fn a_missing_user_is_none() {
        assert_eq!(lookup("maxhq-no-such-user").unwrap(), None);
        assert!(lookup("bad\0name").is_err());
    }

    #[test]
    fn only_root_can_run_the_agent_as_someone_else() {
        let run_as = RunAs {
            user: "nobody".into(),
            uid: 65_534,
            gid: 65_534,
            home: "/nonexistent".into(),
        };
        let prepared = prepare(&run_as, Path::new("/"));
        if rustix::process::geteuid().is_root() {
            return;
        }
        let err = prepared.err().expect("a refusal");
        assert!(err.contains("running as root"), "{err}");
    }
}
