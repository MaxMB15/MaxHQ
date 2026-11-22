// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A terminal's holder, `maxd hold --pty <terminal folder>`.
//!
//! It detaches as an agent's holder does, takes the folder's lock, reads
//! its key from stdin, binds its socket and starts the program on a new
//! pty, in a session of its own with the pty as its terminal. Then it says
//! `ready`.
//!
//! One task reads the master into the ring and a headless terminal, for a
//! `!` run once its marker has come (`marker.rs`). On the way it takes out
//! the shell's marks (`marks.rs`), and keeps where each was for clients
//! that ask for them. The headless terminal answers what the program asks,
//! such as where the cursor is, and the holder writes those answers back,
//! so a program gets one answer whether or not anyone looks.
//! One task writes input and answers, in the order they came.
//! Every 150 ms the holder looks at the pty's modes, its foreground group
//! and the line the cursor is on (`modes.rs`), and tells clients when the
//! program starts or stops waiting for a password or for you, or the
//! shell starts or stops running something.
//!
//! Once the program ends and its output is in, the holder writes
//! `exit.json`. A goodbye from a client that saw the exit deletes the
//! folder and ends the holder. Without one, the holder ends after its
//! linger and leaves the folder for `maxd` to find.

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::time::Timestamp;
use maxhq_term::Emulator;
use rustix::process::{Pid, Signal, WaitOptions};
use tokio::io::BufReader;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, watch};

use super::marker::Cutoff;
use super::marks::{Filter, Kind, Mark};
use super::modes::{self, Pty, QUIET};
use super::osc::{Scanner, Set};
use super::ring::{Ring, RING_BYTES};
use super::wire::{self, FromHolder, ToHolder, Typed, CHUNK};
use super::{socket_name, LaunchFile, RunOutput, TermExit, TermHello, TermModes};
use crate::files::{self, HolderFile, EXIT, HOLDER, LAUNCH, LOCK};
use crate::holder::{
    quiet_stdin, read_stdin, say_ready, take_lock, DRAIN_WAIT, FLUSH_WAIT, HELLO_WAIT, QUIT_GRACE,
    REAP_TICK,
};
use crate::pty::{self, Master};
use crate::secret::{self, Secret};
use crate::socket::{self, Socket};

/// How often the holder looks for a password prompt and a busy shell.
const POLL: Duration = Duration::from_millis(150);

/// How much output the holder reads at once.
const READ_SIZE: usize = 64 * 1024;

/// The most marks the holder keeps, newest last. It also drops those whose
/// output has left the ring.
const MAX_MARKS: usize = 4096;

/// What the writer task writes.
enum Input {
    /// What someone typed.
    Typed(Typed),
    /// The headless terminal's answers to what the program asked.
    Answers(Vec<u8>),
}

/// What the second step sets up before saying it's ready.
pub(super) struct Started {
    holder: Arc<Holder>,
    socket: Socket,
    input: mpsc::UnboundedReceiver<Input>,
    _lock: File,
}

impl Started {
    pub(super) fn start(dir: &Path) -> Result<Self, String> {
        let at = |path: &Path| {
            let path = path.to_owned();
            move |err: io::Error| format!("couldn't set up {}: {err}", path.display())
        };
        let lock = take_lock(&dir.join(LOCK))?;
        let launch: LaunchFile = files::read_json(&dir.join(LAUNCH))
            .map_err(at(&dir.join(LAUNCH)))?
            .ok_or_else(|| format!("{} has no {LAUNCH}", dir.display()))?;
        if dir.join(HOLDER).exists() || dir.join(EXIT).exists() {
            return Err(format!("terminal {} has already run", launch.terminal));
        }
        let (key, _) = read_stdin(true, &[])?;
        let key = key.ok_or("the holder's key didn't come on stdin")?;
        quiet_stdin();
        let sockets = PathBuf::from(launch.sockets.0.clone());
        socket::make_dir(&sockets).map_err(at(&sockets))?;
        let socket =
            socket::bind_named(&sockets, &socket_name(launch.terminal)).map_err(at(&sockets))?;
        socket
            .listener
            .set_nonblocking(true)
            .map_err(at(&socket.path))?;

        let size = pty::winsize(launch.cols.into(), launch.rows.into(), 0, 0);
        let (master, slave) =
            pty::open(size).map_err(|err| format!("couldn't open a pty: {err}"))?;
        let program = PathBuf::from(launch.program.0.clone());
        let cwd = PathBuf::from(launch.cwd.0.clone());
        let mut command = Command::new(&program);
        command
            .args(launch.args.iter().map(|arg| &arg.0))
            .current_dir(&cwd);
        for (key, value) in &launch.env {
            command.env(&key.0, &value.0);
        }
        let stdio = |slave: &std::os::fd::OwnedFd| {
            slave
                .try_clone()
                .map(Stdio::from)
                .map_err(|err| format!("couldn't hand over the pty: {err}"))
        };
        command
            .stdin(stdio(&slave)?)
            .stdout(stdio(&slave)?)
            .stderr(Stdio::from(slave));
        pty::controlling_tty(&mut command);
        let spawned = command.spawn();
        // The program has the terminal's end now. The holder keeps none, so
        // reading the master ends once the program and what it started let
        // go of theirs.
        drop(command);
        let child = spawned.map_err(|err| spawn_failure(&program, &cwd, &err))?;
        let child_pid = i32::try_from(child.id())
            .ok()
            .and_then(Pid::from_raw)
            .ok_or("the program has no process ID")?;
        // The holder reaps the program itself, so it can signal its group
        // only while the ID still belongs to it.
        drop(child);
        let master = Master::new(master).map_err(|err| format!("couldn't read the pty: {err}"))?;

        let started_at = Timestamp::now();
        let (input_tx, input) = mpsc::unbounded_channel();
        let holder = Arc::new(Holder {
            dir: dir.to_owned(),
            terminal: launch.terminal,
            key,
            pid: std::process::id(),
            child_pid,
            started_at,
            run: launch.run,
            linger: Duration::from_millis(launch.linger_ms),
            master,
            input: input_tx,
            state: Mutex::new(State {
                ring: Ring::new(RING_BYTES),
                cutoff: launch.run.then(Cutoff::new),
                // A shell can run for days, so its terminal keeps only the
                // screen.
                emulator: if launch.run {
                    maxhq_term::emulator(launch.cols, launch.rows)
                } else {
                    maxhq_term::screen(launch.cols, launch.rows)
                },
                last_output: Instant::now(),
                modes: TermModes::default(),
                scanner: Scanner::new(),
                filter: Filter::new(launch.nonce.as_deref()),
                marks: VecDeque::new(),
                marks_gone: 0,
                title: None,
                cwd: None,
                at_prompt: true,
                password: false,
                noted: false,
                busy: launch.run,
                cols: launch.cols,
                rows: launch.rows,
                phase: Phase::Running,
                output_done: false,
                ending: false,
            }),
            changed: watch::Sender::new(0),
        });
        let record = HolderFile {
            holder_pid: holder.pid,
            child_pid: child_pid.as_raw_nonzero().get().unsigned_abs(),
            socket: socket.path.as_path().into(),
            started_at,
        };
        if let Err(err) = files::write_json(&dir.join(HOLDER), &record) {
            holder.signal_child(Signal::KILL);
            return Err(format!("couldn't write {HOLDER}: {err}"));
        }
        say_ready(dir);
        Ok(Self {
            holder,
            socket,
            input,
            _lock: lock,
        })
    }

    pub(super) async fn run(self) {
        let holder = self.holder;
        match self
            .socket
            .listener
            .try_clone()
            .and_then(tokio::net::UnixListener::from_std)
        {
            Ok(listener) => {
                let accepting = holder.clone();
                tokio::spawn(async move {
                    loop {
                        match listener.accept().await {
                            Ok((stream, _)) => {
                                tokio::spawn(serve(accepting.clone(), stream));
                            }
                            Err(err) => {
                                eprintln!("couldn't accept a connection: {err}");
                                tokio::time::sleep(REAP_TICK).await;
                            }
                        }
                    }
                });
            }
            Err(err) => {
                eprintln!("couldn't listen on {}: {err}", self.socket.path.display());
                holder.quit();
            }
        }
        tokio::spawn(holder.clone().read_output());
        tokio::spawn(holder.clone().write_input(self.input));
        tokio::spawn(holder.clone().reap_loop());
        tokio::spawn(holder.clone().poll());
        tokio::spawn(holder.clone().watch_signals());

        let mut changes = holder.changed.subscribe();
        while !holder.exited() {
            changes.borrow_and_update();
            if holder.exited() || changes.changed().await.is_err() {
                break;
            }
        }
        let linger = tokio::time::sleep(holder.linger);
        tokio::pin!(linger);
        loop {
            changes.borrow_and_update();
            if holder.lock().ending {
                break;
            }
            tokio::select! {
                () = &mut linger => break,
                changed = changes.changed() => if changed.is_err() { break },
            }
        }
        tokio::time::sleep(FLUSH_WAIT).await;
        drop(self.socket);
    }
}

/// Why the program didn't start, naming the folder when that's what's
/// gone.
fn spawn_failure(program: &Path, cwd: &Path, err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::NotFound && matches!(cwd.try_exists(), Ok(false)) {
        format!("the folder {} is gone", cwd.display())
    } else {
        format!("couldn't start {}: {err}", program.display())
    }
}

/// Where the program is in its life.
enum Phase {
    Running,
    /// Reaped, with the holder still reading what it printed last.
    Reaped {
        code: Option<i32>,
        signal: Option<i32>,
        at: Timestamp,
    },
    Exited(TermExit),
}

struct State {
    ring: Ring,
    /// A `!` run's output while it waits for the marker.
    cutoff: Option<Cutoff>,
    /// The headless terminal that answers what the program asks, and that
    /// a `!` run's transcript comes from.
    emulator: Box<dyn Emulator>,
    /// When the program last printed something.
    last_output: Instant,
    /// How the program reads input, as the last look saw it.
    modes: TermModes,
    scanner: Scanner,
    filter: Filter,
    marks: VecDeque<Placed>,
    /// How many marks have left the front of `marks`, so a connection
    /// knows which it has sent.
    marks_gone: u64,
    title: Option<String>,
    cwd: Option<String>,
    /// Whether output now is the shell's own, where it says its folder:
    /// before its first mark, or after a command's end and before its
    /// prompt. Between the prompt and the end, the output is what you
    /// typed and what the command printed, which could say any folder.
    /// A shell that never marks stays here.
    at_prompt: bool,
    password: bool,
    /// The transcript already says a password went at this prompt.
    noted: bool,
    busy: bool,
    cols: u16,
    rows: u16,
    phase: Phase,
    output_done: bool,
    /// The holder should go once the program has ended: a client said
    /// goodbye or the holder was told to quit.
    ending: bool,
}

impl State {
    /// Keeps output clients see, and the title, folder and marks it sets.
    fn keep(&mut self, bytes: &[u8]) {
        let mut shown = Vec::with_capacity(bytes.len());
        let mut found = Vec::new();
        self.filter.feed(bytes, &mut shown, &mut found);
        if shown.is_empty() {
            return;
        }
        let start = self.ring.end();
        self.ring.push(&shown);
        self.emulator.feed(&shown);
        // A command starts in the folder the shell said before it.
        let mut scanned = 0;
        for (at, kind) in found {
            self.scan(&shown[scanned..at]);
            scanned = at;
            self.at_prompt = matches!(kind, Kind::Done(_));
            self.marks.push_back(Placed {
                offset: start + at as u64,
                at: Timestamp::now(),
                mark: kind.mark(self.cwd.as_deref()),
            });
        }
        self.scan(&shown[scanned..]);
        let kept = self.ring.start();
        while self.marks.len() > MAX_MARKS
            || self
                .marks
                .front()
                .is_some_and(|placed| placed.offset < kept)
        {
            self.marks.pop_front();
            self.marks_gone += 1;
        }
        // A prompt that hides input prints nothing while it's answered, so
        // any output means the next password is a new one.
        self.noted = false;
    }

    /// Notes the title and folder `bytes` set. The folder counts only from
    /// the shell at its prompt.
    fn scan(&mut self, bytes: &[u8]) {
        let Self {
            scanner,
            title,
            cwd,
            at_prompt,
            ..
        } = self;
        scanner.feed(bytes, |set| match set {
            Set::Title(set) => *title = Some(set),
            Set::Cwd(set) if *at_prompt => *cwd = Some(set),
            Set::Cwd(_) => {}
        });
    }

    /// Stops waiting for a run's marker and keeps what waited for it.
    /// Returns whether anything did. What the startup asked before the
    /// marker is long past, so it gets no answer.
    fn release(&mut self) -> bool {
        let held = self.cutoff.take().map(Cutoff::release).unwrap_or_default();
        self.keep(&held);
        drop(self.emulator.answers());
        !held.is_empty()
    }
}

/// A mark and where it is in the output.
#[derive(Clone)]
struct Placed {
    offset: u64,
    at: Timestamp,
    mark: Mark,
}

/// What a connection has told its client, so it sends only changes.
#[derive(Clone, PartialEq, Eq)]
struct View {
    password: bool,
    busy: bool,
    title: Option<String>,
    cwd: Option<String>,
    modes: TermModes,
}

struct Holder {
    dir: PathBuf,
    terminal: TerminalId,
    key: Secret,
    pid: u32,
    child_pid: Pid,
    started_at: Timestamp,
    run: bool,
    linger: Duration,
    master: Master,
    /// Input and answers for the writer task, which is the only one that
    /// writes.
    input: mpsc::UnboundedSender<Input>,
    state: Mutex<State>,
    /// Counts changes, so tasks can wait for the next one.
    changed: watch::Sender<u64>,
}

impl Holder {
    /// Why a client whose hello names `terminal` and shows `key` is turned
    /// away, or `None` when it's welcome. The key comes first, so a client
    /// without it learns nothing else.
    fn refusal(&self, terminal: TerminalId, key: &Secret) -> Option<String> {
        if !secret::key_matches(&self.key, key) {
            return Some("the key in the hello isn't this holder's".into());
        }
        (terminal != self.terminal).then(|| format!("this holder runs terminal {}", self.terminal))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    fn exited(&self) -> bool {
        matches!(self.lock().phase, Phase::Exited(_))
    }

    /// Reads the master until every terminal end has closed.
    async fn read_output(self: Arc<Self>) {
        let mut buf = vec![0; READ_SIZE];
        loop {
            match self.master.read(&mut buf).await {
                Ok(0) => break,
                Ok(read) => self.record(&buf[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    eprintln!("stopped reading the terminal: {err}");
                    break;
                }
            }
        }
        self.lock().output_done = true;
        self.bump();
    }

    /// Keeps output, once a run's marker has come, and writes back the
    /// answers to what it asked.
    fn record(&self, bytes: &[u8]) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Exited(_)) {
            return;
        }
        state.last_output = Instant::now();
        match state.cutoff.as_mut().map(|cutoff| cutoff.feed(bytes)) {
            None => state.keep(bytes),
            Some(None) => return,
            Some(Some(shown)) => {
                state.cutoff = None;
                state.keep(&shown);
            }
        }
        let answers = state.emulator.answers();
        drop(state);
        if !answers.is_empty() {
            let _ = self.input.send(Input::Answers(answers));
        }
        self.bump();
    }

    /// The writer task. It's the only one that writes to the master, so
    /// input and answers never interleave.
    async fn write_input(self: Arc<Self>, mut input: mpsc::UnboundedReceiver<Input>) {
        while let Some(next) = input.recv().await {
            let bytes = match &next {
                Input::Typed(typed) => {
                    // A secret goes when you press Enter. A prompt in raw
                    // mode can echo a mask for each key before it, which
                    // is output of its own.
                    let ends_line = typed.0.iter().any(|&byte| matches!(byte, b'\r' | b'\n'));
                    if ends_line && (self.master.password_mode() || self.lock().password) {
                        self.note_secret();
                    }
                    &typed.0
                }
                Input::Answers(answers) => answers,
            };
            // A program that has gone takes nothing more, and that's no
            // error worth reporting.
            let _ = self.master.write_all(bytes).await;
        }
    }

    /// Marks a run's transcript where a password went, once per prompt. A
    /// shell's terminal keeps no transcript.
    fn note_secret(&self) {
        if !self.run {
            return;
        }
        let mut state = self.lock();
        if state.noted {
            return;
        }
        // The prompt goes in first, even when it came before the marker.
        state.release();
        state.noted = true;
        state.emulator.note(maxhq_term::SECRET_SENT);
    }

    /// Queues `typed` for the program.
    fn type_in(&self, typed: Typed) {
        let _ = self.input.send(Input::Typed(typed));
    }

    fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let mut state = self.lock();
        if (state.cols, state.rows) == (cols, rows) {
            return;
        }
        state.cols = cols;
        state.rows = rows;
        state.emulator.resize(cols, rows);
        self.master
            .resize(pty::winsize(cols.into(), rows.into(), 0, 0));
    }

    /// Looks at the pty's modes, its foreground group and the screen, and
    /// bumps when the password prompt, the busy shell or the modes
    /// changed. A run that waits for input before its marker shows what it
    /// held, since a startup file asks for something.
    async fn poll(self: Arc<Self>) {
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            if self.exited() {
                return;
            }
            let hidden = self.master.password_mode();
            let keys = self.master.key_mode();
            let foreground = self.master.foreground();
            let mut state = self.lock();
            let running = matches!(state.phase, Phase::Running);
            let busy = if self.run {
                running
            } else {
                running && foreground.is_some_and(|group| group != self.child_pid)
            };
            let released = state.cutoff.is_some() && (hidden || keys) && state.release();
            let pty = Pty {
                busy,
                hidden,
                keys,
                quiet: state.last_output.elapsed() >= QUIET,
            };
            let read = modes::read(pty, &state.emulator.look());
            let changed = released
                || (state.password, state.busy) != (read.password, busy)
                || state.modes != read.modes;
            state.password = read.password;
            state.busy = busy;
            state.modes = read.modes;
            if !state.password {
                state.noted = false;
            }
            drop(state);
            if changed {
                self.bump();
            }
        }
    }

    /// Reaps every child that has exited. Returns whether the program has.
    /// This holds the state lock, as [`Holder::signal_child`] does, so the
    /// program's ID is never signalled once it could be reused.
    fn reap(&self) -> bool {
        let mut state = self.lock();
        loop {
            match rustix::process::wait(WaitOptions::NOHANG) {
                Ok(Some((pid, status))) => {
                    if pid == self.child_pid && matches!(state.phase, Phase::Running) {
                        state.phase = Phase::Reaped {
                            code: status.exit_status(),
                            signal: status.terminating_signal(),
                            at: Timestamp::now(),
                        };
                    }
                }
                Ok(None) => break,
                Err(rustix::io::Errno::INTR) => {}
                // No children left.
                Err(_) => break,
            }
        }
        !matches!(state.phase, Phase::Running)
    }

    async fn reap_loop(self: Arc<Self>) {
        let mut sigchld = signal(SignalKind::child()).ok();
        let mut tick = tokio::time::interval(REAP_TICK);
        let mut drain_until = None;
        loop {
            match sigchld.as_mut() {
                Some(sigchld) => {
                    tokio::select! {
                        _ = sigchld.recv() => {}
                        _ = tick.tick() => {}
                    }
                }
                None => {
                    tick.tick().await;
                }
            }
            if !self.reap() || self.exited() {
                continue;
            }
            // Something the program started in the background can keep the
            // terminal open, and what it prints after the program has gone
            // isn't kept.
            let drained = self.lock().output_done;
            let until = *drain_until.get_or_insert_with(|| Instant::now() + DRAIN_WAIT);
            if drained || Instant::now() >= until {
                self.finish();
                if !cfg!(target_os = "linux") {
                    // Only a subreaper gets orphans to reap later.
                    return;
                }
            }
        }
    }

    /// Records the exit once the program's output is in, with the run's
    /// transcript.
    fn finish(&self) {
        let mut state = self.lock();
        let Phase::Reaped { code, signal, at } = state.phase else {
            return;
        };
        // A run that never printed its marker shows all it printed.
        state.release();
        let output = (self.run).then(|| RunOutput::from_transcript(state.emulator.transcript()));
        let exit = TermExit {
            code,
            signal,
            at,
            output,
        };
        let path = self.dir.join(EXIT);
        if let Err(err) = files::write_json(&path, &exit) {
            eprintln!("couldn't write {}: {err}", path.display());
        }
        state.phase = Phase::Exited(exit);
        state.password = false;
        state.busy = false;
        state.modes = TermModes::default();
        drop(state);
        self.bump();
    }

    /// Signals the program's process group, or the program alone if it has
    /// left it, and the group in the terminal's foreground, while the
    /// program hasn't been reaped.
    fn signal_child(&self, signal: Signal) {
        let state = self.lock();
        if !matches!(state.phase, Phase::Running) {
            return;
        }
        let foreground = self.master.foreground();
        if rustix::process::kill_process_group(self.child_pid, signal).is_err() {
            let _ = rustix::process::kill_process(self.child_pid, signal);
        }
        if let Some(group) = foreground.filter(|&group| group != self.child_pid) {
            let _ = rustix::process::kill_process_group(group, signal);
        }
    }

    /// Kills the group in the terminal's foreground when it isn't the
    /// program's own, while the program hasn't been reaped: a command the
    /// shell runs that ignored Ctrl-C ends, and the shell stays.
    fn stop_command(&self) {
        let state = self.lock();
        if !matches!(state.phase, Phase::Running) {
            return;
        }
        if let Some(group) = (self.master.foreground()).filter(|&group| group != self.child_pid) {
            let _ = rustix::process::kill_process_group(group, Signal::KILL);
        }
        drop(state);
    }

    /// Hangs the terminal up, then kills what's left after `grace`.
    /// Returns once the exit is recorded.
    async fn kill(self: Arc<Self>, grace: Duration) {
        self.signal_child(Signal::HUP);
        if self.wait_exit(Some(grace)).await {
            return;
        }
        self.signal_child(Signal::KILL);
        self.wait_exit(None).await;
    }

    /// Waits up to `limit` for the exit to be recorded. Returns whether it
    /// was.
    async fn wait_exit(&self, limit: Option<Duration>) -> bool {
        let mut changes = self.changed.subscribe();
        let wait = async {
            loop {
                changes.borrow_and_update();
                if self.exited() || changes.changed().await.is_err() {
                    return;
                }
            }
        };
        match limit {
            Some(limit) => tokio::time::timeout(limit, wait).await.is_ok(),
            None => {
                wait.await;
                true
            }
        }
    }

    /// Ends the holder: kills the program if it runs and goes without
    /// lingering.
    fn quit(self: &Arc<Self>) {
        self.lock().ending = true;
        self.bump();
        tokio::spawn(self.clone().kill(QUIT_GRACE));
    }

    async fn watch_signals(self: Arc<Self>) {
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            eprintln!("couldn't watch for signals");
            return;
        };
        loop {
            tokio::select! {
                _ = term.recv() => self.quit(),
                _ = int.recv() => self.quit(),
                // The holder has no terminal of its own, so a hangup means
                // nothing.
                _ = hup.recv() => {}
            }
        }
    }

    /// Deletes the folder once the program has ended, and ends the holder.
    /// A goodbye before the end means nothing.
    fn goodbye(&self) {
        let mut state = self.lock();
        if !matches!(state.phase, Phase::Exited(_)) {
            return;
        }
        if let Err(err) = files::remove_session(&self.dir) {
            eprintln!("couldn't remove {}: {err}", self.dir.display());
        }
        state.ending = true;
        drop(state);
        self.bump();
    }

    /// The hello for a client that has everything before `after`, where
    /// its output starts, and the first mark it gets, counting those gone.
    fn hello(&self, after: Option<u64>) -> (TermHello, View, u64) {
        let state = self.lock();
        let (from, reset) = state.ring.resume(after);
        let before = state.marks.iter().take_while(|placed| placed.offset < from);
        let first_mark = state.marks_gone + before.count() as u64;
        let view = View {
            password: state.password,
            busy: state.busy,
            title: state.title.clone(),
            cwd: state.cwd.clone(),
            modes: state.modes.clone(),
        };
        let hello = TermHello {
            holder_pid: self.pid,
            child_pid: self.child_pid.as_raw_nonzero().get().unsigned_abs(),
            started_at: self.started_at,
            from,
            reset,
            cols: state.cols,
            rows: state.rows,
            password: view.password,
            busy: view.busy,
            title: view.title.clone(),
            cwd: view.cwd.clone(),
            marks: true,
            modes: Some(view.modes.clone()),
        };
        (hello, view, first_mark)
    }

    /// What a connection at `cursor` sends next, with no output for a
    /// quiet one at `None`. One that asks for marks gets the one numbered
    /// `mark` once the output before it has gone.
    fn next(&self, cursor: Option<u64>, mark: Option<u64>, view: &View) -> Next {
        let state = self.lock();
        if let Some(cursor) = cursor
            && cursor < state.ring.end()
        {
            return match state.ring.read(cursor, CHUNK) {
                Some(data) => Next::Output(cursor, data),
                // The connection fell behind the ring, so it starts over
                // at a whole line. The offset shows the jump.
                None => {
                    let (from, _) = state.ring.resume(Some(cursor));
                    Next::Output(from, state.ring.read(from, CHUNK).unwrap_or_default())
                }
            };
        }
        if let Some(mark) = mark {
            // A connection that fell behind skips the marks that went.
            let mark = mark.max(state.marks_gone);
            let index = usize::try_from(mark - state.marks_gone).unwrap_or(usize::MAX);
            if let Some(placed) = state.marks.get(index) {
                return Next::Mark(mark, placed.clone());
            }
        }
        let now = View {
            password: state.password,
            busy: state.busy,
            title: state.title.clone(),
            cwd: state.cwd.clone(),
            modes: state.modes.clone(),
        };
        if now != *view {
            return Next::View(now);
        }
        Next::Idle(match &state.phase {
            Phase::Exited(exit) => Some(exit.clone()),
            _ => None,
        })
    }
}

/// What a connection sends next.
enum Next {
    Output(u64, Vec<u8>),
    /// The mark with this number.
    Mark(u64, Placed),
    View(View),
    /// Nothing new, with the exit once there is one.
    Idle(Option<TermExit>),
}

/// The messages that tell a client how `now` differs from `was`. Only one
/// that asked for `modes` hears of them.
fn changes(was: &View, now: &View, modes: bool) -> Vec<FromHolder> {
    let mut sent = Vec::new();
    if now.password != was.password {
        sent.push(FromHolder::Password { on: now.password });
    }
    if now.busy != was.busy {
        sent.push(FromHolder::Busy { on: now.busy });
    }
    if modes && now.modes != was.modes {
        sent.push(FromHolder::Modes(now.modes.clone()));
    }
    if let Some(title) = now.title.as_ref().filter(|_| now.title != was.title) {
        sent.push(FromHolder::Title {
            title: title.clone(),
        });
    }
    if let Some(cwd) = now.cwd.as_ref().filter(|_| now.cwd != was.cwd) {
        sent.push(FromHolder::Cwd { cwd: cwd.clone() });
    }
    sent
}

/// Serves one client: its hello, then the output from where it asked,
/// what changes, and the exit, while it sends input and the like.
async fn serve(holder: Arc<Holder>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut buf = Vec::new();
    let hello = tokio::time::timeout(HELLO_WAIT, wire::recv(&mut read, &mut buf)).await;
    let Ok(Ok(Some(ToHolder::Hello {
        terminal,
        key,
        after,
        quiet,
        marks,
        modes,
    }))) = hello
    else {
        return;
    };
    if let Some(message) = holder.refusal(terminal, &key) {
        let _ = wire::send(&mut write, &FromHolder::Refused { message }).await;
        return;
    }
    drop(key);
    let (hello, mut view, first_mark) = holder.hello(after);
    let mut cursor = (!quiet).then_some(hello.from);
    let mut mark = marks.then_some(first_mark);
    if wire::send(&mut write, &FromHolder::Hello(hello))
        .await
        .is_err()
    {
        return;
    }
    let mut requests = tokio::spawn(handle_requests(holder.clone(), read));
    let mut changes_seen = holder.changed.subscribe();
    let mut sent_exit = false;
    let served: io::Result<()> = async {
        loop {
            changes_seen.borrow_and_update();
            match holder.next(cursor, mark, &view) {
                Next::Output(offset, data) => {
                    cursor = Some(offset + data.len() as u64);
                    wire::send(&mut write, &FromHolder::Output { offset, data }).await?;
                }
                Next::Mark(sent, placed) => {
                    mark = Some(sent + 1);
                    let Placed { offset, at, mark } = placed;
                    wire::send(&mut write, &FromHolder::Mark { offset, at, mark }).await?;
                }
                Next::View(now) => {
                    for message in changes(&view, &now, modes) {
                        wire::send(&mut write, &message).await?;
                    }
                    view = now;
                }
                Next::Idle(Some(exit)) if !sent_exit => {
                    sent_exit = true;
                    wire::send(&mut write, &FromHolder::Exit(exit)).await?;
                }
                Next::Idle(_) => {
                    tokio::select! {
                        changed = changes_seen.changed() => if changed.is_err() { return Ok(()) },
                        _ = &mut requests => return Ok(()),
                    }
                }
            }
        }
    }
    .await;
    if let Err(err) = served
        && err.kind() != io::ErrorKind::BrokenPipe
    {
        eprintln!("a connection ended: {err}");
    }
    requests.abort();
}

/// Reads a client's requests and acts on each. Ends when the client
/// detaches or goes.
async fn handle_requests(holder: Arc<Holder>, read: BufReader<OwnedReadHalf>) {
    let mut read = read;
    let mut buf = Vec::new();
    while let Ok(Some(request)) = wire::recv::<_, ToHolder>(&mut read, &mut buf).await {
        match request {
            ToHolder::Hello { .. } => {}
            ToHolder::Input { data } => holder.type_in(data),
            ToHolder::Resize { cols, rows } => holder.resize(cols, rows),
            ToHolder::Interrupt => holder.type_in(Typed(vec![0x03])),
            ToHolder::Kill { grace_ms } => {
                tokio::spawn(holder.clone().kill(Duration::from_millis(grace_ms)));
            }
            ToHolder::StopCommand => holder.stop_command(),
            ToHolder::Detach => return,
            ToHolder::Goodbye => holder.goodbye(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(password: bool, title: Option<&str>) -> View {
        View {
            password,
            busy: false,
            title: title.map(str::to_owned),
            cwd: None,
            modes: TermModes::default(),
        }
    }

    #[test]
    fn only_what_changed_goes_out() {
        assert!(changes(&view(false, None), &view(false, None), true).is_empty());
        assert_eq!(
            changes(&view(false, None), &view(true, Some("vim")), true),
            [
                FromHolder::Password { on: true },
                FromHolder::Title {
                    title: "vim".into()
                }
            ]
        );
        // A title that goes away isn't news: the shell sets a new one.
        assert!(changes(&view(false, Some("vim")), &view(false, None), true).is_empty());
    }

    #[test]
    fn only_a_client_that_asked_hears_of_modes() {
        let waiting = View {
            modes: TermModes {
                reads_keys: true,
                waiting: true,
                ..TermModes::default()
            },
            ..view(false, None)
        };
        assert_eq!(
            changes(&view(false, None), &waiting, true),
            [FromHolder::Modes(waiting.modes.clone())]
        );
        assert!(changes(&view(false, None), &waiting, false).is_empty());
    }

    fn state(nonce: Option<&str>) -> State {
        State {
            ring: Ring::new(RING_BYTES),
            cutoff: None,
            emulator: maxhq_term::screen(80, 24),
            last_output: Instant::now(),
            modes: TermModes::default(),
            scanner: Scanner::new(),
            filter: Filter::new(nonce),
            marks: VecDeque::new(),
            marks_gone: 0,
            title: None,
            cwd: None,
            at_prompt: true,
            password: false,
            noted: false,
            busy: false,
            cols: 80,
            rows: 24,
            phase: Phase::Running,
            output_done: false,
            ending: false,
        }
    }

    #[test]
    fn only_the_shell_at_its_prompt_moves_the_folder() {
        let nonce = "0123456789abcdef0123456789abcdef";
        let mark = |kind: &str| format!("\x1b]133;{kind};maxhq={nonce}\x07");
        let folder = |path: &str| format!("\x1b]7;file://devbox{path}\x07");
        let mut shell = state(Some(nonce));
        // Before its first mark.
        shell.keep(folder("/start").as_bytes());
        assert_eq!(shell.cwd.as_deref(), Some("/start"));
        // What a command prints, and what's typed at the prompt, say nothing.
        shell.keep(format!("{}$ {}cat notes{}", mark("A"), mark("B"), mark("C")).as_bytes());
        shell.keep(folder("/printed").as_bytes());
        assert_eq!(shell.cwd.as_deref(), Some("/start"));
        // After the command ends and before the next prompt, the shell does.
        shell.keep(format!("{}{}{}$ ", mark("D;0"), folder("/moved"), mark("A")).as_bytes());
        assert_eq!(shell.cwd.as_deref(), Some("/moved"));
        shell.keep(folder("/typed").as_bytes());
        shell.keep(format!("{}{}", mark("B"), folder("/typed")).as_bytes());
        assert_eq!(shell.cwd.as_deref(), Some("/moved"));
        let Some(Placed {
            mark: Mark::Command { cwd },
            ..
        }) = shell.marks.get(2)
        else {
            panic!("no command mark: {:?}", shell.marks.len());
        };
        assert_eq!(cwd.as_deref(), Some("/start"));

        // A shell with no marks says its folder whenever it likes.
        let mut plain = state(None);
        plain.keep(format!("{}{}", mark("A"), folder("/a")).as_bytes());
        plain.keep(format!("{}{}", mark("C"), folder("/b")).as_bytes());
        assert_eq!(plain.cwd.as_deref(), Some("/b"));
    }
}
