// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A daemon running as root, as the guest in a container does, reads
//! documents as the user whose they are, or as nobody when it can't tell
//! who that is, and never as root.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use maxhq_hold::RunAs;

/// Who a root daemon reads documents as when it can't tell whose they
/// are: the user and group called nobody.
const NOBODY: Reader = Reader {
    uid: 65_534,
    gid: 65_534,
};

/// How many groups the child looks at to check it has none left.
const GROUPS_SEEN: usize = 64;

/// The user and group a child that reads documents runs as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reader {
    uid: u32,
    gid: u32,
}

/// Has `command` run as `owner` when this process runs as root. See
/// [`reader`].
pub fn give_up(command: &mut Command, owner: Option<&str>) {
    if let Some(reader) = reader(running_as_root(), owner_ids(owner)) {
        run_as(command, reader);
    }
}

/// The owner's user and group, when there's an owner to find.
fn owner_ids(owner: Option<&str>) -> Option<Reader> {
    let owner = owner?;
    match RunAs::lookup(owner) {
        Ok(user) => Some(Reader {
            uid: user.uid,
            gid: user.gid,
        }),
        Err(err) => {
            tracing::debug!("couldn't find user {owner} to read documents as: {err}");
            None
        }
    }
}

/// Who the child runs as, given whether this process is root and the
/// owner's user and group. A process that isn't root has nothing to give
/// up, so the child stays as it is. Root gives the child to the owner, or
/// to nobody when there's no owner or the owner is root too.
fn reader(root: bool, owner: Option<Reader>) -> Option<Reader> {
    if !root {
        return None;
    }
    let owner = owner.filter(|owner| owner.uid != 0 && owner.gid != 0);
    Some(owner.unwrap_or(NOBODY))
}

/// Whether this process runs as root.
#[allow(unsafe_code)]
fn running_as_root() -> bool {
    // SAFETY: `geteuid` takes nothing and can't fail.
    unsafe { libc::geteuid() == 0 }
}

/// Has `command` run as `reader`, with no group but its own. The standard
/// library sets the group, clears the other groups and then sets the user
/// before the child starts, but carries on with the groups kept when the
/// system refuses to clear them. So the child checks what it ended up with
/// and fails to start, which skips the document, rather than read it with
/// more than it should.
#[allow(unsafe_code)]
fn run_as(command: &mut Command, reader: Reader) {
    let Reader { uid, gid } = reader;
    command.uid(uid).gid(gid);
    let mut groups: Vec<libc::gid_t> = vec![0; GROUPS_SEEN];
    let check = move || -> io::Result<()> {
        // SAFETY: the buffer is the closure's own and holds `GROUPS_SEEN`
        // entries, and these are plain system calls, which are safe in a
        // forked process.
        let (count, ids) = unsafe {
            let count = libc::getgroups(GROUPS_SEEN as libc::c_int, groups.as_mut_ptr());
            let ids = (
                libc::getuid(),
                libc::geteuid(),
                libc::getgid(),
                libc::getegid(),
            );
            (count, ids)
        };
        let count = usize::try_from(count).map_err(|_| io::Error::last_os_error())?;
        let refused = || io::Error::from_raw_os_error(libc::EPERM);
        let kept = groups.get(..count).ok_or_else(refused)?;
        if ids != (uid, uid, gid, gid) || kept.iter().any(|&group| group != gid) {
            return Err(refused());
        }
        Ok(())
    };
    // SAFETY: the closure allocates nothing and takes no locks. It only
    // makes system calls and reads `errno`.
    unsafe {
        command.pre_exec(check);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    #[test]
    fn root_reads_as_the_owner_or_as_nobody() {
        let agent = Reader {
            uid: 1000,
            gid: 1000,
        };
        assert_eq!(reader(false, Some(agent)), None);
        assert_eq!(reader(false, None), None);
        assert_eq!(reader(true, Some(agent)), Some(agent));
        assert_eq!(reader(true, None), Some(NOBODY));
        for root in [
            Reader { uid: 0, gid: 0 },
            Reader { uid: 0, gid: 1000 },
            Reader { uid: 1000, gid: 0 },
        ] {
            assert_eq!(reader(true, Some(root)), Some(NOBODY), "{root:?}");
        }
        assert_eq!(
            NOBODY,
            Reader {
                uid: 65_534,
                gid: 65_534
            }
        );
    }

    #[test]
    fn a_child_left_with_other_groups_doesnt_start() {
        let id = |flag: &str| {
            let out = Command::new("id").arg(flag).output().unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let uid: u32 = id("-u").trim().parse().unwrap();
        let gid: u32 = id("-g").trim().parse().unwrap();
        if uid == 0 {
            return;
        }
        // Without root the child can't clear its groups, so it starts only
        // when it has none but its own.
        let alone = id("-G")
            .split_whitespace()
            .all(|group| group.parse() == Ok(gid));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "id -u"]).stdout(Stdio::piped());
        run_as(&mut command, Reader { uid, gid });
        match command.output() {
            Ok(out) => {
                assert!(alone, "the child kept other groups");
                assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), uid.to_string());
            }
            Err(err) => {
                assert!(!alone, "{err}");
                assert_eq!(err.raw_os_error(), Some(libc::EPERM), "{err}");
            }
        }
    }
}
