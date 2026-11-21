// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: folding case
// for search, the scan that counts matches, selecting a match and the
// result for a selection, translated to Rust. The rest is MaxHQ's: which text
// of each item counts, and the lines it's cut into, so the app counts the
// same in each row.

//! Find across an agent's timeline. Each item is a document whose text is
//! what its row shows, in [`Shown`] lines: a match never crosses a block or
//! a line break, and each run of whitespace counts as one space. Matching
//! ignores case, the way the app's JavaScript lowercases, and counts UTF-16
//! units the same. [`select`] picks the active match among the counts and
//! names the items with matches around it.

use maxhq_protocol::timeline::{
    Delivery, FindAt, FindItem, FindMatch, ItemBody, ItemId, TimelineFind,
};

use crate::quote_span::Shown;

/// The longest query find takes, in UTF-16 units once trimmed.
pub const MAX_QUERY: usize = 200;

/// How many items with matches an answer names around the active one.
pub const NEAR: usize = 17;

/// `text` lowercased one UTF-16 unit for one, so a match's place in the
/// folded text is its place in the text. A character whose lowercase is
/// longer stays as it is, and a final sigma folds to the other sigma.
pub fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for c in text.chars() {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some('ς'), None) => folded.push('σ'),
            (Some(one), None) if one.len_utf16() == c.len_utf16() => folded.push(one),
            _ => folded.push(c),
        }
    }
    folded
}

/// How many times `query`, already folded, shows in `text`, already
/// folded. Matches don't overlap.
fn count_in(text: &str, query: &str) -> u32 {
    if text.is_empty() || query.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(query) {
        count += 1;
        cursor += offset + query.len();
    }
    count
}

/// What an item's row shows that find looks through, folded and cut into
/// the lines a match can't cross, in the order the row shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Searchable {
    lines: Vec<String>,
}

impl Searchable {
    /// What find looks through in an item with this body.
    pub fn of(body: &ItemBody) -> Self {
        let mut lines = Vec::new();
        for shown in fields(body) {
            for line in shown.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    lines.push(fold(line));
                }
            }
        }
        Self { lines }
    }

    /// How many times `query`, already folded, shows in the item.
    pub fn count(&self, query: &str) -> u32 {
        self.lines.iter().map(|line| count_in(line, query)).sum()
    }

    /// How many bytes its text takes.
    pub fn bytes(&self) -> usize {
        self.lines.iter().map(String::len).sum()
    }
}

/// The fields of an item that find looks through, in row order, each as it
/// shows. A queued or cancelled message has no row. Everything else shows
/// no text find counts.
fn fields(body: &ItemBody) -> Vec<Shown> {
    let plain = Shown::plain;
    let markdown = Shown::markdown;
    match body {
        ItemBody::UserMessage { text, delivery, .. } => match delivery {
            Delivery::Queued | Delivery::Cancelled => Vec::new(),
            Delivery::Sent | Delivery::Steer => vec![Shown::message(text)],
        },
        ItemBody::AssistantMessage { text, .. }
        | ItemBody::Reasoning { text, .. }
        | ItemBody::AgentAttachments { text, .. } => vec![markdown(text)],
        ItemBody::Plan { markdown: text, .. } => vec![markdown(text)],
        ItemBody::Command {
            command, output, ..
        } => vec![plain(command), plain(output)],
        ItemBody::ToolCall { input, output, .. } => [input, output]
            .into_iter()
            .flatten()
            .map(|text| plain(text))
            .collect(),
        ItemBody::Subagent {
            description,
            result,
            ..
        } => std::iter::once(plain(description))
            .chain(result.as_deref().map(markdown))
            .collect(),
        ItemBody::Error { message, .. } | ItemBody::Notice { message, .. } => vec![plain(message)],
        _ => Vec::new(),
    }
}

/// How many UTF-16 units `text` takes, as JavaScript counts its length.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Which match `at` makes active among items counted oldest first, and
/// the items with matches around it: all of them when there are at most
/// [`NEAR`], or else [`NEAR`] of them in a ring with the active one ninth.
pub fn select(counts: &[(ItemId, u32)], at: &FindAt) -> TimelineFind {
    let total: u32 = counts.iter().map(|(_, count)| *count).sum();
    let active = match at {
        FindAt::Index { index } => (*index).min(total.saturating_sub(1)),
        FindAt::From { from, offset } => {
            let mut requested = 0;
            if let Some(start) = counts.iter().position(|(item, _)| *item == from.item) {
                let before: u32 = counts[..start].iter().map(|(_, count)| *count).sum();
                requested = before + from.occurrence.min(counts[start].1);
                if requested >= total {
                    requested = 0;
                }
            }
            if total > 0 {
                (i64::from(requested) + i64::from(*offset)).rem_euclid(i64::from(total)) as u32
            } else {
                0
            }
        }
    };
    let mut first = 0;
    let mut entries = Vec::new();
    for (item, count) in counts {
        if *count > 0 {
            entries.push(FindItem {
                item: item.clone(),
                first,
                count: *count,
            });
        }
        first += count;
    }
    let holder = entries
        .iter()
        .position(|entry| active >= entry.first && active < entry.first + entry.count);
    let found = holder.map(|at| FindMatch {
        item: entries[at].item.clone(),
        occurrence: active - entries[at].first,
    });
    let near = match holder {
        Some(at) if entries.len() > NEAR => {
            let n = entries.len();
            (0..NEAR)
                .map(|i| entries[(at + i + n - NEAR / 2) % n].clone())
                .collect()
        }
        _ => entries,
    };
    TimelineFind {
        total,
        active,
        found,
        near,
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::context::REF_MARK;
    use maxhq_protocol::timeline::NoticeLevel;

    use super::*;

    fn count(body: &ItemBody, query: &str) -> u32 {
        Searchable::of(body).count(&fold(query))
    }

    fn reply(text: &str) -> ItemBody {
        ItemBody::AssistantMessage {
            text: text.to_owned(),
            streaming: false,
        }
    }

    fn sent(text: &str, delivery: Delivery) -> ItemBody {
        ItemBody::UserMessage {
            text: text.to_owned(),
            attachments: Vec::new(),
            refs: Vec::new(),
            delivery,
        }
    }

    fn id(n: usize) -> ItemId {
        ItemId(format!("item-{n}"))
    }

    fn counts(each: &[u32]) -> Vec<(ItemId, u32)> {
        each.iter()
            .enumerate()
            .map(|(n, count)| (id(n), *count))
            .collect()
    }

    fn from(item: usize, occurrence: u32, offset: i32) -> FindAt {
        FindAt::From {
            from: FindMatch {
                item: id(item),
                occurrence,
            },
            offset,
        }
    }

    fn found(find: &TimelineFind) -> Option<(ItemId, u32)> {
        find.found
            .as_ref()
            .map(|found| (found.item.clone(), found.occurrence))
    }

    #[test]
    fn case_folds_one_unit_for_one() {
        assert_eq!(fold("Hello WORLD"), "hello world");
        // İ lowercases to two characters, so it stays.
        assert_eq!(fold("İstanbul İ"), "İstanbul İ");
        assert_eq!(utf16_len(&fold("İx")), utf16_len("İx"));
        // Both sigmas fold the same, wherever they stand.
        assert_eq!(fold("ΟΔΟΣ"), "οδοσ");
        assert_eq!(fold("οδος"), "οδοσ");
        assert_eq!(count(&reply("ΟΔΟΣ οδος"), "οδος"), 2);
        assert_eq!(count(&reply("İstanbul"), "i̇stanbul"), 0);
        assert_eq!(count(&reply("İstanbul"), "İSTANBUL"), 1);
    }

    #[test]
    fn matches_dont_overlap() {
        assert_eq!(count(&reply("aaaa"), "aa"), 2);
        assert_eq!(count(&reply("aaa"), "aa"), 1);
        assert_eq!(count(&reply("Bug bug BUG"), "bug"), 3);
    }

    #[test]
    fn whitespace_runs_count_as_one_space() {
        let body = sent("flaky   test and\tflaky test", Delivery::Sent);
        assert_eq!(count(&body, "flaky test"), 2);
        // The query only loses its ends, as the app trims it.
        assert_eq!(count(&body, "flaky  test"), 0);
    }

    #[test]
    fn a_match_never_crosses_a_block_or_a_line() {
        assert_eq!(count(&reply("one\n\ntwo"), "one two"), 0);
        assert_eq!(count(&reply("one\ntwo"), "one two"), 0);
        assert_eq!(count(&reply("# one\ntwo"), "one two"), 0);
        assert_eq!(count(&reply("- one\n- two"), "one two"), 0);
        assert_eq!(
            count(&reply("| one | two |\n|---|---|\n| a | b |"), "one two"),
            0
        );
        assert_eq!(count(&reply("one  \ntwo"), "one two"), 0);
        assert_eq!(count(&reply("```\none\ntwo\n```"), "one two"), 0);
        assert_eq!(count(&reply("one **two** three"), "one two three"), 1);
        assert_eq!(count(&sent("one\ntwo", Delivery::Sent), "one two"), 0);
        assert_eq!(count(&sent("one\ntwo", Delivery::Sent), "two"), 1);
    }

    #[test]
    fn fields_count_in_row_order() {
        let command = ItemBody::Command {
            command: "cargo test".to_owned(),
            cwd: None,
            exit_code: Some(1),
            output: "test failed\ncargo test exited".to_owned(),
            output_truncated: false,
            background: false,
        };
        let shown = Searchable::of(&command);
        assert_eq!(
            shown.lines,
            vec!["cargo test", "test failed", "cargo test exited"]
        );
        assert_eq!(shown.count("cargo test"), 2);
        let tool = ItemBody::ToolCall {
            server: None,
            name: "search".to_owned(),
            title: None,
            input: Some("{\"q\": \"Needle\"}".to_owned()),
            output: Some("needle found".to_owned()),
            images: Vec::new(),
            artifact: None,
        };
        assert_eq!(
            Searchable::of(&tool).lines,
            vec!["{\"q\": \"needle\"}", "needle found"]
        );
        let subagent = ItemBody::Subagent {
            subagent: None,
            agent_type: None,
            description: "Find the bug".to_owned(),
            result: Some("**Found** the bug".to_owned()),
        };
        assert_eq!(
            Searchable::of(&subagent).lines,
            vec!["find the bug", "found the bug"]
        );
        let notice = ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "Context compacted".to_owned(),
        };
        assert_eq!(count(&notice, "compacted"), 1);
        assert_eq!(count(&ItemBody::Interrupted, "interrupted"), 0);
    }

    #[test]
    fn a_sent_message_counts_its_words_and_not_its_pills() {
        let text = format!("see {REF_MARK} now and{REF_MARK}then");
        assert_eq!(count(&sent(&text, Delivery::Sent), "see now"), 1);
        assert_eq!(count(&sent(&text, Delivery::Sent), "andthen"), 1);
        assert_eq!(count(&sent("hidden", Delivery::Queued), "hidden"), 0);
        assert_eq!(count(&sent("hidden", Delivery::Cancelled), "hidden"), 0);
        assert_eq!(count(&sent("steer", Delivery::Steer), "steer"), 1);
    }

    #[test]
    fn a_sent_message_counts_what_its_markdown_shows() {
        let message = |text: &str| sent(text, Delivery::Sent);
        assert_eq!(count(&message("a **bold** word"), "a bold word"), 1);
        assert_eq!(count(&message("a **bold** word"), "**"), 0);
        assert_eq!(count(&message("use `cargo` now"), "use cargo now"), 1);
        assert_eq!(count(&message("use <Foo> here"), "<foo>"), 1);
        assert_eq!(count(&message("<div>\nhello\n</div>"), "<div>"), 1);
        assert_eq!(count(&message("one\ntwo"), "one two"), 0);
        let text = format!("see **{REF_MARK} now**");
        assert_eq!(count(&message(&text), "see now"), 1);
    }

    #[test]
    fn markdown_counts_what_it_shows() {
        assert_eq!(
            count(&reply("a [link](https://example.com) here"), "example"),
            0
        );
        assert_eq!(
            count(&reply("a [link](https://example.com) here"), "a link here"),
            1
        );
        assert_eq!(count(&reply("use `cargo` now"), "use cargo now"), 1);
    }

    #[test]
    fn an_index_picks_that_match_held_to_the_last() {
        let counts = counts(&[2, 0, 3]);
        let find = select(&counts, &FindAt::Index { index: 3 });
        assert_eq!((find.total, find.active), (5, 3));
        assert_eq!(found(&find), Some((id(2), 1)));
        let find = select(&counts, &FindAt::Index { index: 99 });
        assert_eq!(find.active, 4);
        assert_eq!(found(&find), Some((id(2), 2)));
        let find = select(&counts, &FindAt::Index { index: 0 });
        assert_eq!(found(&find), Some((id(0), 0)));
    }

    #[test]
    fn nothing_found_is_none() {
        let find = select(&counts(&[0, 0]), &FindAt::Index { index: 4 });
        assert_eq!((find.total, find.active), (0, 0));
        assert_eq!(find.found, None);
        assert!(find.near.is_empty());
        let find = select(&counts(&[0, 0]), &from(1, 0, 1));
        assert_eq!((find.active, find.found), (0, None));
    }

    #[test]
    fn from_steps_and_wraps_round() {
        let counts = counts(&[2, 0, 3]);
        let find = select(&counts, &from(0, 1, 1));
        assert_eq!((find.active, found(&find)), (2, Some((id(2), 0))));
        let find = select(&counts, &from(0, 0, -1));
        assert_eq!((find.active, found(&find)), (4, Some((id(2), 2))));
        let find = select(&counts, &from(2, 2, 1));
        assert_eq!((find.active, found(&find)), (0, Some((id(0), 0))));
        let find = select(&counts, &from(0, 0, -11));
        assert_eq!(find.active, 4);
        // An item with no matches counts from where its matches would be.
        let find = select(&counts, &from(1, 0, 0));
        assert_eq!(found(&find), Some((id(2), 0)));
        // An occurrence past the item's last counts from just after it.
        let find = select(&counts, &from(0, 9, 0));
        assert_eq!(find.active, 2);
        // Past the last match goes back to the first.
        let find = select(&counts, &from(2, 3, 0));
        assert_eq!(find.active, 0);
    }

    #[test]
    fn from_an_item_thats_gone_counts_from_the_first_match() {
        let counts = counts(&[2, 3]);
        let gone = FindAt::From {
            from: FindMatch {
                item: ItemId("gone".to_owned()),
                occurrence: 1,
            },
            offset: 0,
        };
        assert_eq!(select(&counts, &gone).active, 0);
        let back = FindAt::From {
            from: FindMatch {
                item: ItemId("gone".to_owned()),
                occurrence: 1,
            },
            offset: -1,
        };
        assert_eq!(select(&counts, &back).active, 4);
    }

    #[test]
    fn near_holds_every_item_with_matches_up_to_seventeen() {
        let each: Vec<u32> = (0..34).map(|n| u32::from(n % 2 == 0)).collect();
        let find = select(&counts(&each), &FindAt::Index { index: 5 });
        assert_eq!(find.near.len(), 17);
        let items: Vec<ItemId> = find.near.iter().map(|entry| entry.item.clone()).collect();
        let expected: Vec<ItemId> = (0..34).step_by(2).map(id).collect();
        assert_eq!(items, expected);
        assert_eq!(find.near[3].first, 3);
        assert_eq!(find.near[3].count, 1);
    }

    #[test]
    fn past_seventeen_the_ring_puts_the_active_item_ninth() {
        let each = vec![1; 18];
        let find = select(&counts(&each), &FindAt::Index { index: 10 });
        let items: Vec<ItemId> = find.near.iter().map(|entry| entry.item.clone()).collect();
        let expected: Vec<ItemId> = (2..18).chain(0..1).map(id).collect();
        assert_eq!(items, expected);
        assert_eq!(find.near[8].item, id(10));
    }

    #[test]
    fn the_ring_wraps_round_the_ends() {
        let each = vec![2; 18];
        let find = select(&counts(&each), &FindAt::Index { index: 1 });
        let items: Vec<ItemId> = find.near.iter().map(|entry| entry.item.clone()).collect();
        let expected: Vec<ItemId> = (10..18).chain(0..9).map(id).collect();
        assert_eq!(items, expected);
        assert_eq!(find.near[8].item, id(0));
        assert_eq!(find.near[0].first, 20);
        let find = select(&counts(&each), &FindAt::Index { index: 35 });
        assert_eq!(find.near[8].item, id(17));
        assert_eq!(find.near[9].item, id(0));
    }
}
