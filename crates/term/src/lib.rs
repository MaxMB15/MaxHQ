// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Headless terminal emulation.
//!
//! A command you run with `!` prints to a terminal of its own, and what it
//! printed goes in the timeline as styled lines. The holder feeds the
//! command's output through an [`Emulator`], so progress bars that redraw a
//! line, colors and cursor moves end up as the lines you would have seen,
//! and asks it for a [`Transcript`] when the command ends.
//!
//! The holder is also the one answer a program gets when it asks its
//! terminal something, such as where the cursor is: the emulator keeps the
//! [`Emulator::answers`] that are safe to give, and the holder writes them
//! back. It reads the line the cursor waits on with [`Emulator::look`].
//!
//! The emulator sits behind one trait so libghostty-vt can replace the one
//! inside (decision A-4).

mod alacritty;
mod look;
mod scan;

pub use look::Look;

#[cfg(test)]
mod tests;

/// The most lines a transcript keeps, counted from the end.
pub const MAX_LINES: usize = 2_000;

/// The most bytes a transcript's text keeps, counted from the end.
pub const MAX_BYTES: usize = 512 * 1024;

/// Shown after a prompt line when you sent input while the terminal hid
/// what you typed.
pub const SECRET_SENT: &str = " 🔒 sent";

/// The last line of a transcript whose program used the alternate screen.
pub const FULL_SCREEN: &str = "full-screen program";

/// What a program printed, as the lines a terminal showed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    /// The lines, oldest first, joined with `\n`. Each one carries only SGR
    /// escapes for its colors and styles, and ends unstyled.
    pub text: String,
    /// How many lines the program printed, counting those the transcript
    /// left out.
    pub lines: u32,
    /// Whether the transcript left lines out, or the start of one.
    pub truncated: bool,
}

/// A terminal with no window, fed a program's output.
pub trait Emulator: Send {
    /// Takes bytes the program printed.
    fn feed(&mut self, bytes: &[u8]);
    /// Gives the terminal a new size in cells.
    fn resize(&mut self, cols: u16, rows: u16);
    /// Writes `text` at the cursor as though the program printed it, such as
    /// [`SECRET_SENT`]. It never takes what you typed.
    ///
    /// A leading space is left out when the cursor sits after a space or at
    /// the start of a line, so `Password: ` reads `Password: 🔒 sent`.
    fn note(&mut self, text: &str);
    /// The lines so far. Ask once the program has ended: a program still on
    /// the alternate screen is taken off it, back to the normal screen.
    fn transcript(&mut self) -> Transcript;
    /// The answers to what the program asked its terminal since the last
    /// call, in order, for the program to read: the cursor's place, the
    /// terminal's status, its kind and its size in cells. Anything else it
    /// asks goes unanswered, and nothing that echoes what the program or
    /// anyone sent comes back. At most [`MAX_ANSWERS`] bytes wait.
    fn answers(&mut self) -> Vec<u8>;
    /// The screen's modes and the line the cursor is on.
    fn look(&self) -> Look;
}

/// The most answer bytes an emulator keeps between two calls to
/// [`Emulator::answers`]. A program that floods its terminal with questions
/// gets no more.
pub const MAX_ANSWERS: usize = 4 * 1024;

/// The emulator a `!` run uses, `cols` by `rows` cells, which keeps every
/// line for its transcript.
pub fn emulator(cols: u16, rows: u16) -> Box<dyn Emulator> {
    Box::new(alacritty::Screen::new(cols, rows, alacritty::HISTORY))
}

/// An emulator that keeps only the screen, `cols` by `rows` cells, for a
/// shell that can run for days: it answers and looks like [`emulator`]
/// does, and its transcript has only what the screen shows.
pub fn screen(cols: u16, rows: u16) -> Box<dyn Emulator> {
    Box::new(alacritty::Screen::new(cols, rows, 0))
}
