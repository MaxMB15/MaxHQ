// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the terminal tests share on every OS: starting and reaching a
//! terminal holder, and reading what a link tells.

use std::time::Duration;

use maxhq_hold::{
    Commands, Detach, Mark, Reached, TermEvent, TermExit, TermLaunch, TermLink, TermModes,
    Terminals,
};
use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::time::Timestamp;

use crate::common::{exe, Ctx, WAIT};

/// How long a holder stays after its program ends in these tests.
pub(crate) const LINGER: Duration = Duration::from_secs(3);

pub(crate) fn terminals(ctx: &Ctx) -> Terminals {
    Terminals::new(ctx.dirs.clone())
        .with_holder(exe(), vec!["__hold".into()])
        .with_detach(Detach::Setsid)
        .with_linger(LINGER)
}

pub(crate) async fn start(ctx: &Ctx, launch: &TermLaunch) -> TermLink {
    let terminals = terminals(ctx);
    terminals.spawn(launch).await.unwrap();
    live(&terminals, launch.terminal, None).await
}

pub(crate) async fn live(
    terminals: &Terminals,
    terminal: TerminalId,
    after: Option<u64>,
) -> TermLink {
    match terminals.connect(terminal, after).await.unwrap() {
        Reached::Live(link) => link,
        Reached::Ended(exit) => panic!("the terminal ended: {exit:?}"),
        Reached::Missing => panic!("the terminal has no folder"),
    }
}

/// What a link has told, with the output and the offset just past it.
pub(crate) struct Seen {
    pub(crate) bytes: Vec<u8>,
    pub(crate) end: u64,
    pub(crate) exit: Option<TermExit>,
    pub(crate) password: bool,
    pub(crate) busy: bool,
    pub(crate) title: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) marks: Vec<(u64, Mark)>,
    pub(crate) modes: TermModes,
}

/// The state with only the end of the output, which can run to megabytes.
impl std::fmt::Debug for Seen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tail = &self.bytes[self.bytes.len().saturating_sub(300)..];
        f.debug_struct("Seen")
            .field("tail", &String::from_utf8_lossy(tail))
            .field("end", &self.end)
            .field("exit", &self.exit)
            .field("password", &self.password)
            .field("busy", &self.busy)
            .field("title", &self.title)
            .field("cwd", &self.cwd)
            .field("marks", &self.marks)
            .field("modes", &self.modes)
            .finish()
    }
}

impl Seen {
    pub(crate) fn from(link: &TermLink) -> Self {
        Self {
            bytes: Vec::new(),
            end: link.hello.from,
            exit: None,
            password: link.hello.password,
            busy: link.hello.busy,
            title: link.hello.title.clone(),
            cwd: link.hello.cwd.clone(),
            marks: Vec::new(),
            modes: link
                .hello
                .modes
                .clone()
                .expect("a holder of this build says its modes"),
        }
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// Reads events until `done` says so, checking output comes with no
    /// gap unless `gaps`.
    pub(crate) async fn until(
        &mut self,
        link: &mut TermLink,
        gaps: bool,
        done: impl Fn(&Self) -> bool,
    ) {
        while !done(self) {
            let event = match tokio::time::timeout(WAIT, link.events.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => panic!("the events ended at {self:?}"),
                Err(_) => panic!("nothing new within {WAIT:?} at {self:?}"),
            };
            match event {
                TermEvent::Output { offset, data } => {
                    if !gaps {
                        assert_eq!(offset, self.end, "a gap before {:?}", self.text());
                    }
                    assert!(offset >= self.end, "output went back");
                    self.end = offset + data.len() as u64;
                    self.bytes.extend(data);
                }
                TermEvent::Password(on) => self.password = on,
                TermEvent::Busy(on) => self.busy = on,
                TermEvent::Title(title) => self.title = Some(title),
                TermEvent::Cwd(cwd) => self.cwd = Some(cwd),
                TermEvent::Mark { offset, mark, .. } => self.marks.push((offset, mark)),
                TermEvent::Modes(modes) => self.modes = modes,
                TermEvent::Exit(exit) => {
                    assert!(self.exit.is_none(), "two exits");
                    self.exit = Some(exit);
                }
            }
        }
    }

    pub(crate) async fn until_text(&mut self, link: &mut TermLink, text: &str) {
        self.until(link, false, |seen| seen.text().contains(text))
            .await;
    }

    pub(crate) async fn until_exit(&mut self, link: &mut TermLink) -> TermExit {
        self.until(link, false, |seen| seen.exit.is_some()).await;
        self.exit.clone().unwrap()
    }
}

/// A mark as the holder passes it on.
pub(crate) fn plain(mark: &Mark) -> String {
    match mark {
        Mark::Prompt => "\x1b]133;A\x07".into(),
        Mark::Input => "\x1b]133;B\x07".into(),
        Mark::Command { .. } => "\x1b]133;C\x07".into(),
        Mark::Done { exit: Some(exit) } => format!("\x1b]133;D;{exit}\x07"),
        Mark::Done { exit: None } => "\x1b]133;D\x07".into(),
    }
}

/// Checks each of `marks` is where it says in `bytes`, the output from its
/// start, and no other 133 got through.
pub(crate) fn assert_marks_in_place(bytes: &[u8], marks: &[(u64, Mark)]) {
    for (offset, mark) in marks {
        let rest = &bytes[usize::try_from(*offset).unwrap()..];
        assert!(
            rest.starts_with(plain(mark).as_bytes()),
            "{mark:?} at {offset}: {:?}",
            String::from_utf8_lossy(&rest[..rest.len().min(40)])
        );
    }
    let all = bytes.windows(6).filter(|w| *w == b"\x1b]133;").count();
    assert_eq!(all, marks.len(), "{:?}", String::from_utf8_lossy(bytes));
}

pub(crate) fn commands_of(marks: &[(u64, Mark)]) -> Commands {
    let mut commands = Commands::default();
    for (offset, mark) in marks {
        commands.mark(*offset, Timestamp::now(), mark);
    }
    commands
}

/// Waits until the shell has run `done` commands and shows its prompt for
/// the next.
pub(crate) async fn ready(watch: &mut TermLink, watched: &mut Seen, done: usize) {
    watched
        .until(watch, false, |seen| {
            let commands = commands_of(&seen.marks);
            let list = commands.list();
            list.len() == done + 1
                && list.iter().take(done).all(|command| command.end.is_some())
                && list.back().is_some_and(|last| last.input.is_some())
        })
        .await;
}
