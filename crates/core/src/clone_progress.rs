// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: parsing one git
// clone progress line, translated to Rust without regexes. MaxHQ
// shows no detail after the percent, so the speed git prints isn't kept.
// `Lines`, which cuts stderr into those lines, is MaxHQ's own.

//! Reads the progress lines `git clone --progress` writes to stderr. Git
//! redraws a line with a bare CR, so pass each redraw as a line of its own.

use maxhq_protocol::model::CloneStage;

/// What one progress line says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// What git is doing.
    pub stage: CloneStage,
    /// How far along that stage is, 0 to 100, when the line says.
    pub percent: Option<u8>,
}

/// The line starts that name each stage, in the order git prints them.
const STAGE_PREFIXES: [(&str, CloneStage); 7] = [
    ("remote: Enumerating objects", CloneStage::Counting),
    ("remote: Counting objects", CloneStage::Counting),
    ("remote: Compressing objects", CloneStage::Counting),
    ("Receiving objects", CloneStage::Receiving),
    ("Resolving deltas", CloneStage::Resolving),
    ("Updating files", CloneStage::Checkout),
    ("Checking out files", CloneStage::Checkout),
];

/// Reads one line of git's stderr, or None when it isn't progress.
pub fn parse(line: &str) -> Option<Progress> {
    let line = line.trim();
    let stage = STAGE_PREFIXES
        .iter()
        .find(|(prefix, _)| line.starts_with(prefix))?
        .1;
    let percent = line
        .match_indices(':')
        .find_map(|(at, _)| percent_after(&line[at + 1..]));
    Some(Progress { stage, percent })
}

/// Reads ` 42% (420/1000)` after a colon, then nothing, or a comma and
/// whatever git adds, such as its speed or `done.`.
fn percent_after(rest: &str) -> Option<u8> {
    let rest = rest.strip_prefix(char::is_whitespace)?.trim_start();
    let (percent, rest) = digits(rest)?;
    let rest = rest.strip_prefix('%')?;
    let rest = rest.strip_prefix(char::is_whitespace)?.trim_start();
    let rest = rest.strip_prefix('(')?;
    let (_, rest) = digits(rest)?;
    let rest = rest.strip_prefix('/')?;
    let (_, rest) = digits(rest)?;
    let rest = rest.strip_prefix(')')?;
    let tail = rest.trim();
    let done = tail
        .strip_prefix(',')
        .is_some_and(|tail| tail.trim_start() == "done.");
    if !(tail.is_empty() || rest.starts_with(',') || done) {
        return None;
    }
    // Digits past what a u8 holds clamp to 100, as git never prints them.
    Some(
        percent
            .parse::<u32>()
            .map_or(100, |percent| percent.min(100)) as u8,
    )
}

/// Splits leading ASCII digits off, if there are any.
fn digits(text: &str) -> Option<(&str, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    (end > 0).then(|| text.split_at(end))
}

/// The longest line [`Lines`] hands on. Git's progress and error lines
/// are far shorter, so a longer one is cut.
pub const MAX_LINE: usize = 1024;

/// Cuts git's stderr into lines as it comes in chunks, at a CR or a LF.
#[derive(Debug, Default)]
pub struct Lines {
    /// The start of a line whose end hasn't come yet.
    partial: Vec<u8>,
}

impl Lines {
    /// Hands each line that `chunk` ends to `each`, without its CR or LF.
    pub fn push(&mut self, chunk: &[u8], mut each: impl FnMut(&str)) {
        for part in chunk.split_inclusive(|&byte| byte == b'\r' || byte == b'\n') {
            let (text, ended) = match part.split_last() {
                Some((&(b'\r' | b'\n'), text)) => (text, true),
                _ => (part, false),
            };
            let room = MAX_LINE.saturating_sub(self.partial.len());
            self.partial
                .extend_from_slice(&text[..text.len().min(room)]);
            if ended {
                each(&String::from_utf8_lossy(&self.partial));
                self.partial.clear();
            }
        }
    }

    /// Hands on the last line, which git ended without a CR or LF.
    pub fn finish(&mut self, mut each: impl FnMut(&str)) {
        if !self.partial.is_empty() {
            each(&String::from_utf8_lossy(&self.partial));
            self.partial.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(stage: CloneStage, percent: u8) -> Option<Progress> {
        Some(Progress {
            stage,
            percent: Some(percent),
        })
    }

    #[test]
    fn reads_each_stage_and_its_percent() {
        let lines = [
            (
                "remote: Enumerating objects: 1234, done.",
                Some(Progress {
                    stage: CloneStage::Counting,
                    percent: None,
                }),
            ),
            (
                "remote: Counting objects:  50% (617/1234)",
                at(CloneStage::Counting, 50),
            ),
            (
                "remote: Compressing objects: 100% (500/500), done.",
                at(CloneStage::Counting, 100),
            ),
            (
                "Receiving objects:  42% (519/1234), 1.20 MiB | 2.40 MiB/s",
                at(CloneStage::Receiving, 42),
            ),
            (
                "Receiving objects: 100% (1234/1234), 3.10 MiB | 2.40 MiB/s, done.",
                at(CloneStage::Receiving, 100),
            ),
            (
                "Resolving deltas:   7% (30/400)  ",
                at(CloneStage::Resolving, 7),
            ),
            (
                "Updating files:  99% (990/1000)",
                at(CloneStage::Checkout, 99),
            ),
            (
                "Checking out files: 100% (12/12), done.",
                at(CloneStage::Checkout, 100),
            ),
        ];
        for (line, want) in lines {
            assert_eq!(parse(line), want, "{line}");
        }
    }

    #[test]
    fn other_lines_are_not_progress() {
        for line in [
            "Cloning into 'name'...",
            "remote: Total 1234 (delta 400), reused 0 (delta 0), pack-reused 0",
            "fatal: repository 'https://example.com/owner/name.git/' not found",
            "",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
    }

    #[test]
    fn a_line_without_a_well_formed_percent_has_none() {
        let none = |stage| {
            Some(Progress {
                stage,
                percent: None,
            })
        };
        assert_eq!(
            parse("Receiving objects: 42% (519/1234) stray"),
            none(CloneStage::Receiving)
        );
        assert_eq!(
            parse("Receiving objects: 42% (519/1234) , 1.20 MiB"),
            none(CloneStage::Receiving)
        );
        assert_eq!(
            parse("Resolving deltas: 100% (5/5) , done."),
            at(CloneStage::Resolving, 100)
        );
        assert_eq!(parse("Receiving objects: 42%"), none(CloneStage::Receiving));
        assert_eq!(
            parse("Resolving deltas:42% (1/2)"),
            none(CloneStage::Resolving)
        );
        assert_eq!(
            parse("Receiving objects: 900% (9/1)"),
            at(CloneStage::Receiving, 100)
        );
    }

    #[test]
    fn lines_end_at_a_cr_or_a_lf_across_chunks() {
        let mut lines = Lines::default();
        let mut seen = Vec::new();
        for chunk in [
            &b"Receiving objects:  1% (1/100)\rReceiving obj"[..],
            b"ects:  2% (2/100)\r\nfatal: gone",
        ] {
            lines.push(chunk, |line| seen.push(line.to_owned()));
        }
        lines.finish(|line| seen.push(line.to_owned()));
        assert_eq!(
            seen,
            [
                "Receiving objects:  1% (1/100)",
                "Receiving objects:  2% (2/100)",
                "",
                "fatal: gone",
            ]
        );
    }

    #[test]
    fn a_long_line_is_cut() {
        let mut lines = Lines::default();
        let mut seen = Vec::new();
        let long = vec![b'x'; MAX_LINE * 3];
        lines.push(&long, |line| seen.push(line.len()));
        lines.push(b"yy\n", |line| seen.push(line.len()));
        assert_eq!(seen, [MAX_LINE]);
    }
}
