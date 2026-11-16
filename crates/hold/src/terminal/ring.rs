// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The last output a terminal printed, by absolute offset.

use std::collections::VecDeque;

/// How much output a terminal holder keeps.
pub(crate) const RING_BYTES: usize = 2 * 1024 * 1024;

/// The newest output, up to a cap. Offsets count every byte the terminal
/// ever printed, so they stay put as old bytes fall off the front.
pub(crate) struct Ring {
    bytes: VecDeque<u8>,
    start: u64,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            start: 0,
            cap,
        }
    }

    /// The offset of the oldest byte kept.
    #[cfg(test)]
    pub fn start(&self) -> u64 {
        self.start
    }

    /// The offset just past the newest byte.
    pub fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    /// Adds output, dropping the oldest bytes past the cap.
    pub fn push(&mut self, data: &[u8]) {
        let skipped = data.len().saturating_sub(self.cap);
        let data = &data[skipped..];
        let over = (self.bytes.len() + data.len()).saturating_sub(self.cap);
        self.bytes.drain(..over);
        self.start += (over + skipped) as u64;
        self.bytes.extend(data);
    }

    /// Where a reader that has everything before `after` carries on, and
    /// whether it must start over because the bytes it needs are gone.
    ///
    /// With `after` held it's `after` itself. Otherwise it's the start of
    /// the first whole line kept, so a reader never starts in the middle of
    /// one or of an escape sequence. With no `after` the reader has
    /// nothing, so it never needs to start over.
    pub fn resume(&self, after: Option<u64>) -> (u64, bool) {
        if let Some(after) = after
            && (self.start..=self.end()).contains(&after)
        {
            return (after, false);
        }
        let from = if self.start == 0 {
            0
        } else {
            self.bytes
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(self.end(), |at| self.start + at as u64 + 1)
        };
        (from, after.is_some())
    }

    /// Up to `max` bytes from `from`, or `None` when they've fallen off the
    /// front.
    pub fn read(&self, from: u64, max: usize) -> Option<Vec<u8>> {
        let skip = usize::try_from(from.checked_sub(self.start)?).ok()?;
        if skip > self.bytes.len() {
            return Some(Vec::new());
        }
        Some(self.bytes.range(skip..).take(max).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_stay_put_as_old_bytes_drop() {
        let mut ring = Ring::new(8);
        ring.push(b"abcdef");
        assert_eq!((ring.start(), ring.end()), (0, 6));
        ring.push(b"ghij");
        assert_eq!((ring.start(), ring.end()), (2, 10));
        assert_eq!(ring.read(2, 100).unwrap(), b"cdefghij");
        assert_eq!(ring.read(8, 1).unwrap(), b"i");
        assert_eq!(ring.read(10, 100).unwrap(), b"");
        assert_eq!(ring.read(1, 100), None);
        // More than the cap at once keeps its end.
        ring.push(b"0123456789xyz");
        assert_eq!((ring.start(), ring.end()), (15, 23));
        assert_eq!(ring.read(15, 100).unwrap(), b"56789xyz");
    }

    #[test]
    fn a_reader_still_held_carries_on_where_it_was() {
        let mut ring = Ring::new(16);
        ring.push(b"one\ntwo\n");
        assert_eq!(ring.resume(Some(4)), (4, false));
        assert_eq!(ring.resume(Some(8)), (8, false));
        assert_eq!(ring.resume(None), (0, false));
        assert_eq!(ring.resume(Some(0)), (0, false));
    }

    #[test]
    fn a_reader_too_far_behind_starts_over_at_a_whole_line() {
        let mut ring = Ring::new(10);
        ring.push(b"first\nsecond\nthird");
        // The ring holds "cond\nthird", so the first whole line is "third".
        assert_eq!(ring.start(), 8);
        assert_eq!(ring.resume(Some(3)), (13, true));
        assert_eq!(ring.resume(None), (13, false));
        assert_eq!(ring.read(13, 100).unwrap(), b"third");
        // An offset past the end can't be right either.
        assert_eq!(ring.resume(Some(99)), (13, true));
        // With no newline at all, the reader starts at the end.
        let mut ring = Ring::new(4);
        ring.push(b"abcdefgh");
        assert_eq!(ring.resume(Some(0)), (8, true));
    }
}
