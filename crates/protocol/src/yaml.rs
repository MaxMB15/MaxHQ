// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A check for YAML from a repository, a skill or an agent, to run before
//! parsing it. Two small files can cost far more than their size says. The
//! scanner's work grows with the tokens times how deep the `[` and `{` go,
//! before any parser's limit applies, so tens of kilobytes take seconds. And
//! each alias replays what it names, so nine kilobytes of nested aliases
//! expand to gigabytes of values, under the parser's limits on both jumps
//! and depth.

use std::fmt;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, IgnoredAny, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};

/// How deep `[` and `{` may go. Real files stay under ten.
pub const MAX_FLOW_DEPTH: usize = 64;

/// How many bytes of text cost the same as one node.
const BYTES_PER_NODE: usize = 64;

/// Whether YAML `text`, also given as `document` in the parser's own
/// deserializer, may be parsed with at most `budget` nodes, aliases counted
/// each time they repeat. A long text counts as more than one node, since an
/// alias can repeat that too. YAML that doesn't parse passes here, so the
/// real parse says what's wrong with it.
pub fn check<'de, D: Deserializer<'de>>(
    text: &[u8],
    document: D,
    budget: u64,
) -> Result<(), &'static str> {
    if flow_too_deep(text) {
        return Err("Its `[` and `{` go too deep.");
    }
    let mut left = budget;
    let walked = Count { left: &mut left }.deserialize(document);
    if walked.is_err() && left == 0 {
        return Err("It expands to too much.");
    }
    Ok(())
}

/// Walks a document without keeping any of it, until the budget runs out.
struct Count<'a> {
    left: &'a mut u64,
}

impl Count<'_> {
    fn spend<E: de::Error>(self, bytes: usize) -> Result<(), E> {
        let cost = 1 + (bytes / BYTES_PER_NODE) as u64;
        if cost > *self.left {
            *self.left = 0;
            return Err(E::custom("too much"));
        }
        *self.left -= cost;
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for Count<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, document: D) -> Result<(), D::Error> {
        document.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Count<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any YAML")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_i128<E: de::Error>(self, _: i128) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<(), E> {
        self.spend(text.len())
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<(), E> {
        self.spend(bytes.len())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        self.spend(0)
    }

    fn visit_some<D: Deserializer<'de>>(self, inner: D) -> Result<(), D::Error> {
        inner.deserialize_any(self)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, inner: D) -> Result<(), D::Error> {
        inner.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<(), A::Error> {
        let left = self.left;
        Count { left: &mut *left }.spend::<A::Error>(0)?;
        while items
            .next_element_seed(Count { left: &mut *left })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<(), A::Error> {
        let left = self.left;
        Count { left: &mut *left }.spend::<A::Error>(0)?;
        while entries.next_key_seed(Count { left: &mut *left })?.is_some() {
            entries.next_value_seed(Count { left: &mut *left })?;
        }
        Ok(())
    }

    fn visit_enum<A: EnumAccess<'de>>(self, tagged: A) -> Result<(), A::Error> {
        let (_, value) = tagged.variant::<IgnoredAny>()?;
        value.newtype_variant_seed(self)
    }
}

/// Whether the `[` and `{` in `text` go past [`MAX_FLOW_DEPTH`], leaving out
/// those in quoted text and comments. This runs before the scanner it's
/// there to spare, so it's a small lexer of its own rather than a full one,
/// and where it can't tell it counts too much: a quote running over several
/// lines might not be one, so the openers in it count and the closers don't.
pub fn flow_too_deep(text: &[u8]) -> bool {
    let mut depth = 0usize;
    // Whether a token can start here, as a quoted text can only there.
    let mut start = true;
    let mut at = 0;
    while let Some(&c) = text.get(at) {
        let blank_after = text.get(at + 1).is_none_or(|next| blank(*next));
        match c {
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_FLOW_DEPTH {
                    return true;
                }
                start = true;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                start = true;
            }
            b',' => start = true,
            b'#' if at == 0 || blank(text[at - 1]) => {
                at = line_end(text, at);
                start = true;
                continue;
            }
            b'\'' | b'"' if start => {
                let end = quote_end(text, at);
                let inside = &text[at + 1..end];
                if inside.iter().any(|c| matches!(*c, b'\n' | b'\r')) {
                    depth += inside.iter().filter(|c| matches!(*c, b'[' | b'{')).count();
                    if depth > MAX_FLOW_DEPTH {
                        return true;
                    }
                }
                at = end + 1;
                continue;
            }
            // A verbatim tag (`!<...>`) takes flow characters into its text
            // and ends at its `>`. What follows is read as usual, so a
            // bracket after it still counts.
            b'!' if start && text[at..].starts_with(b"!<") => {
                at += 2;
                while text.get(at).is_some_and(|c| !blank(*c) && *c != b'>') {
                    at += 1;
                }
                if text.get(at) == Some(&b'>') {
                    at += 1;
                }
                continue;
            }
            // Any other tag, anchor or alias comes before a token or is one,
            // and flow characters end it.
            b'!' | b'&' | b'*' if start => {
                while text
                    .get(at)
                    .is_some_and(|c| !blank(*c) && !b"[]{},".contains(c))
                {
                    at += 1;
                }
                continue;
            }
            b'-' | b'.'
                if start
                    && (at == 0 || matches!(text[at - 1], b'\n' | b'\r'))
                    && (text[at..].starts_with(b"---") || text[at..].starts_with(b"..."))
                    && text.get(at + 3).is_none_or(|next| blank(*next)) =>
            {
                at += 3;
                continue;
            }
            b'-' | b'?' if start && blank_after => {}
            b':' if start || blank_after => start = true,
            b'\n' | b'\r' => {
                // A plain text in flow goes on over lines.
                if depth == 0 {
                    start = true;
                }
            }
            b' ' | b'\t' => {}
            _ => start = false,
        }
        at += 1;
    }
    false
}

fn blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0)
}

/// The first line break from `at` on, counting the Unicode ones the scanner
/// counts too, or the end of `text`.
fn line_end(text: &[u8], from: usize) -> usize {
    let mut at = from;
    while let Some(&c) = text.get(at) {
        let rest = &text[at..];
        if matches!(c, b'\n' | b'\r')
            || rest.starts_with(b"\xC2\x85")
            || rest.starts_with(b"\xE2\x80\xA8")
            || rest.starts_with(b"\xE2\x80\xA9")
        {
            return at;
        }
        at += 1;
    }
    at
}

/// Where the quote opening at `from` closes, or the end of `text` when it
/// never does.
fn quote_end(text: &[u8], from: usize) -> usize {
    let quote = text[from];
    let mut at = from + 1;
    while let Some(&c) = text.get(at) {
        if c == b'\\' && quote == b'"' {
            at += 2;
        } else if c == quote {
            if quote == b'\'' && text.get(at + 1) == Some(&b'\'') {
                at += 2;
            } else {
                return at;
            }
        } else {
            at += 1;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deep(open: &str, close: &str, depth: usize) -> String {
        format!("x: {}{}\n", open.repeat(depth), close.repeat(depth))
    }

    #[test]
    fn real_nesting_passes() {
        for text in [
            "",
            "a: [1, 2, {b: [3, {c: d}]}]\n",
            "a: 'it''s [[[[' # and [[[[[\nb: \"\\\" [[[[\"\n",
            "a: [!<tag:yaml.org,2002:str> x, {b: [1]}]\n",
            &deep("[", "]", MAX_FLOW_DEPTH),
            &deep("{", "}", MAX_FLOW_DEPTH),
        ] {
            assert!(!flow_too_deep(text.as_bytes()), "{text}");
        }
    }

    #[test]
    fn flow_past_the_limit_is_too_deep() {
        for text in [
            deep("[", "]", MAX_FLOW_DEPTH + 1),
            deep("{", "}", MAX_FLOW_DEPTH + 1),
            deep("[", "", 100_000),
            deep("{a: ", "}", MAX_FLOW_DEPTH + 1),
            deep("!!seq [", "]", MAX_FLOW_DEPTH + 1),
            deep("&a[", "]", MAX_FLOW_DEPTH + 1),
            deep("[!!seq ", "]", MAX_FLOW_DEPTH + 1),
            deep("[!<]> ", "", 100_000),
            deep("[!<a>,", "", 100_000),
            deep("[!<a>[", "", 100_000),
            deep("[!<a,b> ", "]", MAX_FLOW_DEPTH + 1),
        ] {
            assert!(flow_too_deep(text.as_bytes()), "{}", &text[..20]);
        }
    }

    #[test]
    fn quoted_text_and_comments_dont_count() {
        let many = "[".repeat(1000);
        for text in [
            format!("a: '{many}'\n"),
            format!("a: \"{many}\"\n"),
            format!("# {many}\n"),
            format!("a: b # {many}\n"),
            format!("[1, '{many}', \"{many}\"] # {many}\n"),
            format!("!!str \"a # b\": '{many}'\n"),
            format!("--- '{many}'\n"),
            format!("{{\"a\":'{many}'}}\n"),
            format!("- '{many}'\n- ? '{many}'\n"),
        ] {
            assert!(!flow_too_deep(text.as_bytes()), "{}", &text[..12]);
        }
    }

    #[test]
    fn what_only_looks_like_quoted_text_or_a_comment_counts() {
        let many = "[".repeat(1000);
        for text in [
            // An apostrophe in a word opens no quote.
            format!("don't: {many}\n"),
            format!("a: don't {many} it's\n"),
            // A hash in a word opens no comment.
            format!("a#b: {many}\n"),
            // A comment ends at any line break the scanner knows.
            format!("# a\u{2028}{many}\n"),
            format!("# a\u{85}{many}\n"),
            // The hash is inside the quote, and the quote ends before the
            // brackets.
            format!("[ \"a # b\", {many}\n"),
            format!("!!str \"a # b\": {many}\n"),
            // A plain text in flow goes on over lines, so this is no quote.
            format!("[ a\n '{many}'\n"),
        ] {
            assert!(flow_too_deep(text.as_bytes()), "{}", &text[..12]);
        }
    }

    #[test]
    fn a_quote_over_several_lines_cant_hide_or_close_brackets() {
        // Each round opens a real bracket, and its closer is in the quote.
        assert!(flow_too_deep("[ \"\n]\n\" ,".repeat(100).as_bytes()));
        // Brackets inside one that never closes count.
        assert!(flow_too_deep(format!("'a\n{}", "[".repeat(100)).as_bytes()));
    }

    #[test]
    fn the_walk_stops_at_its_budget_and_long_text_costs_more() {
        let walk = |text: &str, budget| {
            check(
                text.as_bytes(),
                &mut serde_json::Deserializer::from_str(text),
                budget,
            )
        };
        // The list and its three numbers.
        assert_eq!(walk("[1,2,3]", 4), Ok(()));
        assert!(walk("[1,2,3]", 3).is_err());
        // A map's keys count too.
        assert_eq!(walk("{\"a\":1}", 3), Ok(()));
        assert!(walk("{\"a\":1}", 2).is_err());
        let long = format!("\"{}\"", "a".repeat(10 * BYTES_PER_NODE));
        assert_eq!(walk(&long, 11), Ok(()));
        assert!(walk(&long, 10).is_err());
    }

    #[test]
    fn what_doesnt_parse_is_left_for_the_real_parse_to_explain() {
        let text = "[1, oops";
        let document = &mut serde_json::Deserializer::from_str(text);
        assert_eq!(check(text.as_bytes(), document, 100), Ok(()));
    }
}
