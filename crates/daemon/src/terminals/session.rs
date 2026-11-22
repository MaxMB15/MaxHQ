// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An agent's session: the one shell its `!` commands share, so `cd` and
//! `export` carry from one to the next.
//!
//! A session is a drawer shell of kind `Session`, with the device's shell
//! integration, so it marks its prompts and commands. A command goes into
//! it only when it waits at its prompt with nothing typed there. It starts
//! as one when the agent runs its first `!`, or when you pick a folder for
//! it, and runs on, through `maxd` restarts, until the agent ends, you end
//! it, or its shell exits.
//!
//! Each command becomes a block. A link to the holder that hears the
//! shell's marks among the output cuts the block's output from the mark
//! that starts the command to the one that ends it, and renders it as a
//! `!` run's output is rendered. A password you send at a prompt that
//! hides what you type counts as one, and the block's output says so in
//! its place. What you type never goes anywhere but the holder.
//!
//! When the session can't take a command, a `!` runs in a fresh shell as
//! it did before sessions, and the manager hears why.

use std::path::Component;
use std::time::Instant;

use maxhq_hold::Mark;
use maxhq_protocol::terminal::FreshReason;
use maxhq_term::{Emulator, Transcript};

use super::*;

/// How long a new session has to show its first marked prompt.
const FIRST_PROMPT_WAIT: Duration = Duration::from_secs(15);

/// How often a request looks again at a session it waits for.
const LOOK: Duration = Duration::from_millis(25);

/// How long a session that just ran something has to settle at its
/// prompt before a command goes to a fresh shell instead.
const SETTLE_WAIT: Duration = Duration::from_millis(300);

/// How long a block whose shell showed a new prompt, but no start, has to
/// end before a command goes to a fresh shell instead.
const ENDING_WAIT: Duration = Duration::from_secs(1);

/// How long a line the shell showed a new prompt after, with no command
/// started, waits for a late start before it ends with no code: a comment,
/// a syntax error, or a shell that marks nothing.
const UNMARKED_GRACE: Duration = Duration::from_millis(300);

/// How long a block that ended waits for the prompt after it.
const PROMPT_WAIT: Duration = Duration::from_secs(1);

/// How long a block's line has to start, with the shell holding the
/// foreground, before `maxd` takes it that the shell waits for more of
/// it, such as a closing quote, and cancels it.
const CONTINUATION_WAIT: Duration = Duration::from_secs(3);

/// How long a line you sent while something ran has to start once the
/// shell shows its prompt. Past it, `maxd` takes it that what ran read
/// the line.
const AHEAD_WAIT: Duration = Duration::from_millis(500);

/// How long a block waits after the prompt for the last of what comes
/// with it, such as the folder.
const SETTLE: Duration = Duration::from_millis(50);

/// How much output a running block holds back from the emulator, waiting
/// for the mark that ends it. Past it, all but [`HOLD_KEEP`] goes in.
const HOLD_MAX: usize = 1024 * 1024;

/// How much of the output a running block holds back once it goes past
/// [`HOLD_MAX`].
const HOLD_KEEP: usize = 512 * 1024;

/// How much of a command a busy session's refusal names.
const NAMED_CHARS: usize = 60;

const NO_FOLDER_GIVEN: &str = "Say which folder to go to.";
const CONTROL_IN_FOLDER: &str =
    "A folder's name can't hold a line break or another control character.";
const NO_MARKS: &str = "Sessions need a shell that reports its commands: zsh, bash or fish.";
const SESSION_ENDED: &str = "The session ended. Try again.";
const NO_FIRST_PROMPT: &str = "Your shell didn't report its commands, so the session closed.";
const BUSY_UNNAMED: &str = "The session is busy. Wait for what runs in it to finish, or stop it.";
const TYPED: &str = "The session's prompt has something you typed. Run it or clear it with \
                     Ctrl-C, then try again.";
const CANCELLED: &str =
    "The shell waited for more input, such as a closing quote, so MaxHQ cancelled it.";

/// Something you asked an agent's session to do.
#[derive(Clone, Debug)]
pub struct SessionRequest {
    /// The request's key, which [`RunNote::Session`] carries back, and
    /// [`RunNote::Opened`] too when it runs in a fresh shell.
    pub key: IdempotencyKey,
    /// The agent whose session it is.
    pub agent: AgentId,
    /// What to do.
    pub ask: SessionAsk,
    /// The agent's folder, where a new session starts.
    pub cwd: PathBuf,
    /// The width for a new session or fresh shell, in cells.
    pub cols: u16,
    /// The height for a new session or fresh shell, in cells.
    pub rows: u16,
    /// Who asked.
    pub opener: Opener,
    /// The manager's ID for the connection that asked.
    pub conn: u64,
}

/// What a [`SessionRequest`] asks.
#[derive(Clone, Debug)]
pub enum SessionAsk {
    /// Runs a command you typed after `!`, in the session, or in a fresh
    /// shell when `fresh` says why, or when the session can't take it.
    Run {
        /// The command line.
        command: String,
        /// Whether its block starts private.
        private: bool,
        /// Why it goes to a fresh shell whatever the session does.
        fresh: Option<FreshReason>,
    },
    /// Moves the session to a folder, or opens it there.
    Cd {
        /// The folder, as the device writes it.
        path: String,
    },
}

/// What came of a [`SessionRequest`].
#[derive(Clone, Debug)]
pub enum SessionResult {
    /// The command went into the session as block `block`. What happens
    /// to the block comes as [`RunNote::Block`]s.
    Wrote {
        /// The session's terminal.
        terminal: TerminalId,
        /// The block's ID in the session.
        block: u32,
        /// The command line, as it went in.
        command: String,
        /// The session's folder when it went in, as the device writes it.
        cwd: String,
        /// Whether the block starts private.
        private: bool,
        /// When it went in.
        at: Timestamp,
    },
    /// The session opened for a folder you picked.
    Opened(Terminal),
    /// The command runs in a fresh shell, which comes as
    /// [`RunNote::Opened`] with the same key.
    Fresh(FreshReason),
    /// Why it didn't happen.
    Refused(String),
}

/// What happened to one of a session's blocks.
#[derive(Clone, Debug)]
pub enum BlockChange {
    /// The shell marked the command's start.
    Started {
        /// Where the shell's mark for the start begins, with the output
        /// right after it.
        start: u64,
        /// When the holder read the mark.
        at: Timestamp,
        /// The folder the shell said it ran in, as the device writes it.
        cwd: Option<String>,
    },
    /// It ended, or the session lost track of it.
    Ended {
        /// Its exit code, when the shell said one.
        exit: Option<i32>,
        /// When it ended.
        at: Timestamp,
        /// The folder the shell was in after it, when that changed.
        cwd_after: Option<String>,
        /// What it wrote, rendered.
        output: Transcript,
        /// How many passwords you sent at prompts that hid what you
        /// typed.
        secret_prompts: u32,
        /// Whether your Stop or Force stop ended it.
        stopped: bool,
    },
}

/// What `maxd` keeps about a session in its terminal's note.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct SessionNote {
    /// How many blocks it has had, which is the last block's ID.
    pub(super) blocks: u32,
    /// Whether its shell is fish, which quotes differently.
    #[serde(default)]
    pub(super) fish: bool,
    /// The block that runs, until it's kept.
    #[serde(default)]
    pub(super) current: Option<BlockNote>,
    /// Whether its shell has shown a marked prompt.
    #[serde(default)]
    pub(super) prompted: bool,
}

/// A block that runs, as its note keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct BlockNote {
    pub(super) id: u32,
    pub(super) command: String,
    /// Where the prompt it went in at starts.
    pub(super) after: u64,
    pub(super) written_at: Timestamp,
    /// Where the shell's mark for its start begins, once marked.
    #[serde(default)]
    pub(super) start: Option<u64>,
    /// The session's folder when it went in, as the device writes it.
    pub(super) cwd: String,
    /// Whether it has ended, its output still to cut.
    #[serde(default)]
    pub(super) done: bool,
    #[serde(default)]
    pub(super) exit: Option<i32>,
}

/// What you typed into a session's terminal, as far as whether a command
/// may go in: never the bytes, only whether they sent a line and whether
/// some sit at the prompt or wait for it.
///
/// Keys you type while something runs wait in the terminal for the shell,
/// unless what runs reads them, and the shell takes them at its next
/// prompt: a line you ended runs then, and the rest sits at the prompt.
#[derive(Debug, Default)]
pub(super) struct Typed {
    /// You sent a line, or Ctrl-C or Ctrl-D, since the last prompt.
    sent: bool,
    /// Something you typed may sit at the prompt, or wait for it.
    line: bool,
    /// Lines you sent while the shell wasn't reading, which its next
    /// prompts may read.
    ahead: u32,
    /// When the shell showed a prompt that may read a line you sent
    /// ahead, until a command starts.
    queued_at: Option<Instant>,
}

impl Typed {
    /// Takes in keys you typed. `busy` says whether something ran, so the
    /// shell wasn't reading them.
    pub(super) fn keys(&mut self, data: &[u8], busy: bool) {
        if replies_only(data) {
            return;
        }
        let mut data = data;
        // A line you sent before has yet to reach a prompt, so the shell
        // isn't reading either.
        let ahead = busy || self.sent;
        if ahead && let Some(at) = data.iter().rposition(|byte| *byte == 0x03) {
            // Ctrl-C while something runs: the terminal drops what waits
            // for the shell.
            self.sent = true;
            self.line = false;
            self.ahead = 0;
            data = &data[at + 1..];
        }
        for byte in data {
            if matches!(byte, b'\r' | b'\n' | 0x03 | 0x04) {
                if busy || self.sent {
                    self.ahead = self.ahead.saturating_add(1);
                }
                self.sent = true;
                self.line = false;
            } else {
                self.line = true;
            }
        }
    }

    /// Notes that the shell showed a prompt, so what you sent has run. It
    /// may read a line you sent ahead next.
    pub(super) fn prompt(&mut self) {
        self.sent = false;
        if self.queued_at.is_some_and(|at| at.elapsed() >= AHEAD_WAIT) {
            // The last prompt started nothing in time: what ran read the
            // lines you sent.
            self.ahead = 0;
        }
        self.queued_at = None;
        if self.ahead > 0 {
            self.ahead -= 1;
            self.queued_at = Some(Instant::now());
        }
    }

    /// Notes that the shell started a command.
    pub(super) fn started(&mut self) {
        self.queued_at = None;
    }

    /// Whether the shell may start a line you sent ahead any moment.
    fn queued(&self) -> bool {
        self.queued_at.is_some_and(|at| at.elapsed() < AHEAD_WAIT)
    }
}

/// Whether `data` holds only what a terminal answers on its own, such as
/// where its cursor is or its colors, rather than keys.
fn replies_only(data: &[u8]) -> bool {
    let mut rest = data;
    while !rest.is_empty() {
        match reply_len(rest) {
            Some(len) => rest = &rest[len..],
            None => return false,
        }
    }
    true
}

/// The length of the terminal's answer that `data` starts with, if it
/// starts with one.
fn reply_len(data: &[u8]) -> Option<usize> {
    let [0x1b, kind, body @ ..] = data else {
        return None;
    };
    match kind {
        b'[' => match *body.first()? {
            b'I' | b'O' => Some(3),
            b'?' | b'>' | b'<' | b'=' => {
                let end = body.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
                Some(2 + end + 1)
            }
            b'0'..=b'9' | b';' => {
                let end = body
                    .iter()
                    .position(|byte| !(byte.is_ascii_digit() || *byte == b';'))?;
                (body[end] == b'R').then_some(2 + end + 1)
            }
            _ => None,
        },
        b']' | b'P' | b'_' | b'^' | b'X' => {
            for (at, byte) in body.iter().enumerate() {
                if *byte == 0x07 {
                    return Some(2 + at + 1);
                }
                if *byte == 0x1b && body.get(at + 1) == Some(&b'\\') {
                    return Some(2 + at + 2);
                }
            }
            None
        }
        _ => None,
    }
}

/// Why a session can't take a command now.
#[derive(Clone, Debug, PartialEq, Eq)]
enum NotNow {
    IntegrationOff,
    NoMarks,
    NoFirstPrompt,
    /// Something runs, the command when it's a block.
    Busy(Option<String>),
    Typed,
    Gone,
    Failed(String),
}

impl NotNow {
    /// Why a command runs in a fresh shell instead, or why it doesn't run.
    fn fresh(self) -> Result<FreshReason, String> {
        match self {
            Self::IntegrationOff => Ok(FreshReason::IntegrationOff),
            Self::NoMarks | Self::NoFirstPrompt | Self::Gone => Ok(FreshReason::NoMarks),
            Self::Busy(_) | Self::Typed => Ok(FreshReason::Busy),
            Self::Failed(reason) => Err(reason),
        }
    }

    /// What a request that needs the session reads.
    fn reason(self, device: &str) -> String {
        match self {
            Self::IntegrationOff => {
                format!("Sessions need shell integration, which is off on {device}.")
            }
            Self::NoMarks => NO_MARKS.to_owned(),
            Self::NoFirstPrompt => NO_FIRST_PROMPT.to_owned(),
            Self::Busy(Some(command)) => {
                let mut named: String = command.chars().take(NAMED_CHARS).collect();
                if command.chars().count() > NAMED_CHARS {
                    named.push('…');
                }
                format!("The session is busy. Wait for {named} to finish, or stop it.")
            }
            Self::Busy(None) => BUSY_UNNAMED.to_owned(),
            Self::Typed => TYPED.to_owned(),
            Self::Gone => SESSION_ENDED.to_owned(),
            Self::Failed(reason) => reason,
        }
    }
}

/// Where a session stands for a command that waits for it.
enum Look {
    Ready,
    /// It hasn't shown its first prompt since `maxd` began following it
    /// then.
    FirstPrompt(Instant),
    /// Something runs, which may end in a moment.
    Busy(NotNow),
    /// A block ends, its shell back at its prompt.
    Ending(NotNow),
    /// It can't take one.
    Stop(NotNow),
}

/// What a request comes to.
enum Step {
    Answer(SessionResult),
    Fresh {
        reason: FreshReason,
        cwd: PathBuf,
        command: String,
        private: bool,
    },
    /// The answer went to the manager already.
    Noted,
}

impl Terminals {
    /// Does what `request` asks of an agent's session, one request per
    /// agent at a time. The answer goes to the manager as
    /// [`RunNote::Session`].
    pub(super) fn serve_session(&self, request: SessionRequest) {
        let this = self.clone();
        tokio::spawn(async move {
            let lock = this.agent_lock(request.agent);
            let _one_at_a_time = lock.lock().await;
            let (key, agent) = (request.key, request.agent);
            match this.session_step(&request).await {
                Step::Answer(result) => this.note(RunNote::Session { key, agent, result }),
                Step::Fresh {
                    reason,
                    cwd,
                    command,
                    private,
                } => {
                    this.note(RunNote::Session {
                        key,
                        agent,
                        result: SessionResult::Fresh(reason),
                    });
                    this.start(OpenRequest {
                        key,
                        agent,
                        kind: TerminalKind::Run { command, private },
                        cwd,
                        cols: request.cols,
                        rows: request.rows,
                        opener: request.opener,
                        conn: request.conn,
                    });
                }
                Step::Noted => {}
            }
        });
    }

    async fn session_step(&self, request: &SessionRequest) -> Step {
        let live = self.live_session(request.agent);
        let here = (live.as_ref())
            .map(|(_, cwd)| expand(cwd))
            .filter(|cwd| cwd.is_dir())
            .unwrap_or_else(|| request.cwd.clone());
        let target = match &request.ask {
            SessionAsk::Run {
                command,
                private,
                fresh: Some(reason),
            } => {
                return Step::Fresh {
                    reason: *reason,
                    cwd: here,
                    command: command.clone(),
                    private: *private,
                };
            }
            SessionAsk::Run { .. } => None,
            SessionAsk::Cd { path } => match resolve(path, &here) {
                Ok(target) => Some(target),
                Err(reason) => return Step::Answer(SessionResult::Refused(reason)),
            },
        };
        let terminal = match live {
            Some((terminal, _)) => terminal,
            None => {
                let cwd = target.clone().unwrap_or_else(|| request.cwd.clone());
                match self.open_session(request, cwd).await {
                    Ok(entity) if target.is_some() => {
                        return Step::Answer(SessionResult::Opened(entity));
                    }
                    Ok(entity) => entity.id,
                    Err(why) => return self.not_now(request, here, why),
                }
            }
        };
        if let Err(why) = self.wait_idle(terminal).await {
            return self.not_now(request, here, why);
        }
        let (line, command, private) = match (&request.ask, &target) {
            (
                SessionAsk::Run {
                    command, private, ..
                },
                _,
            ) => (command.clone(), command.clone(), *private),
            (SessionAsk::Cd { .. }, Some(target)) => {
                let fish = self.with_session(terminal, |session| session.fish);
                let line = cd_line(target, fish.unwrap_or(false));
                (line.clone(), line, false)
            }
            (SessionAsk::Cd { .. }, None) => {
                return Step::Answer(SessionResult::Refused(NO_FOLDER_GIVEN.to_owned()));
            }
        };
        match self.write_block(terminal, request, &line, command, private) {
            Ok(step) => step,
            Err(why) => self.not_now(request, here, why),
        }
    }

    /// What a request the session can't take comes to: a fresh shell for
    /// a command, a refusal for a folder.
    fn not_now(&self, request: &SessionRequest, here: PathBuf, why: NotNow) -> Step {
        match &request.ask {
            SessionAsk::Run {
                command, private, ..
            } => match why.fresh() {
                Ok(reason) => Step::Fresh {
                    reason,
                    cwd: here,
                    command: command.clone(),
                    private: *private,
                },
                Err(reason) => Step::Answer(SessionResult::Refused(reason)),
            },
            SessionAsk::Cd { .. } => {
                let device = self.inner.hub.device().name.clone();
                Step::Answer(SessionResult::Refused(why.reason(&device)))
            }
        }
    }

    /// The agent's session that runs, with its folder as the device
    /// writes it.
    fn live_session(&self, agent: AgentId) -> Option<(TerminalId, String)> {
        (self.lock().iter())
            .find(|(_, open)| open.is_session_of(agent) && open.running() && !open.closing)
            .map(|(terminal, open)| (*terminal, open.entity.cwd.clone()))
    }

    /// Starts the agent's session in `cwd`.
    async fn open_session(
        &self,
        request: &SessionRequest,
        cwd: PathBuf,
    ) -> Result<Terminal, NotNow> {
        if !self.integration_on() {
            return Err(NotNow::IntegrationOff);
        }
        let terminal = TerminalId::generate();
        let kind = TerminalKind::Session;
        let mut launch = self.launch(terminal, &kind, cwd, request.cols, request.rows);
        if !self.integrate(&mut launch) {
            return Err(NotNow::NoMarks);
        }
        let fish = launch
            .program
            .file_name()
            .is_some_and(|name| name == "fish");
        self.finish_ended_sessions(request.agent);
        let session = SessionNote {
            fish,
            ..SessionNote::default()
        };
        let (entity, events) = self
            .spawn_launch(
                &launch,
                request.agent,
                kind,
                request.opener,
                request.conn,
                Some(session),
            )
            .await
            .map_err(NotNow::Failed)?;
        tokio::spawn(self.clone().follow(entity.id, events));
        Ok(entity)
    }

    /// Waits for the session to wait at its prompt with nothing typed
    /// there. Something that runs gets a moment to end.
    async fn wait_idle(&self, terminal: TerminalId) -> Result<(), NotNow> {
        let began = Instant::now();
        loop {
            match self.idle_state(terminal) {
                Look::Ready => return Ok(()),
                Look::Stop(why) => return Err(why),
                Look::FirstPrompt(followed) => {
                    if followed.elapsed() >= FIRST_PROMPT_WAIT {
                        self.give_up(terminal);
                        return Err(NotNow::NoFirstPrompt);
                    }
                }
                Look::Busy(why) if began.elapsed() >= SETTLE_WAIT => return Err(why),
                Look::Ending(why) if began.elapsed() >= ENDING_WAIT => return Err(why),
                Look::Busy(_) | Look::Ending(_) => {}
            }
            tokio::time::sleep(LOOK).await;
        }
    }

    fn idle_state(&self, terminal: TerminalId) -> Look {
        (self.lock().get(&terminal)).map_or(Look::Stop(NotNow::Gone), Open::look)
    }

    /// Ends a session whose shell never showed a marked prompt.
    fn give_up(&self, terminal: TerminalId) {
        let mut opens = self.lock();
        if let Some(open) = opens.get_mut(&terminal) {
            if let Some(control) = &open.control {
                control.kill(Duration::ZERO);
            }
            open.closing = true;
        }
    }

    /// Types `line` into the session as a new block, and follows it. It
    /// looks once more whether the session takes it under the same lock,
    /// so nothing you type comes in between.
    fn write_block(
        &self,
        terminal: TerminalId,
        request: &SessionRequest,
        line: &str,
        command: String,
        private: bool,
    ) -> Result<Step, NotNow> {
        let mut opens = self.lock();
        let open = opens.get_mut(&terminal).ok_or(NotNow::Gone)?;
        match open.look() {
            Look::Ready => {}
            Look::FirstPrompt(_) => return Err(NotNow::Busy(None)),
            Look::Busy(why) | Look::Ending(why) | Look::Stop(why) => return Err(why),
        }
        let control = open.control.clone().ok_or(NotNow::Busy(None))?;
        let after = (open.commands.list().back())
            .and_then(|last| last.prompt)
            .unwrap_or(0);
        let cwd = open.entity.cwd.clone();
        let session = open.note.session.as_mut().ok_or(NotNow::Gone)?;
        // The line takes the prompt: what you sent before has run, or what
        // ran read it.
        open.keys = Typed::default();
        open.stopped_at = None;
        session.blocks += 1;
        let at = Timestamp::now();
        let block = BlockNote {
            id: session.blocks,
            command: command.clone(),
            after,
            written_at: at,
            start: None,
            cwd: cwd.clone(),
            done: false,
            exit: None,
        };
        session.current = Some(block.clone());
        self.save(terminal, &open.note);
        let (secrets, secrets_rx) = mpsc::unbounded_channel();
        open.block = Some((block.id, secrets));
        let (cols, rows) = (open.entity.cols, open.entity.rows);
        // The manager hears of the block before anything about it.
        self.note(RunNote::Session {
            key: request.key,
            agent: request.agent,
            result: SessionResult::Wrote {
                terminal,
                block: block.id,
                command,
                cwd,
                private,
                at,
            },
        });
        control.input(format!("{line}\r").into_bytes());
        drop(opens);
        tokio::spawn(self.clone().follow_block(
            terminal,
            request.agent,
            block,
            secrets_rx,
            cols,
            rows,
        ));
        Ok(Step::Noted)
    }

    /// Follows a block that went in again, after a restart.
    pub(super) fn resume_block(&self, terminal: TerminalId, entity: &Terminal, block: BlockNote) {
        let (secrets, secrets_rx) = mpsc::unbounded_channel();
        if let Some(open) = self.lock().get_mut(&terminal) {
            open.block = Some((block.id, secrets));
        }
        tokio::spawn(self.clone().follow_block(
            terminal,
            entity.agent,
            block,
            secrets_rx,
            entity.cols,
            entity.rows,
        ));
    }

    /// Cuts a block's output from the session's terminal, and tells the
    /// manager when the command starts and how it ended.
    async fn follow_block(
        self,
        terminal: TerminalId,
        agent: AgentId,
        block: BlockNote,
        mut secrets: mpsc::UnboundedReceiver<()>,
        cols: u16,
        rows: u16,
    ) {
        let mut cutter = Cutter::new(&block, cols, rows);
        let from = block.start.unwrap_or(block.after);
        let mut tracked: Option<String> = None;
        let mut started_in: Option<String> = None;
        match self.inner.holds.connect_marked(terminal, Some(from)).await {
            Ok(Reached::Live(link)) => {
                let TermLink {
                    hello,
                    mut events,
                    control,
                } = link;
                // The link stays up while this holds its control.
                let _control = control;
                tracked = hello.cwd;
                let mut timer: Option<tokio::time::Instant> = None;
                // When to look whether the shell waits for more of the line.
                // An `exec` line never comes here: the manager sends it
                // fresh.
                let mut more = cutter
                    .waiting_for_more()
                    .then(|| tokio::time::Instant::now() + CONTINUATION_WAIT);
                let mut secrets_open = true;
                loop {
                    tokio::select! {
                        event = events.recv() => {
                            let Some(event) = event else { break };
                            let now = tokio::time::Instant::now();
                            match event {
                                TermEvent::Output { offset, data } => cutter.output(offset, &data),
                                TermEvent::Password(false) => cutter.password_off(),
                                TermEvent::Cwd(cwd) => tracked = Some(cwd),
                                TermEvent::Mark { offset, at, mark } => {
                                    match cutter.mark(offset, at, &mark) {
                                        Cut::None => {}
                                        Cut::Started { start, at, cwd } => {
                                            timer = None;
                                            started_in.clone_from(&cwd);
                                            self.keep_block(terminal, block.id, |kept| {
                                                kept.start = Some(start);
                                            });
                                            self.note(RunNote::Block {
                                                agent,
                                                terminal,
                                                block: block.id,
                                                change: BlockChange::Started {
                                                    start,
                                                    at,
                                                    cwd: cwd.map(|cwd| shown(Path::new(&cwd))),
                                                },
                                            });
                                        }
                                        Cut::Grace => {
                                            let soon = now + UNMARKED_GRACE;
                                            timer = Some(timer.map_or(soon, |when| when.min(soon)));
                                        }
                                        Cut::Ended { prompt } => {
                                            let exit = cutter.exit();
                                            self.keep_block(terminal, block.id, |kept| {
                                                kept.done = true;
                                                kept.exit = exit;
                                            });
                                            timer = Some(now + if prompt { SETTLE } else { PROMPT_WAIT });
                                        }
                                        Cut::Prompt => {
                                            let soon = now + SETTLE;
                                            timer = Some(timer.map_or(soon, |when| when.min(soon)));
                                        }
                                        Cut::Next => break,
                                    }
                                }
                                TermEvent::Exit(_) => break,
                                TermEvent::Password(true)
                                | TermEvent::Busy(_)
                                | TermEvent::Modes(_)
                                | TermEvent::Title(_) => {}
                            }
                        }
                        secret = secrets.recv(), if secrets_open => match secret {
                            Some(()) => cutter.secret(),
                            None => secrets_open = false,
                        },
                        () = until(more) => {
                            more = None;
                            if !cutter.waiting_for_more() {
                                continue;
                            }
                            let now = tokio::time::Instant::now();
                            if self.cancel_line(terminal, block.id, block.after) {
                                cutter.cancel();
                                let soon = now + PROMPT_WAIT;
                                timer = Some(timer.map_or(soon, |when| when.min(soon)));
                            } else {
                                more = Some(now + CONTINUATION_WAIT);
                            }
                        }
                        () = until(timer) => {
                            if cutter.grace_over() {
                                let exit = cutter.exit();
                                self.keep_block(terminal, block.id, |kept| {
                                    kept.done = true;
                                    kept.exit = exit;
                                });
                                timer = Some(tokio::time::Instant::now() + SETTLE);
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
            Ok(Reached::Ended(_) | Reached::Missing) => {}
            Err(err) => warn!(%err, %terminal, "couldn't follow a session's command"),
        }
        let (exit, at, output, secret_prompts) = cutter.finish();
        let before = started_in.map_or_else(|| block.cwd.clone(), |cwd| shown(Path::new(&cwd)));
        let cwd_after = tracked
            .map(|cwd| shown(Path::new(&cwd)))
            .filter(|after| *after != before);
        let mut opens = self.lock();
        let mut stopped = false;
        if let Some(open) = opens.get_mut(&terminal) {
            if open.block.as_ref().is_some_and(|(id, _)| *id == block.id) {
                open.block = None;
                stopped = open.stopped();
            }
            let session = open.note.session.as_mut();
            if let Some(session) = session
                && session
                    .current
                    .as_ref()
                    .is_some_and(|kept| kept.id == block.id)
            {
                session.current = None;
                self.save(terminal, &open.note);
            }
        }
        // Under the lock, so a block that ends as the shell exits goes to
        // the manager once.
        self.note(RunNote::Block {
            agent,
            terminal,
            block: block.id,
            change: BlockChange::Ended {
                exit,
                at,
                cwd_after,
                output,
                secret_prompts,
                stopped,
            },
        });
        drop(opens);
    }

    /// Changes the block a session runs, when it's still `id`, and keeps
    /// the change.
    fn keep_block(&self, terminal: TerminalId, id: u32, change: impl FnOnce(&mut BlockNote)) {
        let mut opens = self.lock();
        let Some(open) = opens.get_mut(&terminal) else {
            return;
        };
        let current = (open.note.session.as_mut()).and_then(|session| session.current.as_mut());
        match current {
            Some(block) if block.id == id => change(block),
            _ => return,
        }
        self.save(terminal, &open.note);
    }

    /// Sends Ctrl-C to a session whose shell waits for more of block `id`'s
    /// line, as for a closing quote: the block runs, the shell holds the
    /// foreground and has marked neither a start nor a prompt since the
    /// line went in after `after`. Returns whether it did.
    fn cancel_line(&self, terminal: TerminalId, id: u32, after: u64) -> bool {
        let opens = self.lock();
        let Some(open) = opens.get(&terminal) else {
            return false;
        };
        let ours = (open.block.as_ref()).is_some_and(|(block, _)| *block == id);
        let waits = (open.commands.list().back()).is_some_and(|last| {
            last.start.is_none() && last.prompt.is_none_or(|prompt| prompt <= after)
        });
        let shell = open.running() && !open.foreground && !open.entity.password_prompt;
        match &open.control {
            Some(control) if ours && waits && shell => {
                control.input(vec![0x03]);
                true
            }
            _ => false,
        }
    }

    /// Reads a session's note.
    fn with_session<T>(
        &self,
        terminal: TerminalId,
        read: impl FnOnce(&SessionNote) -> T,
    ) -> Option<T> {
        let opens = self.lock();
        (opens.get(&terminal))
            .and_then(|open| open.note.session.as_ref())
            .map(read)
    }

    /// Ends an agent's session: hangs up what runs in it, then kills it
    /// after a moment, or at once with `force`. Without `force`, one that
    /// still runs a command asks first. A session whose shell has ended
    /// goes.
    pub fn end_session(&self, agent: AgentId, force: bool) -> ActionOutcome {
        let mut opens = self.lock();
        let live = (opens.values_mut())
            .find(|open| open.is_session_of(agent) && open.running() && !open.closing);
        if let Some(open) = live {
            let pending = (open.note.session.as_ref())
                .and_then(|session| session.current.as_ref())
                .is_some_and(|block| !block.done);
            if !force && (open.entity.busy || pending) {
                return ActionOutcome::Refused {
                    reason: STILL_RUNNING.to_owned(),
                };
            }
            let Some(control) = &open.control else {
                return ActionOutcome::Refused {
                    reason: UNREACHABLE.to_owned(),
                };
            };
            control.kill(if force { Duration::ZERO } else { HANGUP_GRACE });
            open.closing = true;
            return ActionOutcome::Done;
        }
        drop(opens);
        self.finish_ended_sessions(agent);
        ActionOutcome::Done
    }

    /// Lets go of an agent's sessions whose shells have ended.
    fn finish_ended_sessions(&self, agent: AgentId) {
        let ended: Vec<TerminalId> = (self.lock().iter())
            .filter(|(_, open)| open.is_session_of(agent) && !open.running())
            .map(|(terminal, _)| *terminal)
            .collect();
        for terminal in ended {
            self.finish(terminal);
        }
    }

    /// The lock that lets one request at a time work on an agent's
    /// session.
    fn agent_lock(&self, agent: AgentId) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = (self.inner.locks.lock()).unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(locks.entry(agent).or_default())
    }
}

impl Open {
    fn is_session_of(&self, agent: AgentId) -> bool {
        self.note.agent == agent && matches!(self.note.kind, TerminalKind::Session)
    }

    /// Where the session stands for a command that waits for it.
    fn look(&self) -> Look {
        if !self.running() || self.closing {
            return Look::Stop(NotNow::Gone);
        }
        if self.control.is_none() {
            return Look::Busy(NotNow::Busy(None));
        }
        if !self.entity.marks {
            return Look::Stop(NotNow::NoMarks);
        }
        self.shell_look()
    }

    /// Where the session's shell stands, from its marks and what you
    /// typed.
    fn shell_look(&self) -> Look {
        let session = self.note.session.as_ref();
        let last = self.commands.list().back();
        if let Some(block) =
            (session.and_then(|session| session.current.as_ref())).filter(|block| !block.done)
        {
            let busy = NotNow::Busy(Some(block.command.clone()));
            // The shell showed a new prompt with nothing started: the
            // block ends in a moment, with no code.
            let ending = last.is_some_and(|last| {
                last.prompt.is_some_and(|prompt| prompt > block.after) && last.start.is_none()
            });
            return if ending {
                Look::Ending(busy)
            } else {
                Look::Busy(busy)
            };
        }
        let Some(last) = last else {
            // No marks to go by. A new shell has yet to show its first
            // prompt. One that did may run something whose marks the
            // holder no longer keeps, or they come again after a restart.
            let new = session.is_some_and(|session| session.blocks == 0 && !session.prompted);
            return if new && !self.foreground {
                Look::FirstPrompt(self.followed)
            } else {
                Look::Busy(NotNow::Busy(None))
            };
        };
        let at_prompt = last.prompt.is_some() && last.start.is_none();
        if !at_prompt || self.foreground || self.keys.sent {
            return Look::Busy(NotNow::Busy(None));
        }
        if self.keys.line {
            return Look::Stop(NotNow::Typed);
        }
        if self.keys.queued() {
            // The shell may start a line you sent while something ran.
            return Look::Ending(NotNow::Busy(None));
        }
        Look::Ready
    }

    /// Takes in a mark the shell made. Returns whether the terminal
    /// changed, and whether its note did.
    pub(super) fn marked(&mut self, offset: u64, at: Timestamp, mark: &Mark) -> (bool, bool) {
        let mut keep = false;
        match mark {
            Mark::Prompt => {
                self.keys.prompt();
                if let Some(session) = self.note.session.as_mut() {
                    keep = !session.prompted;
                    session.prompted = true;
                }
            }
            Mark::Command { .. } => self.keys.started(),
            Mark::Input | Mark::Done { .. } => {}
        }
        self.commands.mark(offset, at, mark);
        (self.rebusy(), keep)
    }

    /// Takes in keys you typed into the session, as far as what they leave
    /// at its prompt, or that a password went when one waited. Keys typed
    /// while something runs count too: the shell reads them next.
    pub(super) fn heard_keys(&mut self, data: &[u8]) {
        if self.entity.password_prompt {
            // The secret goes at Enter. A prompt that reads each key gets
            // the rest before it.
            let sent = data.iter().any(|&byte| matches!(byte, b'\r' | b'\n'));
            if let Some((_, secrets)) = &self.block
                && sent
            {
                let _ = secrets.send(());
            }
            return;
        }
        let busy = self.entity.busy || self.block.is_some();
        self.keys.keys(data, busy);
    }
}

/// `maxd`'s home folder.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// A folder the device writes with `~`, as a path.
pub(super) fn expand(shown: &str) -> PathBuf {
    match (shown, home()) {
        ("~", Some(home)) => home,
        (shown, Some(home)) if shown.starts_with("~/") => home.join(&shown[2..]),
        (shown, _) => PathBuf::from(shown),
    }
}

/// Waits until `when`, or forever without one.
async fn until(when: Option<tokio::time::Instant>) {
    match when {
        Some(when) => tokio::time::sleep_until(when).await,
        None => std::future::pending().await,
    }
}

/// The folder you asked a session to go to, from where it is.
fn resolve(path: &str, here: &Path) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err(NO_FOLDER_GIVEN.to_owned());
    }
    if path.chars().any(char::is_control) {
        return Err(CONTROL_IN_FOLDER.to_owned());
    }
    let expanded = if path == "~" || path.starts_with("~/") {
        expand(path)
    } else {
        PathBuf::from(path)
    };
    let target = tidy(&if expanded.is_absolute() {
        expanded
    } else {
        here.join(expanded)
    });
    if !target.is_dir() {
        return Err(format!("There's no folder at {path}."));
    }
    Ok(target)
}

/// `path` with `.` and `..` worked out by name, as `cd` does.
fn tidy(path: &Path) -> PathBuf {
    let mut tidied = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                tidied.pop();
            }
            other => tidied.push(other),
        }
    }
    tidied
}

/// The `cd` line that takes a shell to `path`, quoted for fish or for a
/// POSIX shell.
fn cd_line(path: &Path, fish: bool) -> String {
    if let Some(home) = home()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "cd -- ~".to_owned();
        }
        return format!("cd -- ~/{}", quote(&rest.to_string_lossy(), fish));
    }
    format!("cd -- {}", quote(&path.to_string_lossy(), fish))
}

/// `word` as one word to the shell.
fn quote(word: &str, fish: bool) -> String {
    let plain = !word.is_empty()
        && (word.chars()).all(|c| c.is_ascii_alphanumeric() || "-_./+@%,:=".contains(c));
    if plain {
        word.to_owned()
    } else if fish {
        format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// What a mark meant for the block.
#[derive(Debug, PartialEq, Eq)]
enum Cut {
    None,
    /// The command started.
    Started {
        start: u64,
        at: Timestamp,
        cwd: Option<String>,
    },
    /// The shell showed a prompt with no command started.
    Grace,
    /// The command ended, at a prompt when `prompt` says so.
    Ended {
        prompt: bool,
    },
    /// The prompt after the command.
    Prompt,
    /// Another command started.
    Next,
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    /// Before the shell marks the command's start.
    Waiting,
    Running {
        start: u64,
    },
    Ended {
        exit: Option<i32>,
        end: u64,
        at: Timestamp,
    },
}

/// Cuts one block's output from a session terminal's output and marks.
struct Cutter {
    /// Where the prompt it went in at starts.
    after: u64,
    phase: Phase,
    /// Output not yet in the emulator, from `buf_at` to `next`.
    buf: Vec<u8>,
    buf_at: u64,
    next: u64,
    cols: u16,
    rows: u16,
    emulator: Box<dyn Emulator>,
    /// Whether the start of the block's output is gone.
    cut: bool,
    /// Whether the password prompt that shows has its note.
    noted: bool,
    secrets: u32,
    /// Where the shell's prompt marked the start of what was typed.
    input_at: Option<u64>,
    /// Where a prompt after the line, with no command started, starts.
    prompt_at: Option<u64>,
    /// Whether `maxd` cancelled the line, as the shell waited for more.
    cancelled: bool,
}

impl Cutter {
    fn new(block: &BlockNote, cols: u16, rows: u16) -> Self {
        let (phase, from) = match block.start {
            Some(start) => (Phase::Running { start }, start),
            None => (Phase::Waiting, block.after),
        };
        let (cols, rows) = (cols.max(1), rows.max(1));
        Self {
            after: block.after,
            phase,
            buf: Vec::new(),
            buf_at: from,
            next: from,
            cols,
            rows,
            emulator: maxhq_term::emulator(cols, rows),
            cut: false,
            noted: false,
            secrets: 0,
            input_at: None,
            prompt_at: None,
            cancelled: false,
        }
    }

    fn output(&mut self, offset: u64, data: &[u8]) {
        let end = offset + data.len() as u64;
        if end <= self.next {
            return;
        }
        let (offset, data) = if offset < self.next {
            let seen = usize::try_from(self.next - offset).unwrap_or(data.len());
            (self.next, &data[seen.min(data.len())..])
        } else {
            (offset, data)
        };
        if let Phase::Ended { .. } = self.phase {
            self.next = end;
            return;
        }
        if offset > self.next {
            // The link fell behind, or the holder no longer had what came
            // after where it asked.
            self.buf.clear();
            if let Phase::Running { .. } = self.phase {
                self.emulator = maxhq_term::emulator(self.cols, self.rows);
                self.cut = true;
            }
        }
        if self.buf.is_empty() {
            self.buf_at = offset;
        }
        self.buf.extend_from_slice(data);
        self.next = end;
        self.noted = false;
        if self.buf.len() > HOLD_MAX {
            let gone = self.buf.len() - HOLD_KEEP;
            // Before the start, what goes is the prompt and the line, or
            // what a command the shell marked no start for wrote, whose
            // start [`Cutter::skip_to`] then finds gone.
            if matches!(self.phase, Phase::Running { .. }) {
                self.emulator.feed(&self.buf[..gone]);
            }
            self.buf.drain(..gone);
            self.buf_at += gone as u64;
        }
    }

    fn mark(&mut self, offset: u64, at: Timestamp, mark: &Mark) -> Cut {
        match (&self.phase, mark) {
            (Phase::Waiting, Mark::Command { cwd }) if offset > self.after => {
                self.skip_to(offset);
                self.phase = Phase::Running { start: offset };
                self.cancelled = false;
                Cut::Started {
                    start: offset,
                    at,
                    cwd: cwd.clone(),
                }
            }
            (Phase::Waiting, Mark::Prompt) if offset > self.after => {
                self.prompt_at.get_or_insert(offset);
                Cut::Grace
            }
            (Phase::Waiting, Mark::Input) if offset >= self.after => {
                self.input_at.get_or_insert(offset);
                Cut::None
            }
            (Phase::Waiting, Mark::Done { exit }) if offset > self.after => {
                self.end_unmarked(offset, *exit, at);
                Cut::Ended { prompt: false }
            }
            (&Phase::Running { start }, Mark::Done { exit }) if offset >= start => {
                self.feed_to(offset);
                self.phase = Phase::Ended {
                    exit: *exit,
                    end: offset,
                    at,
                };
                Cut::Ended { prompt: false }
            }
            (&Phase::Running { start }, Mark::Prompt) if offset > start => {
                self.feed_to(offset);
                self.phase = Phase::Ended {
                    exit: None,
                    end: offset,
                    at,
                };
                Cut::Ended { prompt: true }
            }
            (&Phase::Running { start }, Mark::Command { .. }) if offset > start => {
                self.feed_to(offset);
                self.phase = Phase::Ended {
                    exit: None,
                    end: offset,
                    at,
                };
                Cut::Next
            }
            (&Phase::Ended { end, .. }, Mark::Prompt) if offset >= end => Cut::Prompt,
            (&Phase::Ended { end, .. }, Mark::Command { .. }) if offset > end => Cut::Next,
            _ => Cut::None,
        }
    }

    /// Ends a line the shell marked no start for, at `end`. Its output
    /// starts after the line it echoed.
    fn end_unmarked(&mut self, end: u64, exit: Option<i32>, at: Timestamp) {
        match self.input_at.filter(|input| *input >= self.buf_at) {
            Some(input) => {
                let from = usize::try_from(input - self.buf_at).unwrap_or(usize::MAX);
                let newline = (self.buf.get(from..).unwrap_or_default().iter())
                    .position(|byte| *byte == b'\n');
                match newline {
                    Some(newline) => self.skip_to(input + newline as u64 + 1),
                    None => self.skip_to(end),
                }
            }
            None => self.cut = true,
        }
        self.feed_to(end);
        self.phase = Phase::Ended { exit, end, at };
    }

    /// Ends the block with no code once a prompt came with no start in
    /// time. Returns whether it did.
    fn grace_over(&mut self) -> bool {
        match (&self.phase, self.prompt_at) {
            (Phase::Waiting, Some(prompt)) => {
                self.end_unmarked(prompt, None, Timestamp::now());
                true
            }
            _ => false,
        }
    }

    /// Whether the line went in with neither a start nor a prompt since.
    fn waiting_for_more(&self) -> bool {
        self.phase == Phase::Waiting && self.prompt_at.is_none() && !self.cancelled
    }

    /// Notes that `maxd` cancelled the line.
    fn cancel(&mut self) {
        self.cancelled = true;
    }

    fn password_off(&mut self) {
        self.noted = false;
    }

    /// Notes a password you sent, once per prompt.
    fn secret(&mut self) {
        if !matches!(self.phase, Phase::Running { .. }) || self.noted {
            return;
        }
        self.feed_to(self.next);
        self.emulator.note(maxhq_term::SECRET_SENT);
        self.secrets += 1;
        self.noted = true;
    }

    fn exit(&self) -> Option<i32> {
        match self.phase {
            Phase::Ended { exit, .. } => exit,
            _ => None,
        }
    }

    fn skip_to(&mut self, to: u64) {
        if to < self.buf_at {
            self.cut = true;
            return;
        }
        if to >= self.next {
            self.buf.clear();
            self.buf_at = to;
            self.next = to;
            return;
        }
        let skipped = usize::try_from(to - self.buf_at).unwrap_or(self.buf.len());
        self.buf.drain(..skipped.min(self.buf.len()));
        self.buf_at = to;
    }

    fn feed_to(&mut self, to: u64) {
        if to <= self.buf_at {
            return;
        }
        let fed = usize::try_from(to - self.buf_at)
            .unwrap_or(usize::MAX)
            .min(self.buf.len());
        self.emulator.feed(&self.buf[..fed]);
        self.buf.drain(..fed);
        self.buf_at += fed as u64;
    }

    /// The block's end: its code, when it ended, its output and how many
    /// passwords went. A block that never ended, as when its shell went,
    /// ends now with no code.
    fn finish(mut self) -> (Option<i32>, Timestamp, Transcript, u32) {
        let now = Timestamp::now();
        match self.phase {
            Phase::Running { .. } => {
                self.feed_to(self.next);
                self.phase = Phase::Ended {
                    exit: None,
                    end: self.next,
                    at: now,
                };
            }
            Phase::Waiting => self.end_unmarked(self.next, None, now),
            Phase::Ended { .. } => {}
        }
        let (exit, at) = match self.phase {
            Phase::Ended { exit, at, .. } => (exit, at),
            _ => (None, now),
        };
        if self.cancelled {
            let output = Transcript {
                text: CANCELLED.to_owned(),
                lines: 1,
                truncated: false,
            };
            return (None, at, output, self.secrets);
        }
        let mut output = self.emulator.transcript();
        output.truncated |= self.cut;
        (exit, at, output, self.secrets)
    }
}

#[cfg(test)]
mod tests;
