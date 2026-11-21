// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Whether the desktop app's window still answers. The page and every frame
//! in it share one thread, so a visual or a diagram that loops forever
//! stops the whole window, and nothing in the page can tell. The shell asks
//! the window a tiny question every second, and a question left unanswered
//! for a few seconds means the window has stopped.
//!
//! While it's stopped, a note waits in the app's data folder, and the window
//! answering again takes it away. The next launch tells its first page
//! whether the note was there. When it wasn't, what was running when the app
//! went away ran fine, so the page runs it again without asking.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often the shell asks the window whether it answers.
pub const ASK_EVERY: Duration = Duration::from_secs(1);

/// How long a question waits for its answer before the window counts as
/// stopped. A busy window answers well within it.
pub const STOPPED_AFTER: Duration = Duration::from_secs(3);

/// The note's name in the app's data folder.
const NOTE: &str = "window-stopped";

/// What the shell does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing for now.
    Wait,
    /// Asks the window the question with this id.
    Ask(u64),
    /// Leaves the note when the window stopped, or takes it away when it
    /// answers again.
    Note(bool),
}

#[derive(Debug)]
struct State {
    /// Whether a page is there to answer.
    shown: bool,
    /// The question waiting for its answer, and when it was asked.
    asked: Option<(u64, Instant)>,
    next: u64,
    /// Whether the note is there.
    stopped: bool,
}

/// The window's answers, kept for the shell.
#[derive(Debug)]
pub struct HangWatch(Mutex<State>);

impl HangWatch {
    /// A watch that starts out knowing whether the note is there.
    pub fn new(stopped: bool) -> Self {
        Self(Mutex::new(State {
            shown: false,
            asked: None,
            next: 0,
            stopped,
        }))
    }

    /// A page started showing, as on a launch or a reload. A question the
    /// page before it had may never be answered, so the next goes to this
    /// one.
    pub fn showing(&self) {
        let mut state = self.lock();
        state.shown = true;
        state.asked = None;
    }

    /// The window answered the question with this id.
    pub fn answered(&self, id: u64) {
        let mut state = self.lock();
        if state.asked.is_some_and(|(asked, _)| asked == id) {
            state.asked = None;
        }
    }

    /// What to do at `now`, a moment after the last step.
    pub fn step(&self, now: Instant) -> Step {
        let mut state = self.lock();
        if !state.shown {
            return Step::Wait;
        }
        let stopped = state
            .asked
            .is_some_and(|(_, at)| now.saturating_duration_since(at) >= STOPPED_AFTER);
        if stopped != state.stopped {
            state.stopped = stopped;
            return Step::Note(stopped);
        }
        if state.asked.is_some() {
            return Step::Wait;
        }
        let id = state.next;
        state.next += 1;
        state.asked = Some((id, now));
        Step::Ask(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether the note is in `dir`, so the window had stopped when the app
/// last went away.
pub fn noted(dir: &Path) -> bool {
    dir.join(NOTE).exists()
}

/// Leaves the note in `dir`, or takes it away.
pub fn note(dir: &Path, stopped: bool) -> io::Result<()> {
    let path = dir.join(NOTE);
    if stopped {
        fs::create_dir_all(dir)?;
        return fs::write(path, b"");
    }
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The script that tells each page, before its own, whether the window had
/// stopped when the app last went away.
pub fn page_script(stopped: bool) -> String {
    format!("window.__MAXHQ_LAST_STOPPED__={stopped};")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_once_a_page_shows_and_one_question_at_a_time() {
        let watch = HangWatch::new(false);
        let start = Instant::now();
        assert_eq!(watch.step(start), Step::Wait);
        watch.showing();
        assert_eq!(watch.step(start), Step::Ask(0));
        assert_eq!(watch.step(start + ASK_EVERY), Step::Wait);
        watch.answered(0);
        assert_eq!(watch.step(start + ASK_EVERY * 2), Step::Ask(1));
    }

    #[test]
    fn notes_a_window_that_stops_answering_until_it_answers_again() {
        let watch = HangWatch::new(false);
        let start = Instant::now();
        watch.showing();
        assert_eq!(watch.step(start), Step::Ask(0));
        assert_eq!(watch.step(start + STOPPED_AFTER / 2), Step::Wait);
        assert_eq!(watch.step(start + STOPPED_AFTER), Step::Note(true));
        assert_eq!(watch.step(start + STOPPED_AFTER * 2), Step::Wait);
        // An answer to another question doesn't count.
        watch.answered(7);
        assert_eq!(watch.step(start + STOPPED_AFTER * 3), Step::Wait);
        watch.answered(0);
        assert_eq!(watch.step(start + STOPPED_AFTER * 4), Step::Note(false));
        assert_eq!(watch.step(start + STOPPED_AFTER * 4), Step::Ask(1));
    }

    #[test]
    fn a_note_from_the_last_launch_goes_once_the_window_answers() {
        let watch = HangWatch::new(true);
        let start = Instant::now();
        assert_eq!(watch.step(start), Step::Wait);
        watch.showing();
        assert_eq!(watch.step(start), Step::Note(false));
        assert_eq!(watch.step(start), Step::Ask(0));
    }

    #[test]
    fn a_new_page_answers_in_place_of_the_last() {
        let watch = HangWatch::new(false);
        let start = Instant::now();
        watch.showing();
        assert_eq!(watch.step(start), Step::Ask(0));
        watch.showing();
        assert_eq!(watch.step(start + STOPPED_AFTER), Step::Ask(1));
    }

    #[test]
    fn the_note_comes_and_goes() {
        let dir = std::env::temp_dir().join(format!("maxhq-hang-watch-{}", uuid::Uuid::now_v7()));
        assert!(!noted(&dir));
        note(&dir, false).unwrap();
        note(&dir, true).unwrap();
        assert!(noted(&dir));
        note(&dir, true).unwrap();
        note(&dir, false).unwrap();
        assert!(!noted(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_page_learns_how_the_last_launch_ended() {
        assert_eq!(page_script(true), "window.__MAXHQ_LAST_STOPPED__=true;");
        assert_eq!(page_script(false), "window.__MAXHQ_LAST_STOPPED__=false;");
    }
}
