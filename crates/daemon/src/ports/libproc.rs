// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! macOS port detection through libproc, for host folder agents. It looks
//! only in the trees of the holders MaxHQ started: each holder's
//! children, theirs, and so on. It never lists every process.
//!
//! macOS has no subreaper, so a server whose parent exits moves to
//! launchd and leaves the tree. Each process found is remembered with its
//! start time and counts while it runs. A process that left its parent
//! also counts while it stays in a process group a tree process made.
//! One that leaves before any poll sees it, in a group of its own, is
//! missed.
//!
//! The Running list walks the same trees, with the same memory, so a
//! server launchd took still shows under its agent.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{c_char, c_int};
use std::mem::{size_of, MaybeUninit};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use maxhq_protocol::guest::{GuestPort, WatchedHolder};
use maxhq_protocol::ids::SessionId;
use maxhq_protocol::processes::AgentProcess;

use super::processes::{oldest_first, Seen};
use super::{merge, port_numbers, process, Found, Scanner};

/// The most processes one poll looks at, over every tree.
const MAX_TREE: usize = 4096;
/// launchd, which takes a process whose parent exits.
const LAUNCHD: u32 = 1;

/// `PROC_PIDFDSOCKETINFO` from `sys/proc_info.h`.
const PROC_PIDFDSOCKETINFO: c_int = 3;
/// The size of `struct socket_fdinfo`.
const SOCKET_INFO_SIZE: usize = 792;
/// Where `struct socket_fdinfo`'s fields sit, from its start.
const SOI_FAMILY: usize = 184;
const SOI_KIND: usize = 256;
const INSI_LPORT: usize = 268;
const INSI_VFLAG: usize = 288;
const INSI_LADDR: usize = 312;
const TCPSI_STATE: usize = 344;
/// `SOCKINFO_TCP`, `TSI_S_LISTEN`, `INI_IPV6` and `AF_INET6`.
const SOCKINFO_TCP: i32 = 2;
const TSI_S_LISTEN: i32 = 1;
const INI_IPV6: u8 = 0x2;
const AF_INET6: i32 = 30;

/// Finds listening ports in the trees of the holders it's given.
pub struct Libproc {
    memory: Memory,
}

impl Libproc {
    /// Remembers nothing yet. The sockets of this process never show.
    pub fn new() -> Self {
        Self {
            memory: Memory::new(std::process::id()),
        }
    }
}

impl Default for Libproc {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner for Libproc {
    fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort> {
        self.memory.scan(&System, holders)
    }

    fn processes(&mut self, holder: &WatchedHolder) -> Vec<AgentProcess> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        self.memory.processes(&System, holder, now)
    }
}

/// When a process started, which tells it from a later one with its pid:
/// seconds and microseconds since 1970.
type Start = (u64, u64);

/// A start time as one number, in microseconds since 1970.
fn micros(start: Start) -> u64 {
    start.0.saturating_mul(1_000_000).saturating_add(start.1)
}

/// The processes found, with their holders' sessions.
type Tree = HashMap<u32, (SessionId, Info)>;

/// What a scan needs to know about a process.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Info {
    ppid: u32,
    pgid: u32,
    start: Start,
    name: String,
}

/// What a scan asks the system. libproc answers, and tests answer from a
/// table.
trait Procs {
    fn info(&self, pid: u32) -> Option<Info>;
    fn children(&self, pid: u32) -> Vec<u32>;
    fn group(&self, pgid: u32) -> Vec<u32>;
    fn listeners(&self, pid: u32) -> Vec<(IpAddr, u16)>;
    fn command(&self, pid: u32) -> Option<String>;
    /// The CPU time it has used in milliseconds, and its resident bytes.
    fn usage(&self, pid: u32) -> Option<(u64, u64)>;
}

/// A process found in a tree on an earlier poll.
struct Known {
    start: Start,
    session: SessionId,
}

/// What one poll leaves for the next.
struct Memory {
    own: u32,
    /// The holders the last poll watched.
    holders: Vec<WatchedHolder>,
    known: HashMap<u32, Known>,
    /// Process groups a tree process made, with its holder's session and
    /// when the leader started.
    groups: HashMap<u32, (SessionId, Start)>,
    /// Each process's command line, with the start and name it was read
    /// at. An exec keeps the pid and start but changes the name, so a new
    /// name reads it again.
    commands: HashMap<u32, (Start, String, String)>,
}

impl Memory {
    fn new(own: u32) -> Self {
        Self {
            own,
            holders: Vec::new(),
            known: HashMap::new(),
            groups: HashMap::new(),
            commands: HashMap::new(),
        }
    }

    fn scan(&mut self, procs: &impl Procs, holders: &[WatchedHolder]) -> Vec<GuestPort> {
        self.holders = holders.to_vec();
        let tree = self.tree(procs, holders);
        // The holders themselves, and each one's CLI, which it starts
        // first, never show.
        let mut skip: HashSet<u32> = holders.iter().map(|holder| holder.pid).collect();
        skip.insert(self.own);
        skip.extend(
            holders
                .iter()
                .filter_map(|holder| cli_of(&tree, holder.pid)),
        );
        let mut pids: Vec<u32> = tree
            .keys()
            .copied()
            .filter(|pid| !skip.contains(pid))
            .collect();
        pids.sort_unstable();
        let mut found = Vec::new();
        for pid in pids {
            let (session, info) = &tree[&pid];
            for (ip, port) in procs.listeners(pid) {
                let command = self.command(procs, pid, info);
                found.push(Found {
                    ip,
                    port,
                    session: Some(*session),
                    process: Some(process(pid, &info.name, &command)),
                });
            }
        }
        self.remember(tree);
        merge(found)
    }

    /// The processes in `holder`'s tree as of `now`, since 1970. The
    /// trees of the holders the last poll watched are walked too, so what
    /// they found stays remembered.
    fn processes(
        &mut self,
        procs: &impl Procs,
        holder: &WatchedHolder,
        now: Duration,
    ) -> Vec<AgentProcess> {
        let mut holders: Vec<WatchedHolder> = self
            .holders
            .iter()
            .filter(|watched| watched.session != holder.session)
            .cloned()
            .collect();
        holders.push(holder.clone());
        let tree = self.tree(procs, &holders);
        let cli = cli_of(&tree, holder.pid);
        let now = u64::try_from(now.as_micros()).unwrap_or(u64::MAX);
        let mut processes = Vec::new();
        for (&pid, (session, info)) in &tree {
            if *session != holder.session || pid == holder.pid || pid == self.own {
                continue;
            }
            let (cpu_ms, memory) = procs.usage(pid).unwrap_or_default();
            let start = micros(info.start);
            processes.push(
                Seen {
                    pid,
                    parent: info.ppid,
                    start,
                    run_ms: now.saturating_sub(start) / 1000,
                    cpu_ms,
                    memory,
                    name: &info.name,
                    command: &self.command(procs, pid, info),
                    cli: cli == Some(pid),
                    ports: port_numbers(procs.listeners(pid)),
                }
                .shown(),
            );
        }
        self.remember(tree);
        oldest_first(processes)
    }

    /// Walks the trees of `holders`, with what earlier polls found.
    fn tree(&mut self, procs: &impl Procs, holders: &[WatchedHolder]) -> Tree {
        let watched: HashSet<SessionId> = holders.iter().map(|holder| holder.session).collect();
        let mut tree: Tree = HashMap::new();
        let mut queue = VecDeque::new();
        for holder in holders {
            if let Some(info) = procs.info(holder.pid) {
                tree.insert(holder.pid, (holder.session, info));
                queue.push_back(holder.pid);
            }
        }
        for (pid, known) in &self.known {
            if tree.contains_key(pid) || !watched.contains(&known.session) {
                continue;
            }
            if let Some(info) = procs.info(*pid).filter(|info| info.start == known.start) {
                tree.insert(*pid, (known.session, info));
                queue.push_back(*pid);
            }
        }
        walk(procs, &mut tree, &mut queue);

        for (pid, (session, info)) in &tree {
            if info.pgid == *pid {
                self.groups.insert(*pid, (*session, info.start));
            }
        }
        self.groups
            .retain(|_, (session, _)| watched.contains(session));
        let mut stale = Vec::new();
        for (&pgid, &(session, start)) in &self.groups {
            // A leader that started at another time is a new process with
            // the old pid, in a group of its own.
            let leader = match tree.get(&pgid) {
                Some((_, info)) => Some(info.start),
                None => procs.info(pgid).map(|info| info.start),
            };
            if leader.is_some_and(|at| at != start) {
                stale.push(pgid);
                continue;
            }
            let members = procs.group(pgid);
            if members.is_empty() {
                stale.push(pgid);
            }
            for member in members {
                if tree.len() >= MAX_TREE || tree.contains_key(&member) {
                    continue;
                }
                let orphan = procs
                    .info(member)
                    .filter(|info| info.ppid == LAUNCHD && info.pgid == pgid);
                if let Some(info) = orphan {
                    tree.insert(member, (session, info));
                    queue.push_back(member);
                }
            }
        }
        for pgid in stale {
            self.groups.remove(&pgid);
        }
        walk(procs, &mut tree, &mut queue);
        tree
    }

    /// Keeps what `tree` found for the next walk.
    fn remember(&mut self, tree: Tree) {
        self.known = tree
            .into_iter()
            .map(|(pid, (session, info))| {
                let known = Known {
                    start: info.start,
                    session,
                };
                (pid, known)
            })
            .collect();
        let known = &self.known;
        self.commands
            .retain(|pid, (start, _, _)| known.get(pid).is_some_and(|known| known.start == *start));
    }

    fn command(&mut self, procs: &impl Procs, pid: u32, info: &Info) -> String {
        if let Some((start, name, command)) = self.commands.get(&pid)
            && *start == info.start
            && *name == info.name
        {
            return command.clone();
        }
        let command = procs.command(pid).unwrap_or_default();
        self.commands
            .insert(pid, (info.start, info.name.clone(), command.clone()));
        command
    }
}

/// The CLI of the holder at `holder`: the child it started first.
fn cli_of(tree: &Tree, holder: u32) -> Option<u32> {
    tree.iter()
        .filter(|(_, (_, info))| info.ppid == holder)
        .min_by_key(|(pid, (_, info))| (info.start, **pid))
        .map(|(pid, _)| *pid)
}

/// Adds the descendants of every process in `queue` to `tree`.
fn walk(procs: &impl Procs, tree: &mut Tree, queue: &mut VecDeque<u32>) {
    while let Some(pid) = queue.pop_front() {
        let session = tree[&pid].0;
        for child in procs.children(pid) {
            if tree.len() >= MAX_TREE {
                queue.clear();
                return;
            }
            if tree.contains_key(&child) {
                continue;
            }
            if let Some(info) = procs.info(child).filter(|info| info.ppid == pid) {
                tree.insert(child, (session, info));
                queue.push_back(child);
            }
        }
    }
}

/// The system's answers, through libproc.
struct System;

impl Procs for System {
    fn info(&self, pid: u32) -> Option<Info> {
        bsd_info(pid)
    }

    fn children(&self, pid: u32) -> Vec<u32> {
        list_pids(pid, false)
    }

    fn group(&self, pgid: u32) -> Vec<u32> {
        list_pids(pgid, true)
    }

    fn listeners(&self, pid: u32) -> Vec<(IpAddr, u16)> {
        let Ok(pid) = c_int::try_from(pid) else {
            return Vec::new();
        };
        socket_fds(pid)
            .into_iter()
            .filter_map(|fd| socket_info(pid, fd))
            .filter_map(|raw| listening(&raw))
            .collect()
    }

    fn command(&self, pid: u32) -> Option<String> {
        arguments(pid).as_deref().and_then(parse_arguments)
    }

    fn usage(&self, pid: u32) -> Option<(u64, u64)> {
        task_info(pid)
    }
}

/// A process's CPU time in milliseconds and its resident bytes.
#[allow(unsafe_code)]
fn task_info(pid: u32) -> Option<(u64, u64)> {
    let pid = c_int::try_from(pid).ok()?;
    let size = size_of::<libc::proc_taskinfo>();
    let mut raw = MaybeUninit::<libc::proc_taskinfo>::zeroed();
    // SAFETY: the buffer is one `proc_taskinfo`, and the call writes at
    // most `size` bytes to it.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            raw.as_mut_ptr().cast(),
            c_int::try_from(size).ok()?,
        )
    };
    if usize::try_from(got).ok()? != size {
        return None;
    }
    // SAFETY: every field is an integer, so any bytes are a valid value.
    let raw = unsafe { raw.assume_init() };
    let ticks = raw.pti_total_user.saturating_add(raw.pti_total_system);
    Some((mach_ms(ticks), raw.pti_resident_size))
}

/// Mach time units as milliseconds. They're nanoseconds on Intel and
/// longer on Apple silicon.
fn mach_ms(ticks: u64) -> u64 {
    static BASE: OnceLock<(u64, u64)> = OnceLock::new();
    let (numer, denom) = *BASE.get_or_init(timebase);
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1));
    u64::try_from(nanos / 1_000_000).unwrap_or(u64::MAX)
}

/// The ratio of Mach time units to nanoseconds.
#[allow(unsafe_code, deprecated)]
fn timebase() -> (u64, u64) {
    let mut base = libc::mach_timebase_info { numer: 0, denom: 0 };
    // SAFETY: `base` is one `mach_timebase_info`, which the call fills.
    let code = unsafe { libc::mach_timebase_info(&mut base) };
    if code != 0 || base.numer == 0 || base.denom == 0 {
        return (1, 1);
    }
    (u64::from(base.numer), u64::from(base.denom))
}

#[allow(unsafe_code)]
fn bsd_info(pid: u32) -> Option<Info> {
    let pid = c_int::try_from(pid).ok()?;
    let size = size_of::<libc::proc_bsdinfo>();
    let mut raw = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: the buffer is one `proc_bsdinfo`, and the call writes at
    // most `size` bytes to it.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            raw.as_mut_ptr().cast(),
            c_int::try_from(size).ok()?,
        )
    };
    if usize::try_from(got).ok()? != size {
        return None;
    }
    // SAFETY: every field is an integer or an array of them, so any bytes,
    // zeroes included, are a valid value.
    let raw = unsafe { raw.assume_init() };
    let mut name = text(&raw.pbi_name);
    if name.is_empty() {
        name = text(&raw.pbi_comm);
    }
    Some(Info {
        ppid: raw.pbi_ppid,
        pgid: raw.pbi_pgid,
        start: (raw.pbi_start_tvsec, raw.pbi_start_tvusec),
        name,
    })
}

/// A process's children, or the members of a process group.
#[allow(unsafe_code)]
fn list_pids(id: u32, group: bool) -> Vec<u32> {
    let Ok(id) = libc::pid_t::try_from(id) else {
        return Vec::new();
    };
    let mut room = 256;
    loop {
        let mut pids: Vec<libc::pid_t> = vec![0; room];
        let Ok(bytes) = c_int::try_from(room * size_of::<libc::pid_t>()) else {
            return Vec::new();
        };
        // SAFETY: `pids` has room for `bytes` bytes, and the call writes at
        // most that.
        let got = unsafe {
            if group {
                libc::proc_listpgrppids(id, pids.as_mut_ptr().cast(), bytes)
            } else {
                libc::proc_listchildpids(id, pids.as_mut_ptr().cast(), bytes)
            }
        };
        let Ok(got) = usize::try_from(got) else {
            return Vec::new();
        };
        if got >= room && room < MAX_TREE {
            room *= 4;
            continue;
        }
        // The buffer starts zeroed, so the list ends at the first zero
        // whether the count is in pids or bytes.
        return pids
            .into_iter()
            .take(got.min(room))
            .take_while(|pid| *pid > 0)
            .filter_map(|pid| u32::try_from(pid).ok())
            .collect();
    }
}

/// The descriptors of a process that are sockets.
#[allow(unsafe_code)]
fn socket_fds(pid: c_int) -> Vec<c_int> {
    // SAFETY: a null buffer asks only for the size.
    let needed =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let Ok(needed) = usize::try_from(needed) else {
        return Vec::new();
    };
    let room = needed / size_of::<libc::proc_fdinfo>() + 16;
    let mut fds = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0,
        };
        room
    ];
    let Ok(bytes) = c_int::try_from(room * size_of::<libc::proc_fdinfo>()) else {
        return Vec::new();
    };
    // SAFETY: `fds` has room for `bytes` bytes, and the call writes at most
    // that.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            bytes,
        )
    };
    let Ok(got) = usize::try_from(got) else {
        return Vec::new();
    };
    fds.truncate(got / size_of::<libc::proc_fdinfo>());
    let socket = u32::try_from(libc::PROX_FDTYPE_SOCKET).unwrap_or(u32::MAX);
    fds.into_iter()
        .filter(|fd| fd.proc_fdtype == socket)
        .map(|fd| fd.proc_fd)
        .collect()
}

/// One socket's `struct socket_fdinfo`, as bytes.
#[allow(unsafe_code)]
fn socket_info(pid: c_int, fd: c_int) -> Option<Vec<u8>> {
    // Words keep the buffer aligned as the struct is.
    let mut words = [0u64; SOCKET_INFO_SIZE / 8];
    let size = c_int::try_from(SOCKET_INFO_SIZE).ok()?;
    // SAFETY: `words` is `SOCKET_INFO_SIZE` bytes, aligned to 8 like
    // `socket_fdinfo`, and the call writes at most `size` bytes.
    let got = unsafe {
        libc::proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            words.as_mut_ptr().cast(),
            size,
        )
    };
    (got == size).then(|| words.iter().flat_map(|word| word.to_ne_bytes()).collect())
}

/// The address and port of a listening TCP socket.
fn listening(raw: &[u8]) -> Option<(IpAddr, u16)> {
    let int = |at: usize| -> Option<i32> {
        Some(i32::from_ne_bytes(raw.get(at..at + 4)?.try_into().ok()?))
    };
    if int(SOI_KIND)? != SOCKINFO_TCP || int(TCPSI_STATE)? != TSI_S_LISTEN {
        return None;
    }
    // The port sits in network order in the low half of an int.
    let port = u16::from_be(u16::try_from(int(INSI_LPORT)? & 0xffff).ok()?);
    let v6 = int(SOI_FAMILY)? == AF_INET6 && raw.get(INSI_VFLAG)? & INI_IPV6 != 0;
    let ip = if v6 {
        let bytes: [u8; 16] = raw.get(INSI_LADDR..INSI_LADDR + 16)?.try_into().ok()?;
        IpAddr::V6(Ipv6Addr::from(bytes))
    } else {
        let bytes: [u8; 4] = raw.get(INSI_LADDR + 12..INSI_LADDR + 16)?.try_into().ok()?;
        IpAddr::V4(Ipv4Addr::from(bytes))
    };
    Some((ip, port))
}

/// A process's `KERN_PROCARGS2`: its argument count, path, arguments and
/// environment.
#[allow(unsafe_code)]
fn arguments(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        c_int::try_from(pid).ok()?,
    ];
    let mut size: libc::size_t = 0;
    // SAFETY: `mib` names three levels, and a null buffer asks only for the
    // size.
    let code = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if code != 0 || size == 0 {
        return None;
    }
    let mut raw = vec![0u8; size];
    // SAFETY: `raw` has room for `size` bytes, and the call writes at most
    // that and sets `size` to what it wrote.
    let code = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            raw.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if code != 0 {
        return None;
    }
    raw.truncate(size);
    Some(raw)
}

/// The arguments from a `KERN_PROCARGS2`, joined by spaces. The
/// environment after them is left alone.
fn parse_arguments(raw: &[u8]) -> Option<String> {
    let argc = usize::try_from(i32::from_ne_bytes(raw.get(..4)?.try_into().ok()?)).ok()?;
    let rest = raw.get(4..)?;
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let start = path_end + rest[path_end..].iter().position(|byte| *byte != 0)?;
    let args: Vec<Cow<'_, str>> = rest[start..]
        .split(|byte| *byte == 0)
        .take(argc)
        .map(String::from_utf8_lossy)
        .collect();
    Some(args.join(" "))
}

/// A NUL-ended name.
fn text(raw: &[c_char]) -> String {
    let bytes: Vec<u8> = raw
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| byte.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use maxhq_protocol::ids::AgentId;
    use maxhq_protocol::model::BindAddress;

    use super::*;
    use crate::testing::TempDir;

    /// A process table, which notes each pid it's asked about.
    #[derive(Default)]
    struct Table {
        procs: HashMap<u32, (Info, Vec<(IpAddr, u16)>)>,
        /// Command lines that differ from `run <pid>`.
        commands: HashMap<u32, String>,
        asked: RefCell<HashSet<u32>>,
    }

    impl Table {
        fn add(&mut self, pid: u32, ppid: u32, pgid: u32, start: u64, ports: &[u16]) {
            let info = Info {
                ppid,
                pgid,
                start: (start, 0),
                name: format!("p{pid}"),
            };
            let ports = ports
                .iter()
                .map(|port| (IpAddr::V4(Ipv4Addr::LOCALHOST), *port))
                .collect();
            self.procs.insert(pid, (info, ports));
        }
    }

    impl Procs for Table {
        fn info(&self, pid: u32) -> Option<Info> {
            self.asked.borrow_mut().insert(pid);
            self.procs.get(&pid).map(|(info, _)| info.clone())
        }
        fn children(&self, pid: u32) -> Vec<u32> {
            let mut pids: Vec<u32> = self
                .procs
                .iter()
                .filter(|(_, (info, _))| info.ppid == pid)
                .map(|(pid, _)| *pid)
                .collect();
            pids.sort_unstable();
            pids
        }
        fn group(&self, pgid: u32) -> Vec<u32> {
            self.procs
                .iter()
                .filter(|(_, (info, _))| info.pgid == pgid)
                .map(|(pid, _)| *pid)
                .collect()
        }
        fn listeners(&self, pid: u32) -> Vec<(IpAddr, u16)> {
            self.asked.borrow_mut().insert(pid);
            self.procs
                .get(&pid)
                .map(|(_, ports)| ports.clone())
                .unwrap_or_default()
        }
        fn command(&self, pid: u32) -> Option<String> {
            Some(
                self.commands
                    .get(&pid)
                    .cloned()
                    .unwrap_or_else(|| format!("run {pid}")),
            )
        }
        fn usage(&self, pid: u32) -> Option<(u64, u64)> {
            Some((u64::from(pid) * 10, u64::from(pid) * 1024))
        }
    }

    fn holder() -> WatchedHolder {
        WatchedHolder {
            session: SessionId::derive("s"),
            agent: Some(AgentId::derive("a")),
            pid: 50,
        }
    }

    /// A holder, its CLI with a listener of its own, a dev server under
    /// the CLI's shell, and a stranger.
    fn table() -> Table {
        let mut t = Table::default();
        t.add(50, 1, 50, 90, &[]);
        t.add(51, 50, 51, 100, &[41_000]);
        t.add(60, 51, 60, 200, &[]);
        t.add(61, 60, 60, 210, &[5173]);
        t.add(80, 1, 80, 10, &[5432]);
        t
    }

    fn numbers(ports: &[GuestPort]) -> Vec<u16> {
        ports.iter().map(|port| port.port).collect()
    }

    #[test]
    fn the_holders_tree_shows_but_not_its_cli_or_strangers() {
        let t = table();
        let mut memory = Memory::new(99);
        let ports = memory.scan(&t, &[holder()]);
        assert_eq!(numbers(&ports), [5173]);
        let found = ports[0].process.as_ref().unwrap();
        assert_eq!(
            (found.pid, found.name.as_str(), found.command.as_str()),
            (61, "p61", "run 61")
        );
        assert_eq!(ports[0].session, Some(SessionId::derive("s")));
        assert!(
            !t.asked.borrow().contains(&80),
            "a process outside the tree was asked about"
        );
    }

    #[test]
    fn a_server_whose_parent_exits_stays_while_it_runs() {
        let mut t = table();
        let mut memory = Memory::new(99);
        memory.scan(&t, &[holder()]);
        // The shell exits, and the server moves to launchd in a group of
        // its own.
        t.procs.remove(&60);
        t.add(61, 1, 61, 210, &[5173]);
        assert_eq!(numbers(&memory.scan(&t, &[holder()])), [5173]);
        // A new process with the same pid isn't the server.
        t.add(61, 1, 61, 999, &[5173]);
        assert!(memory.scan(&t, &[holder()]).is_empty());
        assert!(memory.scan(&table(), &[]).is_empty());
        assert!(memory.known.is_empty());
    }

    #[test]
    fn a_server_left_in_a_trees_group_is_found_unseen() {
        let mut t = table();
        // The shell's group outlives it, with a server launchd took.
        t.add(70, 1, 60, 300, &[3000]);
        // A stranger in a group the tree never made.
        t.add(81, 1, 80, 20, &[8080]);
        let mut memory = Memory::new(99);
        assert_eq!(numbers(&memory.scan(&t, &[holder()])), [3000, 5173]);
        assert!(!t.asked.borrow().contains(&81));
    }

    #[test]
    fn the_running_list_has_the_tree_and_its_group_but_not_the_holder() {
        let mut t = table();
        t.add(70, 1, 60, 300, &[3000]);
        let mut memory = Memory::new(99);
        let listed = memory.processes(&t, &holder(), Duration::from_secs(1000));
        let rows: Vec<(u32, bool, Vec<u16>)> = listed
            .iter()
            .map(|process| (process.pid, process.cli, process.ports.clone()))
            .collect();
        assert_eq!(
            rows,
            [
                (51, true, vec![41_000]),
                (60, false, vec![]),
                (61, false, vec![5173]),
                (70, false, vec![3000]),
            ]
        );
        let server = &listed[2];
        assert_eq!(
            (server.parent, server.name.as_str(), server.command.as_str()),
            (60, "p61", "run 61")
        );
        assert_eq!(
            (server.start, server.run_ms, server.cpu_ms, server.memory),
            (210_000_000, 790_000, 610, 61 * 1024)
        );
        assert!(
            !t.asked.borrow().contains(&80),
            "a process outside the tree was asked about"
        );
    }

    #[test]
    fn a_process_that_execs_shows_its_new_command() {
        let mut t = table();
        t.commands.insert(61, "sh -c serve".into());
        let mut memory = Memory::new(99);
        let command = |memory: &mut Memory, t: &Table| {
            let listed = memory.processes(t, &holder(), Duration::from_secs(1000));
            listed[2].command.clone()
        };
        assert_eq!(command(&mut memory, &t), "sh -c serve");
        // Read once while the process runs as it was.
        t.commands.insert(61, "changed".into());
        assert_eq!(command(&mut memory, &t), "sh -c serve");
        // The shell execs the server: same pid and start, a new name.
        t.procs.get_mut(&61).unwrap().0.name = "serve".into();
        t.commands.insert(61, "serve --port 5173".into());
        assert_eq!(command(&mut memory, &t), "serve --port 5173");
    }

    #[test]
    fn listing_one_holder_keeps_what_the_poll_remembers_of_the_others() {
        let mut t = table();
        t.add(90, 1, 90, 95, &[]);
        t.add(91, 90, 91, 400, &[]);
        t.add(92, 91, 91, 410, &[8080]);
        let other = WatchedHolder {
            session: SessionId::derive("o"),
            agent: Some(AgentId::derive("b")),
            pid: 90,
        };
        let mut memory = Memory::new(99);
        memory.scan(&t, &[holder(), other.clone()]);
        // The shell exits between polls, and launchd takes the server.
        t.procs.remove(&60);
        t.add(61, 1, 61, 210, &[5173]);
        let listed = memory.processes(&t, &other, Duration::from_secs(1000));
        let pids: Vec<u32> = listed.iter().map(|process| process.pid).collect();
        assert_eq!(pids, [91, 92]);
        assert_eq!(numbers(&memory.scan(&t, &[holder(), other])), [5173, 8080]);
    }

    #[test]
    fn listening_sockets_parse_from_socket_fdinfo() {
        let mut raw = vec![0u8; SOCKET_INFO_SIZE];
        let put = |raw: &mut Vec<u8>, at: usize, value: i32| {
            raw[at..at + 4].copy_from_slice(&value.to_ne_bytes());
        };
        put(&mut raw, SOI_KIND, SOCKINFO_TCP);
        put(&mut raw, TCPSI_STATE, TSI_S_LISTEN);
        put(&mut raw, SOI_FAMILY, 2);
        put(&mut raw, INSI_LPORT, i32::from(5173u16.to_be()));
        raw[INSI_LADDR + 12..INSI_LADDR + 16].copy_from_slice(&[127, 0, 0, 1]);
        assert_eq!(
            listening(&raw),
            Some((IpAddr::V4(Ipv4Addr::LOCALHOST), 5173))
        );

        put(&mut raw, SOI_FAMILY, AF_INET6);
        raw[INSI_VFLAG] = INI_IPV6;
        raw[INSI_LADDR..INSI_LADDR + 16].copy_from_slice(&Ipv6Addr::LOCALHOST.octets());
        assert_eq!(
            listening(&raw),
            Some((IpAddr::V6(Ipv6Addr::LOCALHOST), 5173))
        );

        put(&mut raw, TCPSI_STATE, 4);
        assert_eq!(listening(&raw), None);
        assert_eq!(listening(&raw[..100]), None);
    }

    #[test]
    fn arguments_parse_and_the_environment_is_left_out() {
        let mut raw = 3i32.to_ne_bytes().to_vec();
        raw.extend(b"/usr/bin/node\0\0\0node\0vite\0--port 5173\0TOKEN=secret\0");
        assert_eq!(
            parse_arguments(&raw).as_deref(),
            Some("node vite --port 5173")
        );
        assert_eq!(parse_arguments(b"\x01"), None);
    }

    /// Named in [`HELPER`]'s environment, the folder a helper reports to.
    const HELPER: &str = "MAXHQ_TEST_PORTS_HELPER";

    /// Not a test on its own: the live test runs this binary again with
    /// [`HELPER`] set, and this listens on loopback until told to stop.
    #[test]
    fn helper_listener() {
        let Some(dir) = std::env::var_os(HELPER).map(PathBuf::from) else {
            return;
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        fs::write(
            dir.join("port.new"),
            format!("{port} {}", std::process::id()),
        )
        .unwrap();
        fs::rename(dir.join("port.new"), dir.join("port")).unwrap();
        // The test's folder going counts as stop too.
        let until = Instant::now() + Duration::from_secs(30);
        while dir.is_dir() && !dir.join("stop").exists() && Instant::now() < until {
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Writes the stop file when the test ends, however it ends, so every
    /// process it started exits by itself.
    struct Stop {
        dir: PathBuf,
        holder: Child,
        /// The listener, once it says who it is.
        helper: Option<u32>,
    }

    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = fs::write(self.dir.join("stop"), "");
            let _ = self.holder.wait();
            if let Some(helper) = self.helper {
                let until = Instant::now() + Duration::from_secs(5);
                while bsd_info(helper).is_some() && Instant::now() < until {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    fn wait_for(what: &str, check: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !check() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    /// A shell stands in for a holder and another for its CLI, which starts
    /// a listener and exits. Only processes this test started are looked
    /// at, and each exits once the stop file appears.
    #[test]
    fn a_listener_in_a_real_holders_tree_is_found() {
        let temp = TempDir::new();
        let dir = temp.path().to_path_buf();
        let wait =
            "i=0; while [ ! -f \"$D/$1\" ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done";
        let cli = script(
            &dir,
            "cli.sh",
            &format!(
                "D=\"$1\"; \"$2\" --exact ports::libproc::tests::helper_listener --test-threads=1 >/dev/null 2>&1 &\nset -- go\n{wait}\n"
            ),
        );
        let holder_script = script(
            &dir,
            "holder.sh",
            &format!("D=\"$1\"; /bin/sh \"$2\" \"$1\" \"$3\"\nset -- stop\n{wait}\n"),
        );
        let exe = std::env::current_exe().unwrap();
        let holder = Command::new("/bin/sh")
            .arg(&holder_script)
            .arg(&dir)
            .arg(&cli)
            .arg(&exe)
            .env(HELPER, &dir)
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let holder_pid = holder.id();
        let mut stop = Stop {
            dir: dir.clone(),
            holder,
            helper: None,
        };

        wait_for("the helper's port", || dir.join("port").exists());
        let report = fs::read_to_string(dir.join("port")).unwrap();
        let (port, helper) = report.split_once(' ').unwrap();
        let (port, helper): (u16, u32) = (port.parse().unwrap(), helper.parse().unwrap());
        stop.helper = Some(helper);
        let watched = WatchedHolder {
            session: SessionId::derive("live"),
            agent: Some(AgentId::derive("live")),
            pid: holder_pid,
        };

        let mut scanner = Libproc::new();
        let ports = scanner.scan(std::slice::from_ref(&watched));
        let ours: Vec<&GuestPort> = ports.iter().filter(|found| found.port == port).collect();
        assert_eq!(ours.len(), 1, "{ports:?}");
        assert_eq!(ours[0].bind, BindAddress::Loopback);
        assert_eq!(ours[0].session, Some(watched.session));
        let found = ours[0].process.as_ref().unwrap();
        assert_eq!(found.pid, helper);
        assert!(
            found.command.contains("helper_listener"),
            "{}",
            found.command
        );

        // The CLI exits, and launchd takes the listener.
        fs::write(dir.join("go"), "").unwrap();
        wait_for("the listener to leave the tree", || {
            bsd_info(helper).is_some_and(|info| info.ppid == LAUNCHD)
        });
        let again = scanner.scan(std::slice::from_ref(&watched));
        assert!(again.iter().any(|found| found.port == port), "{again:?}");
        // A scanner that never saw it finds it in the holder's group.
        let fresh = Libproc::new().scan(std::slice::from_ref(&watched));
        assert!(fresh.iter().any(|found| found.port == port), "{fresh:?}");
    }
}
