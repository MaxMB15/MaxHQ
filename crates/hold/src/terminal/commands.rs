// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands a shell ran, as its marks tell them: where each one's
//! prompt, input and output start in the output, how it ended and in what
//! folder it ran.

use std::collections::VecDeque;

use maxhq_protocol::time::Timestamp;

use super::marks::Mark;

/// How many commands a terminal keeps, the last ones.
const KEEP: usize = 1000;

/// One command, by offsets into the terminal's output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellCommand {
    /// Where its prompt starts.
    pub prompt: Option<u64>,
    /// Where what was typed starts, after the prompt.
    pub input: Option<u64>,
    /// Where its output starts, once it runs.
    pub start: Option<u64>,
    /// When it started.
    pub started_at: Option<Timestamp>,
    /// Where its output ends, once it has.
    pub end: Option<u64>,
    /// When it ended.
    pub ended_at: Option<Timestamp>,
    /// Its exit code, when the shell said.
    pub exit: Option<i32>,
    /// The folder it ran in, as the shell said it last.
    pub cwd: Option<String>,
}

impl ShellCommand {
    fn running(&self) -> bool {
        self.start.is_some() && self.end.is_none()
    }
}

/// A terminal's commands, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Commands {
    list: VecDeque<ShellCommand>,
}

impl Commands {
    /// Takes in a mark the shell made at `offset`.
    pub fn mark(&mut self, offset: u64, at: Timestamp, mark: &Mark) {
        match mark {
            Mark::Prompt => {
                self.close(offset);
                // A prompt with nothing run from it.
                if self.list.back().is_some_and(|last| last.start.is_none()) {
                    self.list.pop_back();
                }
                self.push(ShellCommand {
                    prompt: Some(offset),
                    ..ShellCommand::default()
                });
            }
            Mark::Input => {
                if let Some(last) = self.list.back_mut().filter(|last| last.start.is_none()) {
                    last.input = Some(offset);
                }
            }
            Mark::Command { cwd } => {
                self.close(offset);
                if self.list.back().is_none_or(|last| last.start.is_some()) {
                    self.push(ShellCommand::default());
                }
                if let Some(last) = self.list.back_mut() {
                    last.start = Some(offset);
                    last.started_at = Some(at);
                    last.cwd = cwd.clone();
                }
            }
            Mark::Done { exit } => {
                if let Some(last) = self.list.back_mut().filter(|last| last.running()) {
                    last.end = Some(offset);
                    last.ended_at = Some(at);
                    last.exit = *exit;
                }
            }
        }
    }

    /// Ends a command that ran with no end said: a shell started inside it,
    /// or it ended without its shell knowing.
    fn close(&mut self, offset: u64) {
        if let Some(last) = self.list.back_mut().filter(|last| last.running()) {
            last.end = Some(offset);
        }
    }

    fn push(&mut self, command: ShellCommand) {
        if self.list.len() == KEEP {
            self.list.pop_front();
        }
        self.list.push_back(command);
    }

    /// Whether a command runs now.
    pub fn running(&self) -> bool {
        self.list.back().is_some_and(ShellCommand::running)
    }

    /// The commands, oldest first. The last may not have run yet.
    pub fn list(&self) -> &VecDeque<ShellCommand> {
        &self.list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(commands: &mut Commands, marks: &[(u64, Mark)]) {
        for (offset, mark) in marks {
            commands.mark(*offset, Timestamp::now(), mark);
        }
    }

    fn command(cwd: &str) -> Mark {
        Mark::Command {
            cwd: Some(cwd.into()),
        }
    }

    #[test]
    fn commands_go_from_prompt_to_done() {
        let mut commands = Commands::default();
        feed(&mut commands, &[(0, Mark::Prompt), (10, Mark::Input)]);
        assert!(!commands.running());
        feed(&mut commands, &[(15, command("/tmp"))]);
        assert!(commands.running());
        feed(&mut commands, &[(40, Mark::Done { exit: Some(2) })]);
        assert!(!commands.running());
        feed(&mut commands, &[(40, Mark::Prompt), (50, Mark::Input)]);
        let list: Vec<_> = commands.list().iter().cloned().collect();
        assert_eq!(list.len(), 2);
        let first = &list[0];
        assert_eq!(
            (
                first.prompt,
                first.input,
                first.start,
                first.end,
                first.exit
            ),
            (Some(0), Some(10), Some(15), Some(40), Some(2))
        );
        assert_eq!(first.cwd.as_deref(), Some("/tmp"));
        assert!(first.started_at.is_some() && first.ended_at.is_some());
        assert_eq!((list[1].prompt, list[1].start), (Some(40), None));
    }

    #[test]
    fn empty_prompts_go_and_lost_ends_close() {
        let mut commands = Commands::default();
        feed(
            &mut commands,
            &[
                (0, Mark::Prompt),
                (5, Mark::Prompt),
                (9, command("/a")),
                // `exec bash`: a new shell's first prompt with no D.
                (20, Mark::Prompt),
                (30, Mark::Done { exit: Some(1) }),
            ],
        );
        let list = commands.list();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].prompt, list[0].start), (Some(5), Some(9)));
        assert_eq!((list[0].end, list[0].exit), (Some(20), None));
        assert_eq!((list[1].prompt, list[1].end), (Some(20), None));
        // A command with no prompt marked still counts.
        feed(&mut commands, &[(40, command("/b"))]);
        assert!(commands.running());
        assert_eq!(commands.list().len(), 2);
        assert_eq!(commands.list()[1].start, Some(40));
        feed(&mut commands, &[(50, command("/c"))]);
        let list = commands.list();
        assert_eq!(list.len(), 3);
        assert_eq!((list[1].end, list[2].start), (Some(50), Some(50)));
    }

    #[test]
    fn it_keeps_the_last_ones() {
        let mut commands = Commands::default();
        for at in 0..(KEEP as u64 + 5) {
            feed(
                &mut commands,
                &[
                    (at * 2, command("/")),
                    (at * 2 + 1, Mark::Done { exit: Some(0) }),
                ],
            );
        }
        assert_eq!(commands.list().len(), KEEP);
        assert_eq!(commands.list()[0].start, Some(10));
    }
}
