// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The processes an agent started, for the Running list, and the signal a
//! client sends one. A [`Scanner`] lists them with the walk that finds
//! ports. A signal goes only to a process that's in the agent's tree at
//! that moment with the start time the client saw, so a pid the system
//! gave to another process is never signalled. Only that process gets
//! it, never its group.

use std::collections::{HashMap, HashSet};

use maxhq_protocol::guest::{WatchedHolder, MAX_MESSAGE_LEN};
use maxhq_protocol::processes::{AgentProcess, AgentRunning, ProcessSignal};

use super::{clip, Scanner, MAX_COMMAND_LEN, MAX_NAME_LEN};

/// What a scanner knows about one process in a tree.
pub(crate) struct Seen<'a> {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    pub(crate) start: u64,
    pub(crate) run_ms: u64,
    pub(crate) cpu_ms: u64,
    pub(crate) memory: u64,
    pub(crate) name: &'a str,
    pub(crate) command: &'a str,
    pub(crate) cli: bool,
    pub(crate) ports: Vec<u16>,
}

impl Seen<'_> {
    /// The process as it's sent, its name and command cut short and its
    /// ports lowest first.
    pub(crate) fn shown(self) -> AgentProcess {
        let mut ports = self.ports;
        ports.sort_unstable();
        ports.dedup();
        AgentProcess {
            pid: self.pid,
            parent: self.parent,
            start: self.start,
            run_ms: self.run_ms,
            cpu_ms: self.cpu_ms,
            memory: self.memory,
            name: clip(self.name, MAX_NAME_LEN),
            command: clip(self.command, MAX_COMMAND_LEN),
            cli: self.cli,
            ports,
        }
    }
}

/// The most processes one list carries, as many as a tree walk takes.
pub(crate) const MAX_PROCESSES: usize = 4096;

/// The most bytes a list takes as JSON, half of what one guest message
/// may carry, so a list never breaks the link.
const MAX_LIST_BYTES: usize = MAX_MESSAGE_LEN / 2;

/// Puts a list oldest first, as it's sent, and keeps the oldest that fit
/// in [`MAX_LIST_BYTES`].
pub(crate) fn oldest_first(mut processes: Vec<AgentProcess>) -> Vec<AgentProcess> {
    processes.sort_by_key(|process| (process.start, process.pid));
    let mut total = 0usize;
    let fits = processes
        .iter()
        .take_while(|process| {
            let len = serde_json::to_vec(process).map_or(usize::MAX, |json| json.len() + 1);
            total = total.saturating_add(len);
            total <= MAX_LIST_BYTES
        })
        .count();
    processes.truncate(fits);
    processes
}

/// Checks a list a guest sent, since the host trusts nothing from it: at
/// most [`MAX_PROCESSES`], with names and commands cut short.
pub fn checked(processes: Vec<AgentProcess>) -> Result<Vec<AgentProcess>, String> {
    if processes.len() > MAX_PROCESSES {
        return Err("the guest listed more processes than it may send".to_owned());
    }
    let shown = processes
        .into_iter()
        .map(|process| {
            Seen {
                pid: process.pid,
                parent: process.parent,
                start: process.start,
                run_ms: process.run_ms,
                cpu_ms: process.cpu_ms,
                memory: process.memory,
                name: &process.name,
                command: &process.command,
                cli: process.cli,
                ports: process.ports,
            }
            .shown()
        })
        .collect();
    Ok(oldest_first(shown))
}

/// Whether `pid`, started at `start`, may be signalled: it's in the
/// agent's tree now, it's the same process the client saw, and it isn't
/// the agent's CLI, which stopping the agent stops.
pub fn check(processes: &[AgentProcess], pid: u32, start: u64) -> Result<(), String> {
    let Some(process) = processes.iter().find(|process| process.pid == pid) else {
        return Err("that process isn't running in the agent's tree any more".to_owned());
    };
    if process.start != start {
        return Err("that process has exited, and its pid now names another one".to_owned());
    }
    if process.cli {
        return Err("that's the agent itself, so stop the agent instead".to_owned());
    }
    Ok(())
}

/// Lists `holder`'s tree through `scanner`, checks `pid` against it, and
/// sends `signal` to that one process. A holder at init or below has
/// every process under it, so it's refused.
pub fn signal(
    scanner: &mut dyn Scanner,
    holder: &WatchedHolder,
    pid: u32,
    start: u64,
    signal: ProcessSignal,
) -> Result<(), String> {
    if holder.pid <= 1 {
        return Err("that agent has no holder to look under".to_owned());
    }
    check(&scanner.processes(holder), pid, start)?;
    send(pid, signal)
}

/// The most job names an agent's summary carries.
pub(crate) const MAX_JOB_NAMES: usize = 3;

/// The longest job name, in characters.
const MAX_JOB_NAME: usize = 40;

/// The shells a CLI runs each command in.
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "ksh"];

/// The programs that run a script named next on their command line, so a
/// job's name is the script's.
const RUNNERS: [&str; 7] = ["node", "python", "python3", "bun", "deno", "ruby", "env"];

/// The programs that keep the machine awake: `caffeinate` on a Mac and
/// `systemd-inhibit` on Linux. A CLI starts one for a turn, and a common
/// hook starts `caffeinate -di -w <cli pid>` for the CLI's whole life.
/// One with nothing under it does no work. One that runs a program, as
/// `caffeinate -i cargo build` does, is that program's job.
const KEEPERS: [&str; 2] = ["caffeinate", "systemd-inhibit"];

/// What an agent left running, from the processes in its holder's tree,
/// or none when nothing is. A job is a shell its CLI started, since each
/// command runs in one, or a process whose parent left the tree, such as
/// a server a command started with `&`. Each counts once with what runs
/// under it. The CLI's other children, such as its MCP servers, aren't
/// jobs, and neither is a tree that only keeps the machine awake.
pub fn running(processes: &[AgentProcess]) -> Option<AgentRunning> {
    let tree = Tree::new(processes);
    let roots: Vec<&AgentProcess> = processes
        .iter()
        .filter(|process| !process.cli)
        .filter(|process| match tree.cli {
            Some(cli) if process.parent == cli => is_shell(&process.name),
            _ => !tree.pids.contains(&process.parent),
        })
        .filter(|root| !tree.only_keeps(root))
        .collect();
    if roots.is_empty() {
        return None;
    }
    let names = roots
        .iter()
        .take(MAX_JOB_NAMES)
        .map(|root| tree.job_name(root))
        .collect();
    Some(AgentRunning {
        count: u32::try_from(roots.len()).unwrap_or(u32::MAX),
        names,
        waiting: None,
    })
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name.trim_start_matches('-'))
}

fn is_keeper(name: &str) -> bool {
    KEEPERS.contains(&name)
}

/// Whether `command` only waits for the process `pid` to exit, as `tail
/// --pid=<cli pid> -f /dev/null` does under a hook's `systemd-inhibit`.
fn waits_on(command: &str, pid: u32) -> bool {
    let pid = pid.to_string();
    let mut words = command.split_whitespace();
    while let Some(word) = words.next() {
        let waited = match word.strip_prefix("--pid") {
            Some("") => words.next(),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        };
        if waited == Some(pid.as_str()) {
            return true;
        }
    }
    false
}

/// An agent's processes by parent, for finding its jobs.
struct Tree<'a> {
    cli: Option<u32>,
    pids: HashSet<u32>,
    /// Each process's children, oldest first as the list is.
    children: HashMap<u32, Vec<&'a AgentProcess>>,
}

impl<'a> Tree<'a> {
    fn new(processes: &'a [AgentProcess]) -> Self {
        let mut children: HashMap<u32, Vec<&AgentProcess>> = HashMap::new();
        for process in processes {
            children.entry(process.parent).or_default().push(process);
        }
        Self {
            cli: processes
                .iter()
                .find(|process| process.cli)
                .map(|process| process.pid),
            pids: processes.iter().map(|process| process.pid).collect(),
            children,
        }
    }

    fn under(&self, pid: u32) -> &[&'a AgentProcess] {
        self.children.get(&pid).map_or(&[], Vec::as_slice)
    }

    /// Whether `process` does no work itself: it keeps the machine awake,
    /// waits for the CLI to exit, or is a shell waiting on what it runs.
    fn idles(&self, process: &AgentProcess) -> bool {
        is_keeper(&process.name)
            || self.cli.is_some_and(|cli| waits_on(&process.command, cli))
            || (is_shell(&process.name) && !self.under(process.pid).is_empty())
    }

    /// Whether nothing in `root`'s tree does work, as in a hook's
    /// `sh -c '... || caffeinate -di -w $PPID'`.
    fn only_keeps(&self, root: &AgentProcess) -> bool {
        let mut seen = HashSet::new();
        let mut left = vec![root];
        while let Some(process) = left.pop() {
            if !seen.insert(process.pid) {
                continue;
            }
            if !self.idles(process) {
                return false;
            }
            left.extend(self.under(process.pid));
        }
        true
    }

    /// The oldest process under `pid` whose tree does work.
    fn first_working(&self, pid: u32) -> Option<&'a AgentProcess> {
        self.under(pid)
            .iter()
            .copied()
            .find(|child| !self.only_keeps(child))
    }

    /// A job's name: what its shell runs, or the root's own command. A
    /// keeper that runs a program goes by that program.
    fn job_name(&self, root: &'a AgentProcess) -> String {
        let mut named = root;
        if is_shell(&root.name) {
            // The list is oldest first, so this is the shell's first
            // command that does work.
            match self.first_working(root.pid) {
                Some(child) => named = child,
                None => return short_command(script_of(&root.command), &root.name),
            }
        }
        while is_keeper(&named.name)
            && let Some(child) = self.first_working(named.pid)
        {
            named = child;
        }
        short_command(&named.command, &named.name)
    }
}

/// What a shell's command line runs: the script in its `eval '...'`, as
/// Claude Code writes them, or what follows `-c`.
fn script_of(command: &str) -> &str {
    if let Some((_, rest)) = command.split_once("eval '") {
        return rest.split('\'').next().unwrap_or(rest);
    }
    command
        .split_once(" -c ")
        .map_or(command, |(_, script)| script.trim_start_matches("-l "))
}

/// A command line in a few words: the program's own name and its first
/// arguments, with a runner such as `node` left out.
pub(crate) fn short_command(command: &str, name: &str) -> String {
    let mut words = command.split_whitespace().peekable();
    let base = |word: &str| word.rsplit('/').next().unwrap_or(word).to_owned();
    let Some(first) = words.next() else {
        return name.to_owned();
    };
    let mut program = base(first);
    if RUNNERS.contains(&program.as_str())
        && let Some(next) = words.peek()
        && !next.starts_with('-')
        && !next.contains('=')
    {
        program = base(next);
        words.next();
    }
    let shown: Vec<String> = std::iter::once(program)
        .chain(words.take(3).map(str::to_owned))
        .collect();
    let text = shown.join(" ");
    if text.chars().count() <= MAX_JOB_NAME {
        text
    } else {
        let cut: String = text.chars().take(MAX_JOB_NAME - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Sends `signal` to `pid` alone. Windows can't ask one process to end,
/// so there a stop is refused with why, and a kill ends it at once.
pub(crate) fn send(pid: u32, signal: ProcessSignal) -> Result<(), String> {
    use maxhq_os::proc::{self, Stop};

    // Zero or a negative pid would reach a whole group, and 1 is init.
    if pid <= 1 || i32::try_from(pid).is_err() {
        return Err(format!("pid {pid} can't be signalled"));
    }
    let how = match signal {
        ProcessSignal::Term => Stop::Soft,
        ProcessSignal::Kill => Stop::Hard,
    };
    match proc::stop(pid, how) {
        Ok(()) => Ok(()),
        Err(_) if !proc::alive(pid) => Err("that process has already exited".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
            Err("Windows can't ask one process to stop, so kill it to end it now".to_owned())
        }
        Err(error) => Err(format!("the signal wasn't sent: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(pid: u32, start: u64, cli: bool) -> AgentProcess {
        Seen {
            pid,
            parent: 1,
            start,
            run_ms: 0,
            cpu_ms: 0,
            memory: 0,
            name: "sleep",
            command: "sleep 30",
            cli,
            ports: vec![5173, 3000, 5173],
        }
        .shown()
    }

    #[test]
    fn only_a_listed_process_with_its_start_and_not_the_cli_is_signalled() {
        let processes = [listed(10, 100, true), listed(11, 200, false)];
        assert_eq!(check(&processes, 11, 200), Ok(()));
        assert!(check(&processes, 12, 200)
            .unwrap_err()
            .contains("isn't running"));
        assert!(check(&processes, 11, 201)
            .unwrap_err()
            .contains("another one"));
        assert!(check(&processes, 10, 100)
            .unwrap_err()
            .contains("stop the agent"));
        assert!(check(&[], 11, 200).is_err());
    }

    fn proc(pid: u32, parent: u32, name: &str, command: &str) -> AgentProcess {
        AgentProcess {
            pid,
            parent,
            start: u64::from(pid),
            run_ms: 0,
            cpu_ms: 0,
            memory: 0,
            name: name.into(),
            command: command.into(),
            cli: false,
            ports: Vec::new(),
        }
    }

    fn cli() -> AgentProcess {
        AgentProcess {
            cli: true,
            ..proc(10, 1, "claude", "claude -p")
        }
    }

    #[test]
    fn only_the_clis_shells_and_orphans_count_as_jobs() {
        let mcp = proc(11, 10, "node", "node /opt/mcp/server.js");
        let mcp_child = proc(12, 11, "sh", "sh -c helper");
        let shell = proc(
            20,
            10,
            "zsh",
            "/bin/zsh -c -l source /x/snapshot.sh && eval 'pnpm dev' < /dev/null",
        );
        let pnpm = proc(21, 20, "node", "node /usr/local/bin/pnpm dev");
        let vite = proc(22, 21, "node", "node vite");
        let orphan = proc(30, 1, "cargo", "/home/me/.cargo/bin/cargo watch -x test");
        let list = [cli(), mcp, mcp_child, shell, pnpm, vite, orphan];
        assert_eq!(
            running(&list),
            Some(AgentRunning {
                count: 2,
                names: vec!["pnpm dev".into(), "cargo watch -x test".into()],
                waiting: None,
            })
        );
    }

    #[test]
    fn nothing_but_the_cli_and_its_servers_is_nothing_running() {
        assert_eq!(running(&[]), None);
        assert_eq!(running(&[cli()]), None);
        assert_eq!(running(&[cli(), proc(11, 10, "node", "node mcp.js")]), None);
    }

    #[test]
    fn a_shell_with_nothing_under_it_is_named_by_its_script() {
        let bare = proc(20, 10, "bash", "bash -c -l eval 'while true; do :; done'");
        let plain = proc(21, 10, "sh", "/bin/sh -c tail -f log.txt");
        let names = running(&[cli(), bare, plain]).unwrap().names;
        assert_eq!(names, ["while true; do :;", "tail -f log.txt"]);
    }

    /// The tree a common SessionStart hook leaves under a Claude CLI for
    /// its whole life.
    fn hook() -> [AgentProcess; 2] {
        [
            proc(
                11,
                10,
                "sh",
                "/bin/sh -c pgrep -f \"caffeinate -di -w $PPID\" >/dev/null 2>&1 || caffeinate -di -w $PPID",
            ),
            proc(12, 11, "caffeinate", "caffeinate -di -w 10"),
        ]
    }

    #[test]
    fn what_only_keeps_the_machine_awake_is_nothing_running() {
        let [shell, caffeinate] = hook();
        assert_eq!(running(&[cli(), shell, caffeinate.clone()]), None);
        // The CLI's own keeper for a turn, and the hook's once its shell left.
        let turn = proc(13, 10, "caffeinate", "caffeinate -i -t 300");
        let orphan = AgentProcess {
            parent: 1,
            ..caffeinate
        };
        assert_eq!(running(&[cli(), turn, orphan]), None);
        // On Linux: a hook's inhibitor that waits for the CLI to exit.
        let inhibit = proc(
            20,
            10,
            "sh",
            "sh -c systemd-inhibit tail --pid=$PPID -f /dev/null",
        );
        let inhibitor = proc(
            21,
            20,
            "systemd-inhibit",
            "systemd-inhibit tail --pid=10 -f /dev/null",
        );
        let tail = proc(22, 21, "tail", "tail --pid=10 -f /dev/null");
        assert_eq!(running(&[cli(), inhibit, inhibitor, tail]), None);
    }

    #[test]
    fn a_keeper_that_runs_a_program_is_that_programs_job() {
        let shell = proc(
            20,
            10,
            "zsh",
            "/bin/zsh -c -l source /x/snapshot.sh && eval 'caffeinate -i cargo build' < /dev/null",
        );
        let caffeinate = proc(21, 20, "caffeinate", "caffeinate -i cargo build");
        let cargo = proc(22, 21, "cargo", "/home/me/.cargo/bin/cargo build");
        let rustc = proc(23, 22, "rustc", "rustc --crate-name maxd");
        let left = proc(30, 1, "caffeinate", "caffeinate -s make test");
        let make = proc(31, 30, "make", "make test");
        assert_eq!(
            running(&[cli(), shell, caffeinate, cargo, rustc, left, make]),
            Some(AgentRunning {
                count: 2,
                names: vec!["cargo build".into(), "make test".into()],
                waiting: None,
            })
        );
    }

    #[test]
    fn a_real_job_next_to_a_keeper_counts_once() {
        let [shell, caffeinate] = hook();
        let dev = proc(20, 10, "zsh", "/bin/zsh -c -l eval 'pnpm dev' < /dev/null");
        let pnpm = proc(21, 20, "node", "node /usr/local/bin/pnpm dev");
        let both = proc(30, 10, "sh", "sh -c caffeinate -w $PPID & cargo test");
        let keeper = proc(31, 30, "caffeinate", "caffeinate -w 10");
        let test = proc(32, 30, "cargo", "cargo test");
        assert_eq!(
            running(&[cli(), shell, caffeinate, dev, pnpm, both, keeper, test]),
            Some(AgentRunning {
                count: 2,
                names: vec!["pnpm dev".into(), "cargo test".into()],
                waiting: None,
            })
        );
    }

    #[test]
    fn only_a_wait_on_the_clis_own_pid_is_idle() {
        assert!(waits_on("tail --pid=10 -f /dev/null", 10));
        assert!(waits_on("tail -f /dev/null --pid 10", 10));
        assert!(!waits_on("tail --pid=100 -f /dev/null", 10));
        assert!(!waits_on("server --pidfile=10", 10));
        assert!(!waits_on("tail -f log.txt 10", 10));
        let other = proc(20, 10, "sh", "sh -c tail --pid=99 -f log.txt");
        let tail = proc(21, 20, "tail", "tail --pid=99 -f log.txt");
        assert_eq!(running(&[cli(), other, tail]).unwrap().count, 1);
    }

    #[test]
    fn only_the_oldest_few_are_named_but_all_count() {
        let mut list = vec![cli()];
        list.extend((0..5).map(|n| proc(20 + n, 10, "sh", &format!("sh -c 'sleep {n}'"))));
        let running = running(&list).unwrap();
        assert_eq!(running.count, 5);
        assert_eq!(running.names.len(), MAX_JOB_NAMES);
    }

    #[test]
    fn a_long_command_is_cut_short() {
        assert_eq!(short_command("", "node"), "node");
        assert_eq!(
            short_command("python3 -m http.server", "python3"),
            "python3 -m http.server"
        );
        let long = format!("cargo {}", "x".repeat(60));
        let short = short_command(&long, "cargo");
        assert_eq!(short.chars().count(), MAX_JOB_NAME);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn ports_go_lowest_first_once_each() {
        assert_eq!(listed(1, 1, false).ports, [3000, 5173]);
    }

    #[test]
    fn a_group_or_init_is_never_signalled() {
        for pid in [0, 1, u32::MAX] {
            assert!(send(pid, ProcessSignal::Term).is_err(), "{pid}");
        }
    }

    #[test]
    fn a_guests_list_is_cut_short_and_sorted() {
        let mut long = listed(7, 300, false);
        long.command = "x".repeat(MAX_COMMAND_LEN * 2);
        let checked = checked(vec![long, listed(8, 100, true)]).unwrap();
        assert_eq!(
            checked
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<_>>(),
            [8, 7]
        );
        assert_eq!(checked[1].command.len(), MAX_COMMAND_LEN);
        let flood = vec![listed(9, 1, false); MAX_PROCESSES + 1];
        assert!(super::checked(flood).is_err());
    }

    #[test]
    fn a_list_keeps_the_oldest_that_fit_in_one_message() {
        // Control characters take six bytes each as JSON.
        let wide = |pid: u32| {
            let mut process = listed(pid, u64::from(MAX_PROCESSES as u32 - pid), false);
            process.command = "\u{1}".repeat(MAX_COMMAND_LEN);
            process
        };
        let all: Vec<AgentProcess> = (1..=MAX_PROCESSES as u32).map(wide).collect();
        let kept = oldest_first(all);
        assert!(kept.len() < MAX_PROCESSES, "{}", kept.len());
        assert!(serde_json::to_vec(&kept).unwrap().len() <= MAX_LIST_BYTES);
        assert_eq!(kept[0].pid, MAX_PROCESSES as u32, "the oldest goes first");
    }

    /// Runs one live test over a real tree: a shell stands in for a holder,
    /// its first child for the CLI, its second stops on SIGTERM, and its
    /// third ignores it. Only these get a signal, and the guard ends their
    /// group however the test ends.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_checked_process_gets_sigterm_and_then_sigkill() {
        use std::os::unix::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        use std::thread;
        use std::time::{Duration, Instant};

        use maxhq_protocol::ids::SessionId;

        /// Ends the group this test made. The holder isn't reaped yet, so
        /// its pid, the group's id, can't name another group.
        struct Group(Child);

        impl Drop for Group {
            #[allow(unsafe_code)]
            fn drop(&mut self) {
                if let Ok(pgid) = libc::pid_t::try_from(self.0.id()) {
                    // SAFETY: kill only reports an error, and the group is
                    // this test's own.
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                }
                let _ = self.0.wait();
            }
        }

        fn until(
            scanner: &mut dyn Scanner,
            holder: &WatchedHolder,
            what: &str,
            check: impl Fn(&[AgentProcess]) -> bool,
        ) -> Vec<AgentProcess> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let listed = scanner.processes(holder);
                if check(&listed) {
                    return listed;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}: {listed:?}"
                );
                thread::sleep(Duration::from_millis(20));
            }
        }

        let has = |listed: &[AgentProcess], command: &str| {
            listed.iter().any(|process| process.command == command)
        };
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 31 & sleep 32 & sh -c 'trap \"\" TERM; exec sleep 33' & wait")
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let holder = WatchedHolder {
            session: SessionId::derive("live"),
            agent: None,
            pid: child.id(),
        };
        let _group = Group(child);
        let shared = crate::ports::local_scanner();
        let mut guard = shared.lock().unwrap();
        let scanner: &mut dyn Scanner = &mut **guard;

        // The third starts as a shell and execs, so wait for its new
        // command too.
        let listed = until(scanner, &holder, "three sleeps", |listed| {
            listed.len() == 3
                && listed.iter().all(|process| process.name == "sleep")
                && ["sleep 31", "sleep 32", "sleep 33"]
                    .iter()
                    .all(|command| has(listed, command))
        });
        let by = |command: &str| {
            listed
                .iter()
                .find(|process| process.command == command)
                .cloned()
                .unwrap_or_else(|| panic!("no {command} in {listed:?}"))
        };
        let (cli, plain, stubborn) = (by("sleep 31"), by("sleep 32"), by("sleep 33"));
        assert!(cli.cli && !plain.cli && !stubborn.cli, "{listed:?}");

        // Nothing goes out unless the pid, its start and its tree check out.
        let term = ProcessSignal::Term;
        let wrong_start = signal(scanner, &holder, plain.pid, plain.start + 1, term);
        assert!(wrong_start.unwrap_err().contains("another one"));
        let the_cli = signal(scanner, &holder, cli.pid, cli.start, ProcessSignal::Kill);
        assert!(the_cli.unwrap_err().contains("stop the agent"));
        let the_holder = signal(scanner, &holder, holder.pid, 0, ProcessSignal::Kill);
        assert!(the_holder.unwrap_err().contains("isn't running"));
        let init = WatchedHolder {
            pid: 1,
            ..holder.clone()
        };
        assert!(signal(scanner, &init, plain.pid, plain.start, term).is_err());
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            scanner.processes(&holder).len(),
            3,
            "a refused signal went out"
        );

        signal(scanner, &holder, plain.pid, plain.start, term).unwrap();
        until(scanner, &holder, "SIGTERM to end a sleep", |listed| {
            !has(listed, "sleep 32")
        });
        signal(scanner, &holder, stubborn.pid, stubborn.start, term).unwrap();
        thread::sleep(Duration::from_millis(300));
        assert!(
            has(&scanner.processes(&holder), "sleep 33"),
            "SIGTERM ended a sleep that ignores it"
        );
        signal(
            scanner,
            &holder,
            stubborn.pid,
            stubborn.start,
            ProcessSignal::Kill,
        )
        .unwrap();
        until(
            scanner,
            &holder,
            "SIGKILL to end the stubborn sleep",
            |listed| !has(listed, "sleep 33"),
        );
        assert!(
            has(&scanner.processes(&holder), "sleep 31"),
            "the CLI was signalled"
        );
    }
}
