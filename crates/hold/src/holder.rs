// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The holder process, `maxd hold <session folder>`.
//!
//! It starts in two steps. The first runs itself again with `--detached`
//! and exits, so the second is nobody's child and can't be a process group
//! leader. The second starts a new session with setsid, which takes it out
//! of the spawner's process group and away from any terminal. On Linux it
//! also becomes a child subreaper, so processes the CLI leaves behind come
//! back to it rather than to init.
//!
//! The second step takes the session's lock, reads its key and the secret
//! variables from stdin, binds its socket, starts the CLI in a process
//! group of its own and prints `ready`. From then on its stdout and stderr
//! go to `holder.log`. A holder running as root can start the CLI as
//! another user, as `user.rs` describes. It stays the CLI's parent and
//! subreaper either way.
//!
//! The key keeps the CLI itself, or anything else that can reach the
//! socket, from writing to the CLI's stdin, which would let an agent answer
//! its own permission prompt. A client whose hello doesn't show the key is
//! refused and disconnected. The key stays in memory and goes nowhere else.
//!
//! Each pipe has a thread of its own. The stdout thread records every line
//! before any client can see it, and only the stdin thread writes to the
//! CLI, a whole line at a time. Clients are tasks on a small runtime.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::future::Future;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use maxhq_protocol::ids::SessionId;
use maxhq_protocol::time::Timestamp;
use rustix::process::{Pid, Signal, WaitOptions};
use tokio::io::BufReader as AsyncBufReader;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, oneshot, watch};

use crate::files::{
    self, HolderFile, LaunchFile, OsBytes, PendingEntry, EXIT, HOLDER, HOLDER_LOG, LOCK, PENDING,
    STDERR,
};
use crate::log::{Entry, EventLog, LogReader, Tail};
use crate::pending::{self, Claim, Pending};
use crate::secret::{self, Piped, Secret, Zeroed, PIPE_CAP};
use crate::socket::{self, Socket};
use crate::stderr::StderrLog;
use crate::user;
use crate::wire::{self, Failure, FromHolder, ToHolder};
use crate::{ChildState, Exit, Framing, Limits, WriteOutcome};

/// The flag the first step adds when it runs the second.
pub(crate) const DETACHED: &str = "--detached";

/// The flag that makes `maxd hold` a terminal's holder.
pub(crate) const PTY: &str = "--pty";

/// What reading a line of a CLI's stdout found.
#[derive(Debug, PartialEq, Eq)]
enum ReadLine {
    /// The stdout has ended.
    End,
    /// The line is in the buffer, without its newline.
    Kept,
    /// The line ran past the cap, so the buffer is empty. It had this many
    /// bytes, with its newline if it had one.
    TooLong(u64),
}

/// Reads a line of up to `cap` bytes into `buf`, without its newline. A longer
/// one is read past and not kept, so a line with no end can't fill the memory.
fn read_line_within(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    cap: usize,
) -> io::Result<ReadLine> {
    buf.clear();
    let read = reader
        .by_ref()
        .take(cap as u64 + 1)
        .read_until(b'\n', buf)?;
    if read == 0 {
        return Ok(ReadLine::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(ReadLine::Kept);
    }
    if read <= cap {
        return Ok(ReadLine::Kept);
    }
    buf.clear();
    let skipped = reader.skip_until(b'\n')?;
    Ok(ReadLine::TooLong((read + skipped) as u64))
}

/// How long the holder waits for its lock, in case a client is checking
/// it at that moment.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// How long the spawner has to write the secret variables.
const SECRET_WAIT: Duration = Duration::from_secs(10);

/// How long a client has to say hello.
pub(crate) const HELLO_WAIT: Duration = Duration::from_secs(10);

/// How long the holder waits for the CLI's pipes to close after it exits.
/// Something the CLI started can keep them open, and its output after the
/// CLI has gone isn't recorded.
pub(crate) const DRAIN_WAIT: Duration = Duration::from_secs(2);

/// How often the holder checks for the CLI's exit besides SIGCHLD.
pub(crate) const REAP_TICK: Duration = Duration::from_millis(200);

/// The grace a SIGTERM or SIGINT to the holder gives the CLI.
pub(crate) const QUIT_GRACE: Duration = Duration::from_secs(2);

/// How long the holder stays after deciding to go, so connections can send
/// what they have.
pub(crate) const FLUSH_WAIT: Duration = Duration::from_millis(200);

/// The most lines a connection takes in one go.
const BATCH: usize = 256;

/// Runs `maxd hold`. `args` are the arguments after `hold`: `--pty` for a
/// terminal's holder, the session's or terminal's folder, and `--detached`
/// for the second step.
///
/// The first step runs this program again with its own arguments and
/// `--detached` added, so the program has to reach this function the same
/// way both times.
pub fn run_holder(args: &[OsString]) -> ExitCode {
    match args {
        [_dir] => detach(),
        [dir, flag] if flag == DETACHED => hold(Path::new(dir)),
        [pty, _dir] if pty == PTY => detach(),
        [pty, dir, flag] if pty == PTY && flag == DETACHED => crate::terminal::hold(Path::new(dir)),
        _ => {
            eprintln!("usage: maxd hold [--pty] <session or terminal folder>");
            ExitCode::from(2)
        }
    }
}

/// The first step: starts the second and exits without waiting for it.
fn detach() -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("couldn't find this program: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Stdin goes down to the second step, which reads its key and the
    // secret variables from it.
    let spawned = Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .arg(DETACHED)
        .spawn();
    match spawned {
        // Init or the nearest subreaper waits for it once this exits.
        Ok(_second) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("couldn't run {}: {err}", exe.display());
            ExitCode::FAILURE
        }
    }
}

/// The second step: the holder itself.
fn hold(dir: &Path) -> ExitCode {
    detached(dir, |dir| async move {
        match Started::start(&dir) {
            Ok(started) => {
                started.run().await;
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        }
    })
}

/// Leaves the spawner's session and folder, then runs `body` with the
/// holder's absolute folder on a small runtime. An agent's holder and a
/// terminal's start the same way.
pub(crate) fn detached<F, Fut>(dir: &Path, body: F) -> ExitCode
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = ExitCode>,
{
    let dir = match std::path::absolute(dir) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("couldn't resolve {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = rustix::process::setsid() {
        eprintln!("couldn't start a new session: {err}");
        return ExitCode::FAILURE;
    }
    #[cfg(target_os = "linux")]
    if let Err(err) = rustix::process::set_child_subreaper(Some(rustix::process::getpid())) {
        eprintln!("couldn't become a child subreaper: {err}");
    }
    // The holder outlives whatever folder it was started in, so it keeps
    // none of them busy.
    let _ = std::env::set_current_dir("/");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("couldn't start the runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(body(dir));
    runtime.shutdown_timeout(FLUSH_WAIT);
    code
}

/// What the second step sets up before saying it's ready.
struct Started {
    holder: Arc<Holder>,
    socket: Socket,
    _lock: File,
}

impl Started {
    fn start(dir: &Path) -> Result<Self, String> {
        let at = |path: &Path| {
            let path = path.to_owned();
            move |err: io::Error| format!("couldn't set up {}: {err}", path.display())
        };
        let lock = take_lock(&dir.join(LOCK))?;
        let launch: LaunchFile = files::read_json(&dir.join(files::LAUNCH))
            .map_err(at(&dir.join(files::LAUNCH)))?
            .ok_or_else(|| format!("{} has no {}", dir.display(), files::LAUNCH))?;
        if dir.join(HOLDER).exists() || dir.join(EXIT).exists() {
            return Err(format!("session {} has already run", launch.session));
        }
        let (key, secrets) = read_stdin(launch.keyed, &launch.secret_env)?;
        quiet_stdin();
        let cwd = PathBuf::from(launch.cwd.0.clone());
        let run_as = match &launch.run_as {
            Some(run_as) => Some(user::prepare(&run_as.run_as(), &cwd)?),
            None => None,
        };
        let sockets = PathBuf::from(launch.sockets.0.clone());
        socket::make_dir(&sockets).map_err(at(&sockets))?;
        let socket = socket::bind(&sockets, &launch.session).map_err(at(&sockets))?;
        socket
            .listener
            .set_nonblocking(true)
            .map_err(at(&socket.path))?;
        let limits = launch.limits();
        let log = EventLog::create(dir, limits.rotate_at).map_err(at(dir))?;
        let stderr = StderrLog::create(&dir.join(STDERR), limits.stderr_cap).map_err(at(dir))?;
        files::write_json(&dir.join(PENDING), &Vec::<PendingEntry>::new())
            .map_err(at(&dir.join(PENDING)))?;

        let program = PathBuf::from(launch.program.0.clone());
        let mut command = Command::new(&program);
        command
            .args(launch.args.iter().map(|arg| &arg.0))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        for key in &launch.env_remove {
            command.env_remove(&key.0);
        }
        if let Some(run_as) = &run_as {
            for (key, value) in &run_as.env {
                command.env(key, value);
            }
        }
        for (key, value) in &launch.env {
            command.env(&key.0, &value.0);
        }
        for (key, value) in &secrets {
            command.env(key, value.expose());
        }
        drop(secrets);
        match run_as {
            Some(run_as) => run_as.apply(&mut command)?,
            None => {
                command.current_dir(&cwd);
            }
        }
        let spawned = command.spawn();
        // The command has its own copy of the secret values.
        drop(command);
        let mut child = spawned.map_err(|err| spawn_failure(&program, &cwd, &err))?;
        let (Some(stdin), Some(stdout), Some(stderr_pipe)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("the agent started without its pipes".into());
        };
        let child_pid = i32::try_from(child.id())
            .ok()
            .and_then(Pid::from_raw)
            .ok_or("the agent has no process ID")?;
        // The holder reaps the CLI itself, so it can signal its group only
        // while the ID still belongs to it.
        drop(child);

        let (stdin_jobs, stdin_rx) = std_mpsc::channel();
        let holder = Arc::new(Holder {
            dir: dir.to_owned(),
            session: launch.session,
            key,
            framing: launch.framing,
            limits,
            pid: std::process::id(),
            child_pid,
            state: Mutex::new(State {
                log,
                tail: Tail::new(launch.memory_tail),
                pending: Pending::default(),
                phase: Phase::Running,
                stdin_open: true,
                out_done: false,
                err_done: false,
                ending: false,
            }),
            stderr: Mutex::new(stderr),
            stdin: stdin_jobs,
            changed: watch::Sender::new(0),
        });
        let record = HolderFile {
            holder_pid: holder.pid,
            child_pid: child_pid.as_raw_nonzero().get().unsigned_abs(),
            socket: socket.path.as_path().into(),
            started_at: Timestamp::now(),
        };
        if let Err(err) = files::write_json(&dir.join(HOLDER), &record) {
            holder.signal_child(Signal::KILL);
            return Err(format!("couldn't write {HOLDER}: {err}"));
        }

        spawn_thread("stdin", move || write_stdin(stdin, stdin_rx));
        let reader = holder.clone();
        spawn_thread("stdout", move || reader.read_stdout(stdout));
        let reader = holder.clone();
        spawn_thread("stderr", move || reader.read_stderr(stderr_pipe));

        say_ready(dir);
        Ok(Self {
            holder,
            socket,
            _lock: lock,
        })
    }

    async fn run(self) {
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
        tokio::spawn(holder.clone().reap_loop());
        tokio::spawn(holder.clone().watch_signals());

        let mut changes = holder.changed.subscribe();
        while !holder.exited() {
            changes.borrow_and_update();
            if holder.exited() || changes.changed().await.is_err() {
                break;
            }
        }
        let linger = tokio::time::sleep(holder.limits.linger);
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

/// Why the CLI didn't start. A folder that's gone fails the spawn just as a
/// missing program does, whether `current_dir` or a user's `chdir` enters
/// it, so the folder is checked before the program is named.
fn spawn_failure(program: &Path, cwd: &Path, err: &io::Error) -> String {
    if err.kind() == io::ErrorKind::NotFound && matches!(cwd.try_exists(), Ok(false)) {
        format!("the folder {} is gone", cwd.display())
    } else {
        format!("couldn't start {}: {err}", program.display())
    }
}

/// Takes the session's lock, waiting a moment in case a client has it.
pub(crate) fn take_lock(path: &Path) -> Result<File, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| format!("couldn't open {}: {err}", path.display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                // The folder may have been cleared away while this waited,
                // and then the lock is on a file nobody else can see.
                let locked = lock
                    .metadata()
                    .map_err(|err| format!("couldn't check {}: {err}", path.display()))?;
                return match fs::metadata(path) {
                    Ok(there) if there.ino() == locked.ino() && there.dev() == locked.dev() => {
                        Ok(lock)
                    }
                    _ => Err(format!(
                        "{} went away as the holder started",
                        path.display()
                    )),
                };
            }
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(format!("another holder has {}", path.display()));
            }
            Err(TryLockError::Error(err)) => {
                return Err(format!("couldn't lock {}: {err}", path.display()));
            }
        }
    }
}

/// Reads what the spawner writes on stdin: the holder's key when `keyed`,
/// then the secret variables, which must be the ones `names` lists, in
/// order. With no key and no names it reads nothing.
pub(crate) fn read_stdin(keyed: bool, names: &[OsBytes]) -> Result<Piped, String> {
    if !keyed && names.is_empty() {
        return Ok((None, Vec::new()));
    }
    let stdin = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|err| format!("couldn't read the secret variables: {err}"))?;
    // A thread does the reading, so a spawner that never writes can't hold
    // the holder up past the wait.
    let (done, read) = std_mpsc::channel();
    spawn_thread("secrets", move || {
        let cap = u64::try_from(PIPE_CAP)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Zeroed(Vec::with_capacity(PIPE_CAP + 1));
        let result = File::from(stdin).take(cap).read_to_end(&mut bytes.0);
        let _ = done.send(result.map(|_| bytes));
    });
    let bytes = match read.recv_timeout(SECRET_WAIT) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(err)) => return Err(format!("couldn't read the secret variables: {err}")),
        Err(_) => return Err("the secret variables didn't come on stdin in time".into()),
    };
    if bytes.0.len() > PIPE_CAP {
        return Err("the secret variables on stdin come to more than 1 MiB".into());
    }
    let (key, secrets) = secret::decode(&bytes.0, keyed)?;
    let matches = secrets.len() == names.len()
        && secrets
            .iter()
            .zip(names)
            .all(|((name, _), listed)| *name == listed.0);
    if !matches {
        return Err(format!(
            "the secret variables on stdin aren't the ones {} lists",
            files::LAUNCH
        ));
    }
    Ok((key, secrets))
}

/// Points stdin at `/dev/null`, so nothing else reads the spawner's pipe.
pub(crate) fn quiet_stdin() {
    if let Ok(null) = File::open("/dev/null") {
        let _ = rustix::stdio::dup2_stdin(&null);
    }
}

/// Tells the spawner the socket takes connections, then sends the holder's
/// own output to `holder.log`. That closes the spawner's pipes, so it
/// isn't left waiting on them.
pub(crate) fn say_ready(dir: &Path) {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"ready\n").and_then(|()| stdout.flush());
    match files::append(&dir.join(HOLDER_LOG)) {
        Ok(log) => {
            let _ = rustix::stdio::dup2_stdout(&log);
            let _ = rustix::stdio::dup2_stderr(&log);
        }
        Err(_) => {
            if let Ok(null) = File::options().write(true).open("/dev/null") {
                let _ = rustix::stdio::dup2_stdout(&null);
                let _ = rustix::stdio::dup2_stderr(&null);
            }
        }
    }
}

pub(crate) fn spawn_thread(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(err) = thread::Builder::new().name(name.into()).spawn(work) {
        eprintln!("couldn't start the {name} thread: {err}");
    }
}

/// A line for the CLI, or the end of its input.
enum StdinJob {
    Line(Vec<u8>, oneshot::Sender<io::Result<()>>),
    Close,
}

/// The stdin thread. It's the only writer, so lines never interleave.
fn write_stdin(stdin: ChildStdin, jobs: std_mpsc::Receiver<StdinJob>) {
    let mut stdin = Some(stdin);
    for job in jobs {
        match job {
            StdinJob::Line(bytes, done) => {
                let result = match stdin.as_mut() {
                    Some(pipe) => pipe.write_all(&bytes).and_then(|()| pipe.flush()),
                    None => Err(io::ErrorKind::BrokenPipe.into()),
                };
                let _ = done.send(result);
            }
            StdinJob::Close => stdin = None,
        }
    }
}

/// Where the CLI is in its life.
enum Phase {
    Running,
    /// Reaped, with the holder still reading what it printed last.
    Reaped {
        code: Option<i32>,
        signal: Option<i32>,
        at: Timestamp,
    },
    Exited(Exit),
}

struct State {
    log: EventLog,
    tail: Tail,
    pending: Pending,
    phase: Phase,
    stdin_open: bool,
    out_done: bool,
    err_done: bool,
    /// The holder should go once the CLI has exited: a client saw the exit,
    /// the files were forgotten or the holder was told to quit.
    ending: bool,
}

/// A write that passed the pending check.
enum Queued {
    Sent(oneshot::Receiver<io::Result<()>>),
    Already,
}

/// What a connection sends next.
enum Next {
    Lines(Vec<Entry>),
    /// The lines are older than the memory tail, so they come from disk.
    /// The number is where to skip to if the disk has none of them.
    Disk(LogReader, u64),
    /// Nothing new, with the exit once there is one.
    Idle(Option<Exit>),
}

struct Holder {
    dir: PathBuf,
    session: SessionId,
    /// What a client's hello must show, or `None` for a holder whose
    /// spawner is from before keys.
    key: Option<Secret>,
    framing: Framing,
    limits: Limits,
    pid: u32,
    child_pid: Pid,
    state: Mutex<State>,
    stderr: Mutex<StderrLog>,
    stdin: std_mpsc::Sender<StdinJob>,
    /// Counts changes, so tasks can wait for the next one.
    changed: watch::Sender<u64>,
}

impl Holder {
    /// Why a client whose hello names `session` and shows `key` is turned
    /// away, or `None` when it's welcome. The key comes first, so a client
    /// without it learns nothing else.
    fn refusal(&self, session: SessionId, key: Option<&Secret>) -> Option<String> {
        if let Some(own) = &self.key {
            match key {
                None => return Some("this holder takes only clients that show its key".into()),
                Some(key) if !secret::key_matches(own, key) => {
                    return Some("the key in the hello isn't this holder's".into());
                }
                Some(_) => {}
            }
        }
        (session != self.session).then(|| format!("this holder runs session {}", self.session))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stderr_log(&self) -> MutexGuard<'_, StderrLog> {
        self.stderr.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    fn exited(&self) -> bool {
        matches!(self.lock().phase, Phase::Exited(_))
    }

    fn persist_pending(&self, state: &State) {
        let path = self.dir.join(PENDING);
        if let Err(err) = files::write_json(&path, &state.pending.to_file()) {
            eprintln!("couldn't write {}: {err}", path.display());
        }
    }

    /// The stdout thread.
    fn read_stdout(&self, stdout: ChildStdout) {
        let mut reader = BufReader::with_capacity(64 * 1024, stdout);
        let mut buf = Vec::new();
        loop {
            match read_line_within(&mut reader, &mut buf, wire::MAX_LINE) {
                Ok(ReadLine::End) => break,
                Ok(ReadLine::Kept) => self.record(String::from_utf8_lossy(&buf).into_owned()),
                Ok(ReadLine::TooLong(bytes)) => {
                    // Not JSON, so every driver skips it as it skips any other
                    // stray line, and the log still shows a line went missing.
                    let marker = format!(
                        "maxhq: left out a line of {bytes} bytes from the agent's output, past the limit of {} bytes",
                        wire::MAX_LINE
                    );
                    eprintln!("{marker}");
                    self.record(marker);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    eprintln!("stopped reading the agent's stdout: {err}");
                    break;
                }
            }
        }
        self.lock().out_done = true;
        self.bump();
    }

    /// Records a line, and any prompt it asks or withdraws, before a client
    /// can see it.
    fn record(&self, text: String) {
        let effect = pending::from_cli(self.framing, &text);
        let mut state = self.lock();
        if matches!(state.phase, Phase::Exited(_)) {
            return;
        }
        let entry = state.log.append(Timestamp::now(), text.into());
        if state.pending.apply(effect, &entry) {
            self.persist_pending(&state);
        }
        state.tail.push(entry);
        drop(state);
        self.bump();
    }

    /// The stderr thread.
    fn read_stderr(&self, mut stderr: ChildStderr) {
        let mut buf = vec![0; 8192];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.stderr_log().append(&buf[..n]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    eprintln!("stopped reading the agent's stderr: {err}");
                    break;
                }
            }
        }
        self.lock().err_done = true;
        self.bump();
    }

    /// Reaps every child that has exited. Returns whether the CLI has.
    ///
    /// This holds the state lock, as [`Holder::signal_child`] does, so the
    /// CLI's process ID is never signalled after it's reaped and could be
    /// reused.
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
            let drained = {
                let state = self.lock();
                state.out_done && state.err_done
            };
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

    /// Records the exit once the CLI's output is in.
    fn finish(&self) {
        let mut state = self.lock();
        let Phase::Reaped { code, signal, at } = state.phase else {
            return;
        };
        let exit = Exit {
            code,
            signal,
            at,
            stderr_tail: self.stderr_log().tail(self.limits.stderr_tail),
        };
        let path = self.dir.join(EXIT);
        if let Err(err) = files::write_json(&path, &exit) {
            eprintln!("couldn't write {}: {err}", path.display());
        }
        if state.pending.clear() {
            self.persist_pending(&state);
        }
        state.phase = Phase::Exited(exit);
        drop(state);
        self.bump();
    }

    /// Signals the CLI's process group, or the CLI alone if it has left
    /// the group, while it hasn't been reaped.
    fn signal_child(&self, signal: Signal) {
        let state = self.lock();
        if !matches!(state.phase, Phase::Running) {
            return;
        }
        if rustix::process::kill_process_group(self.child_pid, signal).is_err() {
            let _ = rustix::process::kill_process(self.child_pid, signal);
        }
    }

    /// Ends the CLI: closes its stdin after the writes already queued, then
    /// sends SIGTERM to its group after `grace` and SIGKILL after another.
    /// Returns once the exit is recorded.
    async fn stop(self: Arc<Self>, grace: Duration) {
        {
            let mut state = self.lock();
            if state.stdin_open {
                state.stdin_open = false;
                let _ = self.stdin.send(StdinJob::Close);
            }
        }
        if self.wait_exit(Some(grace)).await {
            return;
        }
        self.signal_child(Signal::TERM);
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

    /// Ends the holder: stops the CLI if it runs and goes without lingering.
    fn quit(self: &Arc<Self>) {
        self.lock().ending = true;
        self.bump();
        tokio::spawn(self.clone().stop(QUIT_GRACE));
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
                // The holder has no terminal, so a hangup means nothing.
                _ = hup.recv() => {}
            }
        }
    }

    /// Checks a write against the pending prompts and queues it for the
    /// stdin thread. Both happen under the state lock, so writes reach the
    /// CLI in the order they were claimed.
    fn write(&self, line: String, answers: Option<String>) -> Result<Queued, Failure> {
        let line = match line.strip_suffix('\n') {
            Some(trimmed) => trimmed.to_owned(),
            None => line,
        };
        if line.contains('\n') {
            return Err(Failure::BadLine);
        }
        let mut state = self.lock();
        if !matches!(state.phase, Phase::Running) || !state.stdin_open {
            return Err(Failure::Finished);
        }
        match state.pending.claim(self.framing, &line, answers.as_deref()) {
            Claim::Already => return Ok(Queued::Already),
            Claim::Answered(_) => self.persist_pending(&state),
            Claim::Pass => {}
        }
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        let (done, sent) = oneshot::channel();
        self.stdin
            .send(StdinJob::Line(bytes, done))
            .map_err(|_| Failure::Finished)?;
        Ok(Queued::Sent(sent))
    }

    async fn write_reply(&self, id: u64, line: String, answers: Option<String>) -> FromHolder {
        let outcome = match self.write(line, answers) {
            Err(error) => return FromHolder::Failed { id, error },
            Ok(Queued::Already) => WriteOutcome::AlreadyAnswered,
            Ok(Queued::Sent(sent)) => match sent.await {
                Ok(Ok(())) => WriteOutcome::Written,
                Ok(Err(err)) if err.kind() != io::ErrorKind::BrokenPipe => {
                    let error = Failure::Io {
                        message: format!("couldn't write to the agent: {err}"),
                    };
                    return FromHolder::Failed { id, error };
                }
                _ => {
                    let error = Failure::Finished;
                    return FromHolder::Failed { id, error };
                }
            },
        };
        FromHolder::Written { id, outcome }
    }

    /// Deletes the session's files once the CLI has exited, and ends the
    /// holder.
    fn forget(&self, id: u64) -> FromHolder {
        let mut state = self.lock();
        if !matches!(state.phase, Phase::Exited(_)) {
            let error = Failure::StillRunning;
            return FromHolder::Failed { id, error };
        }
        match files::remove_session(&self.dir) {
            Ok(()) => {}
            Err(err) => {
                let message = format!("couldn't remove {}: {err}", self.dir.display());
                return FromHolder::Failed {
                    id,
                    error: Failure::Io { message },
                };
            }
        }
        state.ending = true;
        drop(state);
        self.bump();
        FromHolder::Forgotten { id }
    }

    /// A client has seen the exit.
    fn saw_exit(&self) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Exited(_)) {
            state.ending = true;
            drop(state);
            self.bump();
        }
    }

    /// The hello for a client that has every line up to `after`, with where
    /// its replay starts and ends.
    fn hello(&self, after: Option<u64>) -> (FromHolder, u64, u64) {
        let state = self.lock();
        let last_seq = state.log.last_seq();
        let first_seq = state
            .log
            .first_seq()
            .min(state.tail.first_seq().unwrap_or(u64::MAX));
        let child = match &state.phase {
            Phase::Exited(exit) => ChildState::Exited(exit.clone()),
            _ => ChildState::Running {
                pid: self.child_pid.as_raw_nonzero().get().unsigned_abs(),
            },
        };
        let pending = state
            .pending
            .entries()
            .map(|entry| entry.line(true))
            .collect();
        let cursor = after.map_or(last_seq, |after| after.min(last_seq));
        let hello = FromHolder::Hello {
            holder_pid: self.pid,
            child,
            first_seq,
            last_seq,
            pending,
        };
        (hello, cursor, last_seq)
    }

    fn next(&self, cursor: u64) -> Next {
        let state = self.lock();
        if cursor >= state.log.last_seq() {
            return Next::Idle(match &state.phase {
                Phase::Exited(exit) => Some(exit.clone()),
                _ => None,
            });
        }
        if state.tail.covers(cursor) {
            return Next::Lines(state.tail.after(cursor, BATCH));
        }
        let skip_to = state
            .tail
            .first_seq()
            .map_or(state.log.last_seq(), |first| first - 1);
        match LogReader::open(&self.dir) {
            Ok(reader) => Next::Disk(reader, skip_to),
            Err(err) => {
                eprintln!("couldn't read the log for a replay: {err}");
                Next::Lines(state.tail.after(cursor, BATCH))
            }
        }
    }

    fn tail_covers(&self, cursor: u64) -> bool {
        self.lock().tail.covers(cursor)
    }
}

/// Serves one client: its hello, then every line after its cursor, then the
/// exit, with replies to its requests in between.
async fn serve(holder: Arc<Holder>, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut read = AsyncBufReader::new(read);
    let mut buf = Vec::new();
    let hello = tokio::time::timeout(HELLO_WAIT, wire::recv(&mut read, &mut buf)).await;
    let Ok(Ok(Some(ToHolder::Hello {
        session,
        after,
        key,
    }))) = hello
    else {
        return;
    };
    if let Some(message) = holder.refusal(session, key.as_ref()) {
        let _ = wire::send(&mut write, &FromHolder::Refused { message }).await;
        return;
    }
    let (hello, mut cursor, replay_upto) = holder.hello(after);
    if wire::send(&mut write, &hello).await.is_err() {
        return;
    }
    let (replies_tx, mut replies) = mpsc::channel(64);
    let requests = tokio::spawn(handle_requests(holder.clone(), read, replies_tx));
    let mut changes = holder.changed.subscribe();
    let mut sent_exit = false;
    let line = |entry: &Entry| FromHolder::Line(entry.line(entry.seq <= replay_upto));
    let served: io::Result<()> = async {
        loop {
            loop {
                match replies.try_recv() {
                    Ok(reply) => wire::send(&mut write, &reply).await?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            changes.borrow_and_update();
            match holder.next(cursor) {
                Next::Lines(entries) => {
                    for entry in &entries {
                        wire::send(&mut write, &line(entry)).await?;
                        cursor = entry.seq;
                    }
                }
                Next::Disk(reader, skip_to) => {
                    let mut reader = reader;
                    let mut sent_any = false;
                    loop {
                        let from = cursor;
                        let (back, batch) = tokio::task::spawn_blocking(move || {
                            let batch = reader.batch(from, BATCH);
                            (reader, batch)
                        })
                        .await
                        .map_err(io::Error::other)?;
                        reader = back;
                        if batch.is_empty() {
                            break;
                        }
                        sent_any = true;
                        for entry in &batch {
                            wire::send(&mut write, &line(entry)).await?;
                            cursor = entry.seq;
                        }
                        if holder.tail_covers(cursor) {
                            break;
                        }
                    }
                    if !sent_any {
                        cursor = cursor.max(skip_to);
                    }
                }
                Next::Idle(Some(exit)) if !sent_exit => {
                    sent_exit = true;
                    wire::send(&mut write, &FromHolder::Exited(exit)).await?;
                }
                Next::Idle(_) => {
                    tokio::select! {
                        changed = changes.changed() => if changed.is_err() { return Ok(()) },
                        reply = replies.recv() => match reply {
                            Some(reply) => wire::send(&mut write, &reply).await?,
                            None => return Ok(()),
                        },
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

/// Reads a client's requests and answers each in turn.
async fn handle_requests(
    holder: Arc<Holder>,
    read: AsyncBufReader<OwnedReadHalf>,
    replies: mpsc::Sender<FromHolder>,
) {
    let mut read = read;
    let mut buf = Vec::new();
    while let Ok(Some(request)) = wire::recv::<_, ToHolder>(&mut read, &mut buf).await {
        let reply = match request {
            ToHolder::Hello { .. } => continue,
            ToHolder::Write { id, line, answers } => holder.write_reply(id, line, answers).await,
            ToHolder::Stop { id, grace_ms } => {
                let holder = holder.clone();
                let replies = replies.clone();
                tokio::spawn(async move {
                    holder.stop(Duration::from_millis(grace_ms)).await;
                    let _ = replies.send(FromHolder::Stopped { id }).await;
                });
                continue;
            }
            ToHolder::Forget { id } => holder.forget(id),
            ToHolder::SawExit => {
                holder.saw_exit();
                continue;
            }
        };
        if replies.send(reply).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads every line of `text` with a cap of 4 bytes.
    fn read_all(text: &[u8]) -> Vec<(ReadLine, Vec<u8>)> {
        let mut reader = BufReader::with_capacity(2, text);
        let mut buf = Vec::new();
        let mut found = Vec::new();
        loop {
            let line = read_line_within(&mut reader, &mut buf, 4).unwrap();
            if line == ReadLine::End {
                return found;
            }
            found.push((line, buf.clone()));
        }
    }

    #[test]
    fn a_line_at_the_cap_stays_and_one_past_it_goes_with_the_next_intact() {
        assert_eq!(
            read_all(b"abcd\nabcde\nnext\n"),
            [
                (ReadLine::Kept, b"abcd".to_vec()),
                (ReadLine::TooLong(6), Vec::new()),
                (ReadLine::Kept, b"next".to_vec()),
            ]
        );
    }

    #[test]
    fn a_last_line_without_a_newline_follows_the_same_cap() {
        assert_eq!(read_all(b"abcd"), [(ReadLine::Kept, b"abcd".to_vec())]);
        assert_eq!(read_all(b"abcde"), [(ReadLine::TooLong(5), Vec::new())]);
        assert_eq!(read_all(b"\n"), [(ReadLine::Kept, Vec::new())]);
    }

    #[test]
    fn a_line_with_no_end_keeps_no_more_than_the_cap() {
        let mut reader = BufReader::new(io::repeat(b'x').take(1 << 20));
        let mut buf = Vec::new();
        let found = read_line_within(&mut reader, &mut buf, 4).unwrap();
        assert_eq!(found, ReadLine::TooLong(1 << 20));
        assert!(buf.capacity() < 1 << 16, "{}", buf.capacity());
    }
}
