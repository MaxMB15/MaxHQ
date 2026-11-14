// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Synthetic output for the benchmarks.

/// A buffer of `line` plus a newline, repeated to about `size` bytes, so a
/// flood writes in large blocks the way `yes` does.
pub fn flood_block(line: &str, size: usize) -> Vec<u8> {
    let unit = format!("{line}\n");
    let count = (size / unit.len()).max(1);
    unit.repeat(count).into_bytes()
}

const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
const FILL: &[u8] = b"working on the task ";

/// One spinner tick: a carriage return, then exactly `cols` columns. The fill
/// shifts by one column per tick, so nearly every cell changes, which is
/// the worst case for a renderer that only redraws what changed.
pub fn spinner_line(tick: u64, cols: usize, elapsed_secs: f64) -> String {
    let frame = FRAMES[(tick % FRAMES.len() as u64) as usize];
    let mut line = format!("\r{frame} {elapsed_secs:7.1}s ");
    let used = line.len() - 1;
    let shift = (tick % FILL.len() as u64) as usize;
    line.extend(
        (0..cols.saturating_sub(used))
            .map(|column| char::from(FILL[(column + shift) % FILL.len()])),
    );
    line.truncate(cols + 1);
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_blocks_are_whole_lines() {
        let block = flood_block("y", 64 * 1024);
        assert_eq!(block.len(), 64 * 1024);
        assert!(block.chunks(2).all(|pair| pair == b"y\n"));
        assert_eq!(
            flood_block("longer than the block", 4),
            b"longer than the block\n"
        );
    }

    #[test]
    fn spinner_lines_fill_the_width_exactly() {
        for tick in 0..50 {
            let line = spinner_line(tick, 200, 12.34);
            assert_eq!(line.len(), 201);
            assert!(line.starts_with('\r'));
        }
        assert_eq!(spinner_line(0, 5, 1.0), "\r|    ");
    }

    #[test]
    fn spinner_lines_change_every_cell() {
        let a = spinner_line(1, 80, 1.0);
        let b = spinner_line(2, 80, 1.0);
        let fill_start = 12;
        assert!(a[fill_start..]
            .bytes()
            .zip(b[fill_start..].bytes())
            .all(|(x, y)| x != y));
    }
}
