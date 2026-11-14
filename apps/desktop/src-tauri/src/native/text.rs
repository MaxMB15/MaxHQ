// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Text helpers for the spell checker. JavaScript string indexes and
//! `NSString` ranges both count UTF-16 code units, so every offset here
//! does too, and a range found in a sentence maps onto the composer's text
//! by adding where the sentence starts.

use std::ops::Range;

/// Splits text into sentences, as UTF-16 ranges with the whitespace around
/// them left out. The spell checker guesses the language once per call, so
/// checking one sentence at a time lets a Dutch sentence sit next to an
/// English one. A sentence ends at a line break, or after `.`, `!`, `?` or
/// `…` and any closing quotes or brackets, when whitespace follows.
pub fn sentences(units: &[u16]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut start = None;
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        if is_line_break(unit) {
            push_trimmed(&mut found, units, start.take(), index);
            index += 1;
            continue;
        }
        if start.is_none() && !is_space(unit) {
            start = Some(index);
        }
        if is_terminator(unit) {
            let mut end = index + 1;
            while end < units.len() && (is_terminator(units[end]) || is_closer(units[end])) {
                end += 1;
            }
            if end == units.len() || is_space(units[end]) || is_line_break(units[end]) {
                push_trimmed(&mut found, units, start.take(), end);
            }
            index = end;
            continue;
        }
        index += 1;
    }
    push_trimmed(&mut found, units, start, units.len());
    found
}

/// The text in a UTF-16 range, or none when the range doesn't fit the
/// text. A range that splits a surrogate pair gets a replacement character
/// for the broken half.
pub fn slice(units: &[u16], range: Range<usize>) -> Option<String> {
    units.get(range).map(String::from_utf16_lossy)
}

/// Records the sentence from `start` to `end`, without trailing
/// whitespace, unless nothing is left.
fn push_trimmed(found: &mut Vec<Range<usize>>, units: &[u16], start: Option<usize>, end: usize) {
    let Some(start) = start else { return };
    let mut end = end;
    while end > start && is_space(units[end - 1]) {
        end -= 1;
    }
    if end > start {
        found.push(start..end);
    }
}

/// Whether a code unit is whitespace. Half a surrogate pair isn't.
fn is_space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(char::is_whitespace)
}

/// Line feed, carriage return, and the Unicode line and paragraph
/// separators.
fn is_line_break(unit: u16) -> bool {
    matches!(unit, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// Marks that end a sentence, in Latin script and the full-width forms.
fn is_terminator(unit: u16) -> bool {
    matches!(
        unit,
        // . ! ? … 。 ！ ？
        0x2E | 0x21 | 0x3F | 0x2026 | 0x3002 | 0xFF01 | 0xFF1F
    )
}

/// Quotes and brackets that can close a sentence after its full stop.
fn is_closer(unit: u16) -> bool {
    matches!(
        unit,
        // " ' ) ] } ’ ” »
        0x22 | 0x27 | 0x29 | 0x5D | 0x7D | 0x2019 | 0x201D | 0xBB
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// The sentences as strings, which reads better in a failure.
    fn split(text: &str) -> Vec<String> {
        let units = utf16(text);
        sentences(&units)
            .into_iter()
            .map(|range| slice(&units, range).unwrap())
            .collect()
    }

    #[test]
    fn splits_at_full_stops_followed_by_space() {
        assert_eq!(
            split("Dit is Nederlands. This is English! Is it? Yes"),
            ["Dit is Nederlands.", "This is English!", "Is it?", "Yes"]
        );
    }

    #[test]
    fn keeps_dots_inside_words_and_numbers() {
        assert_eq!(
            split("Open main.rs at v1.2 now. Then stop."),
            ["Open main.rs at v1.2 now.", "Then stop."]
        );
    }

    #[test]
    fn closing_quotes_stay_with_their_sentence() {
        assert_eq!(
            split("He said \"go.\" Then (it ran.) Done"),
            ["He said \"go.\"", "Then (it ran.)", "Done"]
        );
    }

    #[test]
    fn line_breaks_end_sentences_and_blank_lines_vanish() {
        assert_eq!(
            split("  first line  \n\n\r\nsecond line\u{2029}third"),
            ["first line", "second line", "third"]
        );
    }

    #[test]
    fn empty_and_blank_text_has_no_sentences() {
        assert!(split("").is_empty());
        assert!(split(" \n\t ").is_empty());
        assert!(split("...").len() == 1);
    }

    #[test]
    fn offsets_count_utf16_units() {
        // "é" is one unit, "😀" is two, and "漢" is one.
        let text = "Café 😀 ok. 漢字 teh.";
        let units = utf16(text);
        let found = sentences(&units);
        assert_eq!(found, [0..11, 12..19]);
        // JavaScript's "Café 😀 ok. 漢字 teh.".indexOf("teh") is 15.
        let teh = 15..18;
        assert_eq!(slice(&units, teh).as_deref(), Some("teh"));
        assert_eq!(slice(&units, 5..7).as_deref(), Some("😀"));
    }

    #[test]
    fn slices_outside_the_text_are_none() {
        let units = utf16("abc");
        assert_eq!(slice(&units, 2..4), None);
        assert_eq!(slice(&units, 3..3).as_deref(), Some(""));
    }

    #[test]
    fn a_split_surrogate_pair_becomes_a_replacement_character() {
        let units = utf16("😀");
        assert_eq!(slice(&units, 0..1).as_deref(), Some("\u{FFFD}"));
    }

    #[test]
    fn full_width_stops_end_sentences() {
        assert_eq!(
            split("今日は。 明日も！ ok"),
            ["今日は。", "明日も！", "ok"]
        );
    }
}
