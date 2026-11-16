// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The [`Emulator`] on `alacritty_terminal`.
//!
//! Lines that scroll off the screen go to the backend's scrollback. Long
//! before it fills, they move out of it into [`Screen::kept`] as rendered
//! text, so a run that prints more than the scrollback holds still counts
//! every line, and the transcript knows what it left out.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Timeout};
use alacritty_terminal::Term;

use crate::scan::{Scanner, Seen};
use crate::{Emulator, Transcript, FULL_SCREEN, MAX_BYTES, MAX_LINES};

/// The backend's scrollback, in rows.
const HISTORY: usize = 10_000;

/// Rows of scrollback that start a move into [`Screen::kept`].
const DRAIN_AT: usize = 4_096;

/// The most bytes fed between two looks at the scrollback. One byte adds at
/// most one row, so the scrollback never passes `DRAIN_AT + PIECE` rows,
/// well inside [`HISTORY`].
const PIECE: usize = 4_096;

/// Styles a transcript keeps.
const STYLES: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::STRIKEOUT);

/// A terminal size the backend reads.
struct Size {
    cols: usize,
    rows: usize,
}

impl Size {
    fn new(cols: u16, rows: u16) -> Self {
        // Two columns at least, so a wide character fits.
        Self {
            cols: usize::from(cols.max(2)),
            rows: usize::from(rows.max(1)),
        }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Never holds output back for a synchronized update: nothing draws this
/// terminal, so there's no frame to keep whole.
#[derive(Default)]
struct NoSync;

impl Timeout for NoSync {
    fn set_timeout(&mut self, _: Duration) {}

    fn clear_timeout(&mut self) {}

    fn pending_timeout(&self) -> bool {
        false
    }
}

/// The emulator, with what it moved out of the scrollback.
pub(crate) struct Screen {
    term: Term<VoidListener>,
    parser: Processor<NoSync>,
    scanner: Scanner,
    /// Lines moved out of the scrollback, rendered, oldest first, within
    /// [`MAX_LINES`] and about [`MAX_BYTES`].
    kept: VecDeque<String>,
    /// The bytes in `kept`, with a newline for each line.
    kept_bytes: usize,
    /// How many lines went to `kept`, with those it has dropped since.
    moved: u64,
    /// Whether the start of a line was dropped: a line longer than
    /// [`MAX_BYTES`], or one that filled the scrollback.
    cut: bool,
    /// Whether the program ever used the alternate screen.
    used_alt: bool,
}

impl Screen {
    pub(crate) fn new(cols: u16, rows: u16) -> Self {
        let config = Config {
            scrolling_history: HISTORY,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &Size::new(cols, rows), VoidListener),
            parser: Processor::new(),
            scanner: Scanner::default(),
            kept: VecDeque::new(),
            kept_bytes: 0,
            moved: 0,
            cut: false,
            used_alt: false,
        }
    }

    fn on_alt(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Feeds the backend, then moves old rows out of its scrollback.
    fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.drain();
    }

    fn act_on(&mut self, seen: Seen) {
        match seen {
            Seen::AltScreen { on, legacy } => {
                self.used_alt |= on;
                if legacy {
                    // Swapping twice is a no-op, so 1049 beside 47 is fine.
                    self.advance(if on { b"\x1b[?1049h" } else { b"\x1b[?1049l" });
                }
            }
            // The alternate screen has no scrollback to clear.
            Seen::ClearSaved if self.on_alt() => {}
            Seen::ClearSaved | Seen::Reset => self.forget_kept(),
        }
    }

    /// Drops what moved out of the scrollback, once the program cleared it.
    fn forget_kept(&mut self) {
        self.kept.clear();
        self.kept_bytes = 0;
        self.moved = 0;
        self.cut = false;
    }

    /// Moves the scrollback's rows into `kept` once there are
    /// [`DRAIN_AT`] of them, up to the last row that ends a line. Rows in
    /// the scrollback never change again, so they can be rendered now.
    fn drain(&mut self) {
        if self.on_alt() {
            return;
        }
        let grid = self.term.grid();
        let history = grid.history_size();
        if history < DRAIN_AT {
            return;
        }
        let top = grid.topmost_line().0;
        let mut last = -1;
        while last >= top && wraps(grid, Line(last)) {
            last -= 1;
        }
        if last < top {
            // One line fills the whole scrollback. Drop its start: the line
            // goes on and counts once it ends.
            self.cut = true;
            self.shrink(history);
            return;
        }
        let lines = logical_lines(grid, top, last);
        self.shrink((last - top + 1) as usize);
        for line in lines {
            self.keep(line);
        }
    }

    /// Takes the oldest `rows` rows out of the scrollback.
    fn shrink(&mut self, rows: usize) {
        let grid = self.term.grid_mut();
        let history = grid.history_size();
        grid.update_history(history - rows);
        grid.update_history(HISTORY);
    }

    fn keep(&mut self, line: String) {
        self.moved += 1;
        let line = if line.len() > MAX_BYTES {
            self.cut = true;
            tail(&line, MAX_BYTES).to_owned()
        } else {
            line
        };
        self.kept_bytes += line.len() + 1;
        self.kept.push_back(line);
        while self.kept.len() > MAX_LINES || self.kept_bytes > MAX_BYTES + 1 {
            let Some(old) = self.kept.pop_front() else {
                break;
            };
            self.kept_bytes -= old.len() + 1;
        }
    }
}

impl Emulator for Screen {
    fn feed(&mut self, bytes: &[u8]) {
        let mut start = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            let seen = self.scanner.step(byte);
            if seen.is_none() && at + 1 - start < PIECE {
                continue;
            }
            self.advance(&bytes[start..=at]);
            start = at + 1;
            if let Some(seen) = seen {
                self.act_on(seen);
            }
        }
        if start < bytes.len() {
            self.advance(&bytes[start..]);
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size::new(cols, rows));
        self.drain();
    }

    fn note(&mut self, text: &str) {
        let grid = self.term.grid();
        let point = grid.cursor.point;
        let after_space =
            point.column.0 == 0 || grid[point.line][Column(point.column.0 - 1)].c == ' ';
        let text = match text.strip_prefix(' ') {
            Some(rest) if after_space => rest,
            _ => text,
        };
        self.advance(text.as_bytes());
    }

    fn transcript(&mut self) -> Transcript {
        if self.on_alt() {
            self.advance(b"\x1b[?1049l");
        }
        let grid = self.term.grid();
        let screen = logical_lines(grid, grid.topmost_line().0, grid.bottommost_line().0);
        let mut all: Vec<&str> = self
            .kept
            .iter()
            .map(String::as_str)
            .chain(screen.iter().map(String::as_str))
            .collect();
        while all.last().is_some_and(|line| line.is_empty()) {
            all.pop();
        }
        let dropped = self.moved - self.kept.len() as u64;
        let lines = u32::try_from(dropped + all.len() as u64).unwrap_or(u32::MAX);
        let (max_lines, max_bytes) = if self.used_alt {
            (MAX_LINES - 1, MAX_BYTES - FULL_SCREEN.len() - 1)
        } else {
            (MAX_LINES, MAX_BYTES)
        };
        let (mut text, left_out) = join_tail(&all, max_lines, max_bytes);
        if self.used_alt {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(FULL_SCREEN);
        }
        Transcript {
            text,
            lines,
            truncated: left_out || dropped > 0 || self.cut,
        }
    }
}

/// Whether the row at `line` goes on in the row below.
fn wraps(grid: &Grid<Cell>, line: Line) -> bool {
    let row = &grid[line];
    row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE)
}

/// The rows from `first` to `last`, both in, as rendered lines, with rows
/// that wrapped joined to the rows they wrapped onto.
fn logical_lines(grid: &Grid<Cell>, first: i32, last: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cells: Vec<&Cell> = Vec::new();
    for line in first..=last {
        let row = &grid[Line(line)];
        cells.extend((0..row.len()).map(|column| &row[Column(column)]));
        if !wraps(grid, Line(line)) {
            lines.push(render(&cells));
            cells.clear();
        }
    }
    if !cells.is_empty() {
        lines.push(render(&cells));
    }
    lines
}

/// A cell's colors and styles, as the transcript keeps them. The default
/// colors are `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    flags: Flags,
    fg: Option<Color>,
    bg: Option<Color>,
}

impl Style {
    /// No style at all.
    const PLAIN: Self = Self {
        flags: Flags::empty(),
        fg: None,
        bg: None,
    };

    fn of(cell: &Cell) -> Self {
        Self {
            flags: cell.flags & STYLES,
            fg: palette(cell.fg),
            bg: palette(cell.bg),
        }
    }

    /// Writes the SGR escape that sets this style from scratch.
    fn write(&self, out: &mut String) {
        out.push_str("\x1b[0");
        for (flag, code) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::ALL_UNDERLINES, "4"),
            (Flags::INVERSE, "7"),
            (Flags::STRIKEOUT, "9"),
        ] {
            if self.flags.intersects(flag) {
                out.push(';');
                out.push_str(code);
            }
        }
        if let Some(fg) = self.fg {
            write_color(fg, 30, out);
        }
        if let Some(bg) = self.bg {
            write_color(bg, 40, out);
        }
        out.push('m');
    }
}

/// `color`, or `None` for the terminal's own foreground and background.
fn palette(color: Color) -> Option<Color> {
    match color {
        Color::Named(named) if named_code(named).is_none() => None,
        other => Some(other),
    }
}

/// A named color's place among the 16 base colors.
fn named_code(named: NamedColor) -> Option<u8> {
    let index = named as usize;
    let dim = NamedColor::DimBlack as usize;
    match index {
        0..=15 => Some(index as u8),
        _ if (dim..dim + 8).contains(&index) => Some((index - dim) as u8),
        _ => None,
    }
}

/// Writes `;` and the SGR code for `color`, where `base` is 30 for the
/// foreground and 40 for the background.
fn write_color(color: Color, base: u8, out: &mut String) {
    match color {
        Color::Named(named) => {
            if let Some(code) = named_code(named) {
                let code = if code < 8 {
                    base + code
                } else {
                    base + 60 + code - 8
                };
                let _ = write!(out, ";{code}");
            }
        }
        Color::Indexed(index) => {
            let _ = write!(out, ";{};5;{index}", base + 8);
        }
        Color::Spec(rgb) => {
            let _ = write!(out, ";{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b);
        }
    }
}

/// Whether a cell shows nothing, so it can go from the end of a line.
fn is_blank(cell: &Cell) -> bool {
    is_spacer(cell)
        || (matches!(cell.c, ' ' | '\t')
            && cell.zerowidth().is_none()
            && palette(cell.bg).is_none()
            && !cell
                .flags
                .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT))
}

/// Whether a cell only holds room for a wide character beside it.
fn is_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

/// One line, its trailing blank cells left out, with an SGR escape wherever
/// the style changes and a reset at the end if it ends styled.
fn render(cells: &[&Cell]) -> String {
    let end = cells
        .iter()
        .rposition(|cell| !is_blank(cell))
        .map_or(0, |at| at + 1);
    let mut out = String::new();
    let mut style = Style::PLAIN;
    for cell in &cells[..end] {
        if is_spacer(cell) {
            continue;
        }
        let next = Style::of(cell);
        if next != style {
            if next == Style::PLAIN {
                out.push_str("\x1b[0m");
            } else {
                next.write(&mut out);
            }
            style = next;
        }
        // A tab leaves its cell holding `\t` and the cells it skipped
        // blank, so a space keeps the columns where they were. Hidden text
        // stays hidden.
        if cell.c == '\t' || cell.flags.contains(Flags::HIDDEN) {
            out.push(' ');
        } else {
            out.push(cell.c);
        }
        if let Some(marks) = cell.zerowidth() {
            out.extend(marks);
        }
    }
    if style != Style::PLAIN {
        out.push_str("\x1b[0m");
    }
    out
}

/// The last lines of `lines` that fit in `max_lines` lines and `max_bytes`
/// bytes joined with `\n`, and whether any were left out. When not even the
/// last line fits, its end.
fn join_tail(lines: &[&str], max_lines: usize, max_bytes: usize) -> (String, bool) {
    let Some(last) = lines.len().checked_sub(1) else {
        return (String::new(), false);
    };
    let floor = lines.len().saturating_sub(max_lines);
    let mut first = lines.len();
    let mut bytes = 0;
    for at in (floor..lines.len()).rev() {
        let add = lines[at].len() + usize::from(at != last);
        if bytes + add > max_bytes {
            break;
        }
        bytes += add;
        first = at;
    }
    if first == lines.len() {
        return (tail(lines[last], max_bytes).to_owned(), true);
    }
    (lines[first..].join("\n"), first > 0)
}

/// The end of a rendered `line` in at most `max` bytes, starting on a
/// character and outside an escape.
fn tail(line: &str, max: usize) -> &str {
    let mut start = line.len().saturating_sub(max);
    while !line.is_char_boundary(start) {
        start += 1;
    }
    if let Some(escape) = line[..start].rfind('\x1b')
        && !line[escape..start].contains('m')
    {
        start = line[start..]
            .find('m')
            .map_or(line.len(), |at| start + at + 1);
    }
    &line[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_line_starts_on_a_character_outside_an_escape() {
        let line = "\x1b[0;31méé\x1b[0m";
        // Cut inside the escape, then inside a character.
        assert_eq!(tail(line, line.len() - 3), "éé\x1b[0m");
        assert_eq!(tail(line, 7), "é\x1b[0m");
        assert_eq!(tail(line, 2), "");
    }

    #[test]
    fn only_whole_lines_go_until_one_would_not_fit() {
        assert_eq!(join_tail(&[], 5, 5), (String::new(), false));
        assert_eq!(join_tail(&["a", "b", "c"], 2, 100), ("b\nc".into(), true));
        assert_eq!(join_tail(&["aa", "b", "c"], 5, 4), ("b\nc".into(), true));
        assert_eq!(
            join_tail(&["aa", "b", "c"], 5, 6),
            ("aa\nb\nc".into(), false)
        );
        assert_eq!(join_tail(&["abcdef"], 5, 3), ("def".into(), true));
    }
}
