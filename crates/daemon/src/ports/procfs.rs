// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Linux port detection through `/proc`. `/proc/net/tcp` and `tcp6` list
//! the listening sockets in this process's network namespace by inode.
//! The links in `/proc/<pid>/fd` give each inode's process, and the
//! `PPid` chain in `/proc/<pid>/stat` leads up to a holder. A holder is a
//! subreaper on Linux, so a server whose parent exits stays in its tree.
//!
//! Only the processes in watched trees have their links read, unless the
//! scanner counts everything, as the guest does for its container. Each
//! inode's process is kept until the socket goes, so a poll reads links
//! only when a new socket appears.
//!
//! The Running list walks down from a holder through each `stat`'s
//! parent, past processes that ended but that no parent has reaped yet.
//! Run times count from boot on `CLOCK_BOOTTIME`, which a
//! container's `stat` start times share, while a container's
//! `/proc/uptime` may be its own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

use maxhq_protocol::guest::{GuestPort, WatchedHolder};
use maxhq_protocol::processes::AgentProcess;

use super::processes::{oldest_first, Seen};
use super::{merge, port_numbers, process, Found, Scanner};
use crate::diagnostics::mask::masked_args;

/// The state `/proc/net/tcp` gives a listening socket.
const LISTEN: &str = "0A";
/// How far up the parent chain a holder is looked for.
const MAX_DEPTH: usize = 64;
/// The most processes one holder's tree lists.
const MAX_TREE: usize = 4096;

/// Finds listening ports through a `/proc` tree.
pub struct Procfs {
    root: PathBuf,
    everything: bool,
    own: u32,
    /// The process found holding each listening socket, by inode.
    owners: HashMap<u64, u32>,
}

/// What a process's `stat` file gives.
struct Stat {
    name: String,
    ppid: u32,
    /// When it started, in clock ticks after boot.
    start: u64,
    /// The CPU time it has used, user and system, in clock ticks.
    cpu: u64,
    /// Its resident memory, in pages.
    rss: u64,
    /// Whether it has ended and only waits for its parent to reap it, as a
    /// command's shell does under a parent that never waits.
    ended: bool,
}

/// What turns `stat`'s ticks and pages into time and bytes.
struct Clock {
    /// Milliseconds since boot.
    now_ms: u64,
    /// Clock ticks a second.
    hz: u64,
    /// Bytes a page.
    page: u64,
}

impl Clock {
    #[allow(unsafe_code)]
    fn now() -> Self {
        // SAFETY: sysconf only reads a setting.
        let (hz, page) = unsafe {
            (
                libc::sysconf(libc::_SC_CLK_TCK),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        let positive = |value: libc::c_long| u64::try_from(value).ok().filter(|value| *value > 0);
        Self {
            now_ms: boot_ms(),
            hz: positive(hz).unwrap_or(100),
            page: positive(page).unwrap_or(4096),
        }
    }

    fn ms(&self, ticks: u64) -> u64 {
        ticks.saturating_mul(1000) / self.hz
    }
}

/// Milliseconds since boot, counting suspend, as `stat` counts start
/// times.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn boot_ms() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is one `timespec`, which the call fills.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } != 0 {
        return 0;
    }
    let secs = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    secs.saturating_mul(1000) + nanos / 1_000_000
}

/// Only Linux has a `/proc` to read, so elsewhere run times read as none.
#[cfg(not(target_os = "linux"))]
fn boot_ms() -> u64 {
    0
}

/// A listening socket in `/proc/net/tcp` or `tcp6`.
struct Socket {
    ip: IpAddr,
    port: u16,
    inode: u64,
}

impl Procfs {
    /// Reads `root`, which is `/proc` outside tests. With `everything`, a
    /// port outside every watched tree shows too, without a session. The
    /// sockets of this process never show.
    pub fn new(root: impl Into<PathBuf>, everything: bool) -> Self {
        Self {
            root: root.into(),
            everything,
            own: std::process::id(),
            owners: HashMap::new(),
        }
    }

    /// Treats `pid` as this process, for tests.
    #[cfg(test)]
    fn with_own(mut self, pid: u32) -> Self {
        self.own = pid;
        self
    }

    fn listening(&self) -> Vec<Socket> {
        ["net/tcp", "net/tcp6"]
            .iter()
            .filter_map(|table| fs::read_to_string(self.root.join(table)).ok())
            .flat_map(|text| {
                text.lines()
                    .skip(1)
                    .filter_map(parse_row)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn stats(&self) -> HashMap<u32, Stat> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return HashMap::new();
        };
        entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|pid| {
                let text = fs::read_to_string(self.root.join(pid.to_string()).join("stat")).ok()?;
                Some((pid, parse_stat(&text)?))
            })
            .collect()
    }

    /// The processes under `holder`, as of `clock`.
    fn processes_at(&mut self, holder: &WatchedHolder, clock: &Clock) -> Vec<AgentProcess> {
        let table = self.stats();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, stat) in &table {
            children.entry(stat.ppid).or_default().push(*pid);
        }
        let mut tree: Vec<u32> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::from([holder.pid]);
        let mut queue = VecDeque::from([holder.pid]);
        while let Some(pid) = queue.pop_front() {
            for child in children.get(&pid).into_iter().flatten() {
                if tree.len() < MAX_TREE
                    && *child != self.own
                    && !table[child].ended
                    && seen.insert(*child)
                {
                    tree.push(*child);
                    queue.push_back(*child);
                }
            }
        }
        tree.sort_unstable();
        // The holder's oldest child is its CLI, as in a scan.
        let cli = tree
            .iter()
            .filter(|pid| table[pid].ppid == holder.pid)
            .min_by_key(|pid| (table[pid].start, **pid))
            .copied();
        let sockets = self.listening();
        if !sockets.is_empty() {
            self.map_owners(&sockets, &table, &tree);
        }
        let processes = tree
            .iter()
            .map(|pid| {
                let stat = &table[pid];
                let owned = sockets
                    .iter()
                    .filter(|socket| self.owners.get(&socket.inode) == Some(pid))
                    .map(|socket| (socket.ip, socket.port));
                Seen {
                    pid: *pid,
                    parent: stat.ppid,
                    start: stat.start,
                    run_ms: clock.now_ms.saturating_sub(clock.ms(stat.start)),
                    cpu_ms: clock.ms(stat.cpu),
                    memory: stat.rss.saturating_mul(clock.page),
                    name: &stat.name,
                    command: &self.command(*pid),
                    cli: cli == Some(*pid),
                    ports: port_numbers(owned),
                }
                .shown()
            })
            .collect();
        oldest_first(processes)
    }

    /// Finds the process for each socket not mapped yet, reading the links
    /// of `candidates` only.
    fn map_owners(&mut self, sockets: &[Socket], table: &HashMap<u32, Stat>, candidates: &[u32]) {
        let live: HashSet<u64> = sockets.iter().map(|socket| socket.inode).collect();
        self.owners
            .retain(|inode, pid| live.contains(inode) && table.contains_key(pid));
        let mut wanted: HashSet<u64> = live
            .into_iter()
            .filter(|inode| !self.owners.contains_key(inode))
            .collect();
        for &pid in candidates {
            if wanted.is_empty() {
                break;
            }
            let Ok(fds) = fs::read_dir(self.root.join(pid.to_string()).join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Some(inode) = fs::read_link(fd.path())
                    .ok()
                    .and_then(|link| socket_inode(link.to_str()?))
                else {
                    continue;
                };
                if wanted.remove(&inode) {
                    self.owners.insert(inode, pid);
                }
            }
        }
    }

    fn command(&self, pid: u32) -> String {
        let raw = fs::read(self.root.join(pid.to_string()).join("cmdline")).unwrap_or_default();
        // A token in the arguments stays on this machine.
        masked_args(
            raw.split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(String::from_utf8_lossy),
        )
    }
}

impl Scanner for Procfs {
    fn scan(&mut self, holders: &[WatchedHolder]) -> Vec<GuestPort> {
        let sockets = self.listening();
        if sockets.is_empty() {
            self.owners.clear();
            return Vec::new();
        }
        let table = self.stats();
        let by_pid: HashMap<u32, &WatchedHolder> =
            holders.iter().map(|holder| (holder.pid, holder)).collect();
        let under = |pid: u32| -> Option<&WatchedHolder> {
            let mut at = pid;
            for _ in 0..MAX_DEPTH {
                if let Some(holder) = by_pid.get(&at) {
                    return Some(holder);
                }
                at = table.get(&at)?.ppid;
                if at <= 1 {
                    return None;
                }
            }
            None
        };
        // Each holder runs its CLI first, so the oldest child is the CLI.
        // Servers left behind by a process that exited start later.
        let clis: HashSet<u32> = holders
            .iter()
            .filter_map(|holder| {
                table
                    .iter()
                    .filter(|(_, stat)| stat.ppid == holder.pid)
                    .min_by_key(|(pid, stat)| (stat.start, **pid))
                    .map(|(pid, _)| *pid)
            })
            .collect();
        let mut candidates: Vec<u32> = table
            .keys()
            .copied()
            // This process is read too when everything counts, so its own
            // sockets are known and dropped.
            .filter(|pid| self.everything || under(*pid).is_some())
            .collect();
        candidates.sort_unstable();
        self.map_owners(&sockets, &table, &candidates);

        let mut found = Vec::new();
        for socket in sockets {
            let owner = self.owners.get(&socket.inode).copied();
            if owner.is_some_and(|pid| pid == self.own || clis.contains(&pid)) {
                continue;
            }
            let holder = owner.and_then(under);
            if holder.is_none() && !self.everything {
                continue;
            }
            found.push(Found {
                ip: socket.ip,
                port: socket.port,
                session: holder.map(|holder| holder.session),
                process: owner.map(|pid| process(pid, &table[&pid].name, &self.command(pid))),
            });
        }
        merge(found)
    }

    fn processes(&mut self, holder: &WatchedHolder) -> Vec<AgentProcess> {
        self.processes_at(holder, &Clock::now())
    }
}

/// A listening row of `/proc/net/tcp` or `tcp6`.
fn parse_row(line: &str) -> Option<Socket> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.get(3) != Some(&LISTEN) {
        return None;
    }
    let (ip, port) = fields.get(1)?.split_once(':')?;
    let inode = fields.get(9)?.parse().ok().filter(|inode| *inode != 0)?;
    Some(Socket {
        ip: parse_ip(ip)?,
        port: u16::from_str_radix(port, 16).ok()?,
        inode,
    })
}

/// An address as the kernel prints it: each 32-bit word in hex, in the
/// machine's byte order.
fn parse_ip(hex: &str) -> Option<IpAddr> {
    let word = |at: usize| -> Option<[u8; 4]> {
        let text = hex.get(at..at + 8)?;
        Some(u32::from_str_radix(text, 16).ok()?.to_ne_bytes())
    };
    match hex.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(0)?))),
        32 => {
            let mut bytes = [0; 16];
            for (n, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *chunk = word(n * 8)?;
            }
            Some(IpAddr::V6(Ipv6Addr::from(bytes)))
        }
        _ => None,
    }
}

/// A process's name, parent, start time, CPU time and memory from its
/// `stat` line. The name sits in parentheses and may hold anything, so the
/// fields after it are found from the last one.
fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_owned();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let number = |at: usize| -> u64 {
        rest.get(at)
            .and_then(|field| field.parse().ok())
            .unwrap_or(0)
    };
    Some(Stat {
        name,
        ppid: rest.get(1)?.parse().ok()?,
        start: rest.get(19)?.parse().ok()?,
        cpu: number(11) + number(12),
        rss: number(21),
        // Z is a zombie; X and x are dead, which a read seldom catches.
        ended: matches!(rest.first().copied(), Some("Z" | "X" | "x")),
    })
}

/// The inode in a link like `socket:[12345]`.
fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use maxhq_protocol::ids::{AgentId, SessionId};
    use maxhq_protocol::model::BindAddress;

    use super::*;
    use crate::testing::TempDir;

    /// A `/proc` tree on disk.
    struct Fixture {
        dir: TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new();
            fs::create_dir(dir.path().join("net")).unwrap();
            Self { dir }
        }

        fn process(
            &self,
            pid: u32,
            ppid: u32,
            start: u64,
            name: &str,
            args: &[&str],
            inodes: &[u64],
        ) {
            let at = self.dir.path().join(pid.to_string());
            fs::create_dir_all(at.join("fd")).unwrap();
            // The real format, with a name that holds a space and a parenthesis.
            let stat = format!(
                "{pid} ({name}) S {ppid} {pid} {pid} 0 -1 4194560 100 0 0 0 1 1 0 0 20 0 1 0 {start} 1000 100 0"
            );
            fs::write(at.join("stat"), stat).unwrap();
            let mut cmdline = args.join("\0");
            cmdline.push('\0');
            fs::write(at.join("cmdline"), cmdline).unwrap();
            symlink("/dev/null", at.join("fd/0")).unwrap();
            symlink("pipe:[77]", at.join("fd/1")).unwrap();
            for (n, inode) in inodes.iter().enumerate() {
                symlink(
                    format!("socket:[{inode}]"),
                    at.join("fd").join((n + 3).to_string()),
                )
                .unwrap();
            }
        }

        fn table(&self, name: &str, rows: &[(&str, &str, u64)]) {
            let mut text = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n".to_owned();
            for (n, (local, state, inode)) in rows.iter().enumerate() {
                let remote = if local.len() > 13 {
                    "00000000000000000000000000000000:0000"
                } else {
                    "00000000:0000"
                };
                text.push_str(&format!(
                    "{n:4}: {local} {remote} {state} 00000000:00000000 00:00000000 00000000  1000        0 {inode} 1 0000000000000000 100 0 0 10 0\n"
                ));
            }
            fs::write(self.dir.path().join("net").join(name), text).unwrap();
        }

        fn scanner(&self, everything: bool) -> Procfs {
            Procfs::new(self.dir.path(), everything).with_own(99)
        }
    }

    fn holder() -> WatchedHolder {
        WatchedHolder {
            session: SessionId::derive("s"),
            agent: Some(AgentId::derive("a")),
            pid: 50,
        }
    }

    /// A holder whose CLI's shell started a dev server, a server left
    /// behind and adopted by the holder, and sockets outside the tree.
    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.process(1, 0, 1, "systemd", &["/sbin/init"], &[]);
        f.process(50, 1, 90, "maxd-hold", &["maxd", "hold"], &[]);
        f.process(51, 50, 100, "claude", &["claude", "--resume"], &[1001]);
        f.process(60, 51, 200, "bash", &["bash", "-c", "npm run dev"], &[]);
        f.process(
            61,
            60,
            210,
            "node (vite)",
            &["node", "vite", "--port", "5173"],
            &[1002, 1003],
        );
        f.process(
            70,
            50,
            300,
            "python3",
            &["python3", "-m", "http.server", "3000"],
            &[1004],
        );
        f.process(
            80,
            1,
            20,
            "postgres",
            &["postgres", "-D", "/var/lib/pg"],
            &[1005],
        );
        f.process(81, 1, 10, "sshd", &["sshd"], &[1006]);
        f.process(82, 1, 10, "systemd-resolve", &["systemd-resolved"], &[1007]);
        f.process(99, 1, 30, "maxd", &["maxd", "guest"], &[1008]);
        f.table(
            "tcp",
            &[
                ("0100007F:AFC8", "0A", 1001),
                ("0100007F:1435", "0A", 1002),
                ("00000000:0BB8", "0A", 1004),
                ("00000000:1538", "0A", 1005),
                ("00000000:0016", "0A", 1006),
                ("3500007F:0035", "0A", 1007),
                ("0100007F:1B58", "0A", 1008),
                ("0100007F:2328", "0A", 1009),
                // A connection, not a listener.
                ("0100007F:1435", "01", 2000),
            ],
        );
        f.table(
            "tcp6",
            &[
                ("00000000000000000000000001000000:1435", "0A", 1003),
                ("00000000000000000000000000000000:0016", "0A", 1006),
            ],
        );
        f
    }

    fn summary(ports: &[GuestPort]) -> Vec<(u16, BindAddress, bool, Option<u32>)> {
        ports
            .iter()
            .map(|port| {
                (
                    port.port,
                    port.bind.clone(),
                    port.session.is_some(),
                    port.process.as_ref().map(|process| process.pid),
                )
            })
            .collect()
    }

    #[test]
    fn rows_parse_as_the_kernel_prints_them() {
        let row = "   0: 0100007F:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0";
        let socket = parse_row(row).unwrap();
        assert_eq!(socket.ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!((socket.port, socket.inode), (5173, 4242));
        assert_eq!(
            parse_ip("00000000000000000000000001000000"),
            Some(IpAddr::V6(Ipv6Addr::LOCALHOST))
        );
        assert_eq!(
            parse_ip("0000000000000000FFFF00000F03000A"),
            Some("::ffff:10.0.3.15".parse().unwrap())
        );
        assert_eq!(parse_ip("nothex00"), None);
        assert!(parse_row("garbage").is_none());
        assert_eq!(socket_inode("socket:[99]"), Some(99));
        assert_eq!(socket_inode("pipe:[99]"), None);
    }

    #[test]
    fn a_stat_name_may_hold_spaces_and_parentheses() {
        let stat = parse_stat(
            "61 (node (vite)) S 60 61 61 0 -1 4194560 100 0 0 0 1 1 0 0 20 0 1 0 210 1000",
        )
        .unwrap();
        assert_eq!(stat.name, "node (vite)");
        assert_eq!((stat.ppid, stat.start), (60, 210));
        assert!(!stat.ended);
        let zombie =
            parse_stat("62 (sh) Z 60 61 61 0 -1 4227084 100 0 0 0 0 0 0 0 20 0 1 0 220 0").unwrap();
        assert!(zombie.ended);
    }

    /// On Linux a process that ends stays in `/proc` until its parent
    /// reaps it. A command whose parent never waits, as with `exec sleep`
    /// after `&`, isn't still running.
    #[test]
    fn an_ended_process_its_parent_never_reaped_isnt_listed() {
        let clock = Clock {
            now_ms: 10_000,
            hz: 100,
            page: 4096,
        };
        let f = fixture();
        f.process(62, 51, 220, "sh", &["sh", "-c", "sleep 3; true"], &[]);
        let at = f.dir.path().join("62");
        fs::write(
            at.join("stat"),
            "62 (sh) Z 51 62 62 0 -1 4227084 100 0 0 0 0 0 0 0 20 0 1 0 220 0",
        )
        .unwrap();
        fs::write(at.join("cmdline"), "").unwrap();
        let pids: Vec<u32> = f
            .scanner(false)
            .processes_at(&holder(), &clock)
            .iter()
            .map(|process| process.pid)
            .collect();
        assert_eq!(pids, [51, 60, 61, 70]);
    }

    #[test]
    fn ports_in_a_holders_tree_show_with_its_session() {
        let f = fixture();
        let ports = f.scanner(false).scan(&[holder()]);
        assert_eq!(
            summary(&ports),
            [
                (3000, BindAddress::Any, true, Some(70)),
                (5173, BindAddress::Loopback, true, Some(61)),
            ]
        );
        let vite = ports[1].process.as_ref().unwrap();
        assert_eq!(vite.name, "node (vite)");
        assert_eq!(vite.command, "node vite --port 5173");
        assert_eq!(ports[1].session, Some(SessionId::derive("s")));
    }

    #[test]
    fn a_credential_in_the_arguments_is_taken_out() {
        let f = fixture();
        fs::write(
            f.dir.path().join("70/cmdline"),
            [
                "python3",
                "serve.py",
                "--password",
                "correct horse",
                "--port",
                "3000",
                "",
            ]
            .join("\0"),
        )
        .unwrap();
        let ports = f.scanner(false).scan(&[holder()]);
        let server = ports[0].process.as_ref().unwrap();
        assert_eq!(
            server.command,
            "python3 serve.py --password [hidden] --port 3000"
        );
    }

    #[test]
    fn the_guest_sees_every_port_but_its_own_and_the_clis() {
        let f = fixture();
        let ports = f.scanner(true).scan(&[holder()]);
        assert_eq!(
            summary(&ports),
            [
                (3000, BindAddress::Any, true, Some(70)),
                (5173, BindAddress::Loopback, true, Some(61)),
                (5432, BindAddress::Any, false, Some(80)),
                (9000, BindAddress::Loopback, false, None),
            ]
        );
    }

    #[test]
    fn without_holders_the_host_sees_nothing() {
        let f = fixture();
        assert!(f.scanner(false).scan(&[]).is_empty());
    }

    #[test]
    fn a_socket_that_moves_to_another_process_is_found_again() {
        let f = fixture();
        let mut scanner = f.scanner(false);
        assert_eq!(scanner.scan(&[holder()]).len(), 2);
        // The server exits, and a new one takes its socket's place.
        fs::remove_dir_all(f.dir.path().join("70")).unwrap();
        f.process(71, 61, 400, "python3", &["python3", "serve.py"], &[1004]);
        let ports = scanner.scan(&[holder()]);
        assert_eq!(ports[0].process.as_ref().unwrap().pid, 71);
    }

    #[test]
    fn no_sockets_reads_no_processes() {
        let f = Fixture::new();
        f.table("tcp", &[]);
        assert!(f.scanner(true).scan(&[holder()]).is_empty());
    }

    #[test]
    fn the_running_list_is_the_holders_tree_with_its_cli_marked() {
        let clock = Clock {
            now_ms: 10_000,
            hz: 100,
            page: 4096,
        };
        let listed = fixture().scanner(false).processes_at(&holder(), &clock);
        let rows: Vec<(u32, u32, bool, Vec<u16>)> = listed
            .iter()
            .map(|process| {
                (
                    process.pid,
                    process.parent,
                    process.cli,
                    process.ports.clone(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (51, 50, true, vec![45_000]),
                (60, 51, false, vec![]),
                (61, 60, false, vec![5173]),
                (70, 50, false, vec![3000]),
            ]
        );
        let vite = &listed[2];
        assert_eq!(
            (vite.name.as_str(), vite.command.as_str()),
            ("node (vite)", "node vite --port 5173")
        );
        assert_eq!(
            (vite.start, vite.run_ms, vite.cpu_ms, vite.memory),
            (210, 7_900, 20, 409_600)
        );
        let stranger = WatchedHolder { pid: 1, ..holder() };
        let everyone = fixture().scanner(false).processes_at(&stranger, &clock);
        assert!(
            everyone.iter().all(|process| process.pid != 99),
            "maxd listed itself"
        );
    }

    /// Scans this device's `/proc` as the guest does and prints each port
    /// with the process found for it, to compare with `ss -ltnp`. Run it with
    /// `cargo test -p maxd -- --ignored real_ports --nocapture`.
    #[test]
    #[ignore = "reads this device's processes and sockets"]
    fn real_ports() {
        for port in Procfs::new("/proc", true).scan(&[]) {
            let process = port
                .process
                .map_or_else(|| "unknown".to_owned(), |p| format!("{} {}", p.pid, p.name));
            println!("port: {} {:?} {process}", port.port, port.bind);
        }
    }
}
