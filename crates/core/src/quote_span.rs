// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES, by way of
// MaxHQ's packages/shared/src/pills/anchor.ts: finding a quote, translated
// to Rust. It finds a quote again where it shows up nearest to where it
// started, with runs of whitespace counted as one space. The rest is
// MaxHQ's: building the text the app shows from an item's Markdown, and
// mapping what's found back to the source.

//! Where a quote sits in the item it's from. The app keeps where a quote
//! starts in the item's text as it shows it: in UTF-16 units, with each run
//! of whitespace as one space. A reply and a message you sent show as
//! Markdown, so that text isn't their source. [`Shown`] builds the text the
//! app shows from the source, close enough for the nearest match to land on
//! the right place, and maps each character back to the source bytes it
//! comes from.

use std::collections::HashSet;
use std::ops::Range;

use maxhq_protocol::context::REF_MARK;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A quote the user took from an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// The quoted text.
    pub text: String,
    /// Where it starts in the item's text as the app shows it, in UTF-16
    /// units with each run of whitespace as one.
    pub position: u32,
}

/// One character of the shown text.
#[derive(Clone, Debug)]
struct Unit {
    /// Where it starts in the shown text, in bytes.
    at: usize,
    /// Where it starts in the shown text, in UTF-16 units.
    utf16: usize,
    /// The source bytes it comes from.
    source: Range<usize>,
}

/// An item's text as the app shows it, with each run of whitespace as one
/// space, and the blocks of its source.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// The shown text.
    text: String,
    /// Each of its characters, in order.
    units: Vec<Unit>,
    /// How long it is in UTF-16 units.
    utf16: usize,
    /// The source's leaf blocks as byte ranges, in order, with no
    /// whitespace at either end.
    blocks: Vec<Range<usize>>,
    /// Whether a block ended or a break came since the last character, so
    /// a space goes before the next one.
    pending: bool,
    /// Where each line of the shown text after the first starts, in bytes:
    /// after a block, a break or a line break in the text.
    cuts: Vec<usize>,
    /// Whether a pill's mark drops out, since the row's text skips the pill.
    skips_marks: bool,
}

/// The options that match what remark-gfm and remark-math read.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// The words a callout can start with, as `markdown-plugins.ts` lists them.
const ALERT_WORDS: [&str; 27] = [
    "note",
    "abstract",
    "summary",
    "tldr",
    "info",
    "todo",
    "tip",
    "hint",
    "success",
    "check",
    "done",
    "question",
    "help",
    "faq",
    "important",
    "warning",
    "attention",
    "caution",
    "failure",
    "fail",
    "missing",
    "danger",
    "error",
    "bug",
    "example",
    "quote",
    "cite",
];

/// The tags raw HTML can show, as `markdown-plugins.ts` lists them. Any other
/// reads as the text it was written as.
const SHOWN_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "ul",
    "var",
];

/// The tags that stand with no pair, as `markdown-plugins.ts` lists them.
const NEEDS_NO_PAIR: &[&str] = &[
    "br", "hr", "img", "input", "li", "p", "dt", "dd", "tr", "td", "th", "thead", "tbody", "tfoot",
    "rp", "rt",
];

/// A piece of markup in raw HTML.
enum Markup {
    /// All or part of a comment.
    Comment,
    /// What only reads as text: `<?`, `<!`, or `</` with no name.
    Text,
    Tag {
        name: String,
        closing: bool,
    },
}

/// The tag `html` starts with: whether it closes, its name in lower case, and
/// where it ends, which is the end of `html` if nothing ends it there. A `>`
/// in quotes doesn't end it. There's none if what follows `<` isn't a name.
fn read_tag(html: &str) -> Option<(bool, String, usize)> {
    let after = html.strip_prefix('<')?;
    let (closing, named) = match after.strip_prefix('/') {
        Some(named) => (true, named),
        None => (false, after),
    };
    if !named.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_end = named
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(named.len());
    let name = named[..name_end].to_ascii_lowercase();
    let start = html.len() - named.len() + name_end;
    let mut quote = None;
    for (offset, c) in html[start..].char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return Some((closing, name, start + offset + 1)),
            None => {}
        }
    }
    Some((closing, name, html.len()))
}

/// The markup in one event's raw HTML, in order, read as `HTML_MARKUP` in
/// `markdown-plugins.ts` reads it. A comment can end in a later event than it
/// started, so `in_comment` carries over between them.
fn html_markup(html: &str, in_comment: &mut bool) -> Vec<(Range<usize>, Markup)> {
    let mut found = Vec::new();
    let mut from = 0;
    while from < html.len() {
        let rest = &html[from..];
        if *in_comment {
            let end = rest.find("-->").map(|end| from + end + "-->".len());
            found.push((from..end.unwrap_or(html.len()), Markup::Comment));
            let Some(end) = end else { break };
            from = end;
            *in_comment = false;
            continue;
        }
        let Some(open) = rest.find('<') else { break };
        from += open;
        let tag = &html[from..];
        if tag.starts_with("<!--") {
            found.push((from..from + "<!--".len(), Markup::Comment));
            from += "<!--".len();
            *in_comment = true;
        } else if let Some((closing, name, end)) = read_tag(tag) {
            found.push((from..from + end, Markup::Tag { name, closing }));
            from += end;
        } else if tag.starts_with("<?") || tag.starts_with("<!") || tag.starts_with("</") {
            found.push((from..from + "<?".len(), Markup::Text));
            from += "<?".len();
        } else {
            from += "<".len();
        }
    }
    found
}

/// Where each tag starts that the app shows as text, in `source`: one off the
/// list, as in `Vec<String>`, and one with no pair, as `remarkLiteralTags` does
/// in `markdown-plugins.ts`. Tags pair up across the source here and within a
/// block there, which is close enough for finding a quote.
fn literal_tags(source: &str) -> HashSet<usize> {
    let mut literal = HashSet::new();
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut in_comment = false;
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        let (Event::Html(text) | Event::InlineHtml(text)) = event else {
            continue;
        };
        for (place, markup) in html_markup(&text, &mut in_comment) {
            let at = range.start + place.start;
            let (name, closing) = match markup {
                Markup::Comment => continue,
                Markup::Text => {
                    literal.insert(at);
                    continue;
                }
                Markup::Tag { name, closing } => (name, closing),
            };
            if !SHOWN_TAGS.contains(&name.as_str()) {
                literal.insert(at);
            } else if NEEDS_NO_PAIR.contains(&name.as_str()) {
                continue;
            } else if !closing {
                open.push((name, at));
            } else if let Some(pair) = open.iter().rposition(|(opened, _)| *opened == name) {
                literal.extend(open.drain(pair..).skip(1).map(|(_, at)| at));
            } else {
                literal.insert(at);
            }
        }
    }
    literal.extend(open.into_iter().map(|(_, at)| at));
    literal
}

/// Whether a fence's info string names what the app draws as a picture,
/// which leaves its text out.
fn draws_picture(info: &str) -> bool {
    let name = info.split_whitespace().next().unwrap_or("");
    name.eq_ignore_ascii_case("mermaid") || name.eq_ignore_ascii_case("math")
}

/// Where the `[!NOTE]` that makes the quote at `start` a callout ends, along
/// with the spaces after it. The app shows its name or title instead.
fn alert_marker_end(source: &str, start: usize) -> Option<usize> {
    let quoted = source[start..].strip_prefix('>')?;
    let marker = quoted.trim_start_matches([' ', '\t']).strip_prefix("[!")?;
    let close = marker.find(']')?;
    let word = marker[..close].to_ascii_lowercase();
    if !ALERT_WORDS.contains(&word.as_str()) {
        return None;
    }
    let after = &marker[close + 1..];
    let after = after.strip_prefix(['+', '-']).unwrap_or(after);
    Some(source.len() - after.trim_start_matches([' ', '\t']).len())
}

/// Whether a `$` formula is one the app reads as plain dollars, because one
/// of its ends touches a letter or a digit, as `$5-$10` and `a$b$c` do.
fn dollars_are_plain(source: &str, range: &Range<usize>) -> bool {
    source[..range.start]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
        || source[range.end..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric)
}

/// Whether a tag starts a leaf block, one whose text is all inline.
fn is_leaf(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::Table(_)
    )
}

/// Whether a tag shows as a block, which the app's row text puts a line
/// break around.
fn is_block(tag: &Tag) -> bool {
    !matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

fn ends_block(tag: &TagEnd) -> bool {
    !matches!(
        tag,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

impl Shown {
    /// The text `source` shows as: rendered from Markdown, as a reply is,
    /// or as it is, as a command's output is.
    pub fn new(source: &str, markdown: bool) -> Self {
        if markdown {
            Self::markdown(source)
        } else {
            Self::plain(source)
        }
    }

    /// Text that shows as it is. A block is a run of lines between blank
    /// lines.
    pub fn plain(source: &str) -> Self {
        let mut shown = Self::default();
        shown.push(source, 0);
        let mut start = None;
        let mut at = 0;
        for line in source.split_inclusive('\n') {
            let blank = line.trim().is_empty();
            match (blank, start) {
                (false, None) => start = Some(at),
                (true, Some(from)) => {
                    shown.block(source, from..at);
                    start = None;
                }
                _ => {}
            }
            at += line.len();
        }
        if let Some(from) = start {
            shown.block(source, from..source.len());
        }
        shown
    }

    /// Markdown, rendered the way the app's reply view does, only closer
    /// than exact. Each block's text gets a line break around it and at a
    /// hard break, as the app's row text does. Raw HTML shows the text
    /// between its tags. Images, task list boxes, math and diagrams are left
    /// out, as are a callout's marker and the name it shows for one.
    /// Footnote references show their number.
    pub fn markdown(source: &str) -> Self {
        Self::rendered(source, false)
    }

    /// A message you sent, rendered the way the app's bubble does: Markdown
    /// whose raw HTML shows as text, since it was typed to be read, with
    /// each pill's mark left out.
    pub fn message(source: &str) -> Self {
        Self::rendered(source, true)
    }

    fn rendered(source: &str, message: bool) -> Self {
        let mut shown = Self {
            skips_marks: message,
            ..Self::default()
        };
        // How deep in a leaf block and in an image the parser is.
        let mut leaf = 0usize;
        let mut image = 0usize;
        // A list item's text that no paragraph holds, as in a tight list.
        let mut loose: Option<Range<usize>> = None;
        let mut notes: Vec<String> = Vec::new();
        // Whether this is a fence the app draws as a picture.
        let mut drawn = false;
        // Where the last callout's marker ended.
        let mut marker_end = 0usize;
        // Whether the parser is inside a comment in raw HTML.
        let mut in_comment = false;
        let literal = if message || !source.contains('<') {
            HashSet::new()
        } else {
            literal_tags(source)
        };
        for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
            let inline = match &event {
                Event::Start(tag) => !is_block(tag),
                Event::End(tag) => !ends_block(tag),
                Event::Rule => false,
                _ => true,
            };
            if leaf == 0 {
                if inline {
                    let start = loose.as_ref().map_or(range.start, |open| open.start);
                    loose = Some(start..range.end);
                } else if let Some(open) = loose.take() {
                    shown.block(source, open);
                }
            }
            match event {
                Event::Start(tag) => {
                    match &tag {
                        Tag::CodeBlock(CodeBlockKind::Fenced(info)) => drawn = draws_picture(info),
                        Tag::BlockQuote(_) => {
                            marker_end = alert_marker_end(source, range.start).unwrap_or(marker_end)
                        }
                        _ => {}
                    }
                    if is_leaf(&tag) {
                        if leaf == 0 {
                            shown.block(source, range.clone());
                        }
                        leaf += 1;
                    }
                    if matches!(tag, Tag::Image { .. }) {
                        image += 1;
                    } else if is_block(&tag) {
                        shown.separate();
                    }
                }
                Event::End(tag) => {
                    if tag == TagEnd::CodeBlock {
                        drawn = false;
                    }
                    if matches!(
                        tag,
                        TagEnd::Paragraph
                            | TagEnd::Heading(_)
                            | TagEnd::CodeBlock
                            | TagEnd::HtmlBlock
                            | TagEnd::Table
                    ) {
                        leaf = leaf.saturating_sub(1);
                    }
                    if tag == TagEnd::Image {
                        image = image.saturating_sub(1);
                    } else if ends_block(&tag) {
                        shown.separate();
                    }
                }
                _ if image > 0 || drawn => {}
                Event::Html(text) | Event::InlineHtml(text) if message => {
                    shown.piece(source, &text, range)
                }
                // In the sentence, so it needs no space around it.
                Event::InlineHtml(text) if !in_comment && literal.contains(&range.start) => {
                    shown.push(&text, range.start)
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    shown.html(&text, range.start, &literal, &mut in_comment)
                }
                _ if in_comment => {}
                Event::Text(_) if range.end <= marker_end => {}
                Event::Text(_) if range.start < marker_end => {
                    shown.push(&source[marker_end..range.end], marker_end)
                }
                Event::Text(text) | Event::Code(text) => shown.piece(source, &text, range),
                Event::InlineMath(_) if dollars_are_plain(source, &range) => {
                    shown.push(&source[range.clone()], range.start)
                }
                Event::FootnoteReference(label) => {
                    let number = match notes.iter().position(|seen| *seen == *label) {
                        Some(index) => index + 1,
                        None => {
                            notes.push(label.to_string());
                            notes.len()
                        }
                    };
                    shown.spread(&number.to_string(), range);
                }
                // A callout's marker takes its line with it.
                Event::SoftBreak if range.start <= marker_end => {}
                Event::SoftBreak => shown.spread("\n", range),
                Event::HardBreak | Event::Rule => shown.separate(),
                // Math and task list boxes show no text.
                _ => {}
            }
        }
        if let Some(open) = loose {
            shown.block(source, open);
        }
        shown.blocks.sort_by_key(|block| block.start);
        shown
    }

    /// The text the parser read from `range` of `source`, mapped character
    /// by character where the source holds it as it is, as plain text and
    /// code do, and spread over the range where it doesn't, as an escape or
    /// an entity.
    fn piece(&mut self, source: &str, text: &str, range: Range<usize>) {
        match source[range.clone()].find(text) {
            Some(offset) => self.push(text, range.start + offset),
            None => self.spread(text, range),
        }
    }

    /// The text raw HTML at byte `at` of the source shows: what's between
    /// its tags and comments, each piece its own block as near enough, with
    /// a `literal` tag read as part of the text around it.
    fn html(&mut self, html: &str, at: usize, literal: &HashSet<usize>, in_comment: &mut bool) {
        let mut from = 0;
        for (place, _) in html_markup(html, in_comment) {
            if !literal.contains(&(at + place.start)) {
                self.html_piece(&html[from..place.start], at + from);
                from = place.end;
            }
        }
        self.html_piece(&html[from..], at + from);
    }

    fn html_piece(&mut self, text: &str, at: usize) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.separate();
            self.push(trimmed, at + (text.len() - text.trim_start().len()));
            self.separate();
        }
    }

    /// `text`, which starts at byte `at` of the source.
    fn push(&mut self, text: &str, at: usize) {
        for (offset, c) in text.char_indices() {
            let start = at + offset;
            self.char(c, start..start + c.len_utf8());
        }
    }

    /// `text`, each character of which comes from all of `range`.
    fn spread(&mut self, text: &str, range: Range<usize>) {
        for c in text.chars() {
            self.char(c, range.clone());
        }
    }

    /// The line break between two blocks, which reads as a space once
    /// more text follows, as the app's row text puts it.
    fn separate(&mut self) {
        self.pending = true;
    }

    fn char(&mut self, c: char, source: Range<usize>) {
        if self.pending {
            self.pending = false;
            if let Some(last) = self.units.last() {
                let end = last.source.end;
                self.cut();
                self.put(' ', end..end);
            }
        }
        self.put(c, source);
    }

    fn put(&mut self, c: char, source: Range<usize>) {
        if self.skips_marks && c == REF_MARK {
            return;
        }
        if c == '\n' {
            self.cut();
        }
        let space = c.is_whitespace();
        if space && self.text.ends_with(' ') {
            return;
        }
        let c = if space { ' ' } else { c };
        self.units.push(Unit {
            at: self.text.len(),
            utf16: self.utf16,
            source,
        });
        self.text.push(c);
        self.utf16 += c.len_utf16();
    }

    /// Starts a new line of the shown text here.
    fn cut(&mut self) {
        let at = self.text.len();
        if at > 0 && self.cuts.last() != Some(&at) {
            self.cuts.push(at);
        }
    }

    /// Keeps `range` of `source` as a block, without the whitespace at its
    /// ends.
    fn block(&mut self, source: &str, range: Range<usize>) {
        let text = &source[range.clone()];
        let start = range.start + (text.len() - text.trim_start().len());
        let end = range.start + text.trim_end().len();
        if start < end {
            self.blocks.push(start..end);
        }
    }

    /// The shown text, each run of whitespace one space.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The shown text in the lines the app's row text breaks it into: each
    /// block on its own, cut again at a hard break and at each line break
    /// the text holds. A table's cells and a list's items are blocks too.
    /// A line keeps the space that stands for the break before it.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        let starts = std::iter::once(0).chain(self.cuts.iter().copied());
        let ends = self.cuts.iter().copied().chain([self.text.len()]);
        starts.zip(ends).map(|(start, end)| &self.text[start..end])
    }

    /// The source's leaf blocks as byte ranges, in order: a paragraph, a
    /// heading, a list item's text, a code block, a table or an HTML block.
    /// In plain text, a run of lines between blank lines.
    pub fn blocks(&self) -> &[Range<usize>] {
        &self.blocks
    }

    /// The unit that starts at byte `at` of the shown text, or the count
    /// of units at its end.
    fn unit_at(&self, at: usize) -> usize {
        self.units
            .binary_search_by_key(&at, |unit| unit.at)
            .unwrap_or_else(|index| index)
    }

    /// Where `quote` sits in the source, as a byte range: the place its
    /// text shows up nearest to where it started, as the app's
    /// `findQuote` picks it. A tie goes to the earlier place. None when the
    /// text isn't there.
    pub fn find(&self, quote: &Quote) -> Option<Range<usize>> {
        let needle = normalize(&quote.text);
        if needle.trim().is_empty() {
            return None;
        }
        let position = quote.position as usize;
        let mut found: Option<(usize, usize)> = None;
        each_match(self.text.as_bytes(), needle.as_bytes(), |start| {
            let utf16 = self.units[self.unit_at(start)].utf16;
            if found.is_none_or(|(_, best)| utf16.abs_diff(position) < best.abs_diff(position)) {
                found = Some((start, utf16));
            }
            utf16 < position
        });
        let (start, _) = found?;
        let first = &self.units[self.unit_at(start)];
        let last = &self.units[self.unit_at(start + needle.len()) - 1];
        Some(first.source.start..last.source.end.max(first.source.start))
    }
}

/// Calls `each` with every place `needle` starts in `text`, overlaps too, in
/// order, until it returns false. It's one pass over the text however often
/// the needle repeats, since a search begun afresh after each match costs a
/// long needle's length every time.
fn each_match(text: &[u8], needle: &[u8], mut each: impl FnMut(usize) -> bool) {
    if needle.len() > text.len() {
        return;
    }
    // For each prefix of the needle, the longest shorter prefix that ends it.
    let mut overlap = vec![0; needle.len()];
    let mut matched = 0;
    for (at, byte) in needle.iter().enumerate().skip(1) {
        while matched > 0 && needle[matched] != *byte {
            matched = overlap[matched - 1];
        }
        if needle[matched] == *byte {
            matched += 1;
        }
        overlap[at] = matched;
    }
    matched = 0;
    for (at, byte) in text.iter().enumerate() {
        while matched > 0 && needle[matched] != *byte {
            matched = overlap[matched - 1];
        }
        if needle[matched] == *byte {
            matched += 1;
        }
        if matched == needle.len() {
            if !each(at + 1 - matched) {
                return;
            }
            matched = overlap[matched - 1];
        }
    }
}

/// `text` with each run of whitespace made one space.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if !c.is_whitespace() {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out
}

/// Where `quote`, which started at `position` of what `text` shows, sits
/// in `text`, as a byte range. `markdown` says whether the text shows as
/// Markdown, as a reply does, or as it is, as a command's output does.
pub fn locate(text: &str, markdown: bool, quote: &str, position: u32) -> Option<Range<usize>> {
    Shown::new(text, markdown).find(&Quote {
        text: quote.to_owned(),
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_shows_no_text_but_plain_dollars_do() {
        let shown = Shown::markdown("Mass $E = mc^2$ is energy.\n\n$$\nx^2\n$$\n\nAfter");
        assert_eq!(shown.text(), "Mass is energy. After");
        assert_eq!(Shown::markdown("From $5-$10 now").text(), "From $5-$10 now");
        assert_eq!(Shown::markdown("a$b$c and $d$").text(), "a$b$c and ");
        assert_eq!(
            Shown::markdown("It cost $5 and $6").text(),
            "It cost $5 and $6"
        );
    }

    #[test]
    fn a_fence_drawn_as_a_picture_shows_no_text_but_keeps_its_block() {
        let source = "Before\n\n```mermaid\ngraph TD; A-->B\n```\n\n```MATH\nx\n```\n\nAfter";
        let shown = Shown::markdown(source);
        assert_eq!(shown.text(), "Before After");
        assert_eq!(shown.blocks().len(), 4);
        assert_eq!(Shown::markdown("```ts\nlet a\n```").text(), "let a ");
    }

    #[test]
    fn a_callout_leaves_out_its_marker_and_keeps_its_title() {
        assert_eq!(
            Shown::markdown("> [!NOTE]\n> Mind this").text(),
            "Mind this"
        );
        assert_eq!(
            Shown::markdown("Before\n\n> [!warning]- Be careful\n> of this").text(),
            "Before Be careful of this"
        );
        assert_eq!(Shown::markdown("> [!NOPE]\n> Mind").text(), "[!NOPE] Mind");
        assert_eq!(Shown::markdown("> plain [!NOTE]").text(), "plain [!NOTE]");
        let source = "> [!TIP] Try it";
        let shown = Shown::markdown(source);
        let quote = Quote {
            text: "Try it".to_string(),
            position: 0,
        };
        assert_eq!(&source[shown.find(&quote).unwrap()], "Try it");
    }

    #[test]
    fn raw_html_in_a_reply_shows_the_text_between_its_tags() {
        assert_eq!(
            Shown::markdown("<details>\n<summary>More</summary>\n\nInside\n\n</details>\n\nAfter")
                .text(),
            "More Inside After"
        );
        assert_eq!(
            Shown::markdown("Press <kbd>Ctrl</kbd> now").text(),
            "Press Ctrl now"
        );
        assert_eq!(
            Shown::markdown("<div>Seen <!-- a\nb --> here</div>").text(),
            "Seen here"
        );
    }

    #[test]
    fn a_tag_in_a_reply_that_wont_draw_reads_as_written() {
        for (written, shown) in [
            (
                "Add a <style> block to the page, then run the tests.",
                "Add a <style> block to the page, then run the tests.",
            ),
            ("Vec<String> holds it", "Vec<String> holds it"),
            ("Keep <kbd>open", "Keep <kbd>open"),
            ("Close </kbd> here", "Close </kbd> here"),
            (
                "<script>\nalert(1)\n\nmore()\n</script>\n\nAfter <!-- a -->it",
                "<script> alert(1) more() </script> After it",
            ),
            (
                "<style>a{}</style>\n\n<div>Seen</div>",
                "<style>a{}</style> Seen",
            ),
        ] {
            assert_eq!(Shown::markdown(written).text(), shown, "{written}");
        }
    }

    #[test]
    fn a_quote_after_a_tag_that_reads_as_written_lands_on_it() {
        let source = "Vec<String> holds it, and <style> does too";
        let shown = Shown::markdown(source);
        let quote = Quote {
            text: "<style> does".to_string(),
            position: position("Vec<String> holds it, and <style> does too", "<style>", 0),
        };
        assert_eq!(&source[shown.find(&quote).unwrap()], "<style> does");
    }

    #[test]
    fn a_tag_that_opened_inside_an_unclosed_one_reads_as_written() {
        assert_eq!(
            Shown::markdown("<b>one <kbd>two</b> three").text(),
            "one <kbd>two three"
        );
    }

    #[test]
    fn a_message_keeps_raw_html_as_text() {
        assert_eq!(
            Shown::message("Keep <b>this</b> as typed").text(),
            "Keep <b>this</b> as typed"
        );
    }

    /// Where `quote` starts in `shown`, the text the app shows with each run
    /// of whitespace as one space, written out by hand the way the app's
    /// row text reads it: a reply's blocks and a hard break each put a
    /// space between their text. The position is that text's prefix in
    /// UTF-16 units, as `quoteAnchor` counts it. `nth` picks among repeats.
    fn position(shown: &str, quote: &str, nth: usize) -> u32 {
        let (at, _) = shown.match_indices(quote).nth(nth).unwrap();
        shown[..at].encode_utf16().count() as u32
    }

    #[test]
    fn every_place_a_needle_starts_is_found_overlaps_too() {
        let places = |text: &str, needle: &str| {
            let mut places = Vec::new();
            each_match(text.as_bytes(), needle.as_bytes(), |at| {
                places.push(at);
                true
            });
            places
        };
        assert_eq!(places("aaaa", "aa"), [0, 1, 2]);
        assert_eq!(places("abababa", "aba"), [0, 2, 4]);
        assert_eq!(places("aabaabaaab", "aab"), [0, 3, 7]);
        assert_eq!(places("héllo héllo", "é"), [1, 8]);
        assert!(places("abc", "abcd").is_empty());
        assert!(places("abc", "x").is_empty());
        // The same places as a search begun afresh at each one.
        let text = "abaabaababaaabaabaabaaabab".repeat(3);
        for needle in ["a", "ab", "aba", "abaab", "baab", "aabaab", "abaabaaba"] {
            let afresh: Vec<usize> = (0..text.len())
                .filter(|at| text[*at..].starts_with(needle))
                .collect();
            assert_eq!(places(&text, needle), afresh, "{needle}");
        }
    }

    #[test]
    fn a_long_quote_of_a_repeating_text_costs_one_pass() {
        let source = "a".repeat(60_000);
        let shown = Shown::new(&source, false);
        let quote = Quote {
            text: "a".repeat(30_000),
            position: u32::MAX,
        };
        let started = std::time::Instant::now();
        assert!(shown.find(&quote).is_some());
        // Starting afresh at each of the 30,000 places takes minutes.
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    /// What of `source` the range found for `quote` covers.
    fn found<'a>(source: &'a str, markdown: bool, quote: &str, position: u32) -> &'a str {
        let range = locate(source, markdown, quote, position).expect("the quote is there");
        &source[range]
    }

    /// The byte offset of the `nth` `text` in `source`.
    fn offset(source: &str, text: &str, nth: usize) -> usize {
        source.match_indices(text).nth(nth).unwrap().0
    }

    const STORY: &str = "# The road and the river\n\
        \n\
        She walked **slowly** and the dog followed, *sniffing* the grass.\n\
        \n\
        - First [the map](https://example.com/map) and the compass\n\
        - Then `cargo run` and the rest\n\
        \n\
        ```rust\n\
        let a = 1; // and the code\n\
        ```\n\
        \n\
        | Who | What |\n\
        | --- | --- |\n\
        | Ann | and the cat |\n\
        \n\
        At last the road and the river met.\n";

    /// The text the app shows for [`STORY`], whitespace runs as one.
    const STORY_SHOWN: &str = "The road and the river She walked slowly and the dog followed, \
        sniffing the grass. First the map and the compass Then cargo run and the rest \
        let a = 1; // and the code Who What Ann and the cat At last the road and the river met.";

    #[test]
    fn the_shown_text_reads_the_way_the_app_shows_it() {
        assert_eq!(Shown::markdown(STORY).text(), STORY_SHOWN);
    }

    #[test]
    fn a_repeated_quote_lands_on_the_one_its_position_names() {
        let count = STORY_SHOWN.matches("and the").count();
        assert_eq!(count, 7);
        for nth in 0..count {
            let at = position(STORY_SHOWN, "and the", nth);
            let range = locate(STORY, true, "and the", at).unwrap();
            assert_eq!(&STORY[range.clone()], "and the", "repeat {nth}");
            assert_eq!(range.start, offset(STORY, "and the", nth), "repeat {nth}");
        }
    }

    #[test]
    fn a_position_that_drifted_a_little_still_lands_nearest() {
        let at = position(STORY_SHOWN, "and the", 1);
        let range = locate(STORY, true, "and the", at + 3).unwrap();
        assert_eq!(range.start, offset(STORY, "and the", 1));
        let range = locate(STORY, true, "and the", at.saturating_sub(3)).unwrap();
        assert_eq!(range.start, offset(STORY, "and the", 1));
    }

    #[test]
    fn a_quote_across_markup_covers_the_markup_in_the_source() {
        let quote = "walked slowly and the dog";
        let at = position(STORY_SHOWN, quote, 0);
        assert_eq!(
            found(STORY, true, quote, at),
            "walked **slowly** and the dog"
        );
        let quote = "the map and the compass Then cargo";
        let at = position(STORY_SHOWN, quote, 0);
        assert_eq!(
            found(STORY, true, quote, at),
            "the map](https://example.com/map) and the compass\n- Then `cargo"
        );
    }

    #[test]
    fn a_quote_across_lines_matches_with_its_whitespace_as_one() {
        let source = "One line\nand   the next\n\nAnother block.";
        assert_eq!(found(source, true, "line and the", 4), "line\nand   the");
        assert_eq!(found(source, true, "next Another", 17), "next\n\nAnother");
    }

    #[test]
    fn a_quote_that_isnt_there_is_none() {
        assert_eq!(locate(STORY, true, "and a horse", 0), None);
        assert_eq!(locate(STORY, true, "  ", 0), None);
        // Markup isn't shown text, so a quote of it isn't found.
        assert_eq!(locate(STORY, true, "**slowly**", 0), None);
    }

    #[test]
    fn positions_count_utf16_units() {
        // "é" is one UTF-16 unit and two bytes, and "🦀" is two units and
        // four bytes, so a byte or char count would land on the wrong one.
        let source = "Café 🦀 and the crab. Naïve 🦀🦀 and the crab.";
        let shown = source;
        for nth in 0..2 {
            let at = position(shown, "and the", nth);
            let range = locate(source, true, "and the", at).unwrap();
            assert_eq!(range.start, offset(source, "and the", nth));
        }
        assert_eq!(position(shown, "and the", 1), 33);
        let range = locate(source, false, "🦀🦀 and", 30).unwrap();
        assert_eq!(&source[range], "🦀🦀 and");
    }

    #[test]
    fn a_message_you_sent_shows_as_markdown_with_its_html_and_without_its_marks() {
        let source = format!("Why **does** {REF_MARK}use <Foo> here\nand `the` rest?");
        let shown = Shown::message(&source);
        assert_eq!(shown.text(), "Why does use <Foo> here and the rest?");
        assert_eq!(
            shown.lines().collect::<Vec<_>>(),
            ["Why does use <Foo> here", " and the rest?"]
        );
        let range = shown
            .find(&Quote {
                text: "does use".to_owned(),
                position: 4,
            })
            .expect("the quote is there");
        assert_eq!(&source[range], format!("does** {REF_MARK}use"));
        assert_eq!(
            Shown::message("<div>\nhi\n</div>").text().trim(),
            "<div> hi </div>"
        );
    }

    #[test]
    fn plain_text_shows_as_it_is() {
        let source = "Why **does** and the\n\n  cache and the rest?";
        let shown = "Why **does** and the cache and the rest?";
        assert_eq!(Shown::plain(source).text(), shown);
        let at = position(shown, "and the", 1);
        let range = locate(source, false, "and the", at).unwrap();
        assert_eq!(range.start, offset(source, "and the", 1));
        assert_eq!(found(source, false, "**does** and", 0), "**does** and");
        assert_eq!(Shown::plain(source).blocks(), [0..20, 24..source.len()]);
    }

    #[test]
    fn the_blocks_are_the_leaves_of_the_markdown() {
        let shown = Shown::markdown(STORY);
        let blocks: Vec<&str> = shown
            .blocks()
            .iter()
            .map(|block| &STORY[block.clone()])
            .collect();
        assert_eq!(
            blocks,
            [
                "# The road and the river",
                "She walked **slowly** and the dog followed, *sniffing* the grass.",
                "First [the map](https://example.com/map) and the compass",
                "Then `cargo run` and the rest",
                "```rust\nlet a = 1; // and the code\n```",
                "| Who | What |\n| --- | --- |\n| Ann | and the cat |",
                "At last the road and the river met.",
            ]
        );
        let loose = "- One\n\n  Two\n- Three\n\n> Quoted\n";
        let shown = Shown::markdown(loose);
        let blocks: Vec<&str> = shown
            .blocks()
            .iter()
            .map(|block| &loose[block.clone()])
            .collect();
        assert_eq!(blocks, ["One", "Two", "Three", "Quoted"]);
    }

    #[test]
    fn the_lines_break_where_the_row_text_does() {
        let shown = Shown::markdown(STORY);
        let lines: Vec<&str> = shown.lines().map(str::trim).collect();
        assert_eq!(
            lines,
            [
                "The road and the river",
                "She walked slowly and the dog followed, sniffing the grass.",
                "First the map and the compass",
                "Then cargo run and the rest",
                "let a = 1; // and the code",
                "",
                "Who",
                "What",
                "Ann",
                "and the cat",
                "At last the road and the river met.",
            ]
        );
        assert_eq!(shown.lines().collect::<String>(), STORY_SHOWN);
        let plain = Shown::plain("one\ntwo  three\n\nfour");
        let lines: Vec<&str> = plain.lines().collect();
        assert_eq!(lines, ["one", " two three", " ", "four"]);
    }

    #[test]
    fn images_and_task_boxes_show_no_text() {
        let source =
            "- [x] done <b>bold</b> and ![a cat](cat.png) end\n\n<div>\nraw\n</div>\n\nLast";
        assert_eq!(Shown::markdown(source).text(), "done bold and end raw Last");
    }

    #[test]
    fn an_escape_or_entity_maps_to_its_whole_source() {
        let source = "Tom &amp; Jerry \\* and the rest";
        assert_eq!(Shown::markdown(source).text(), "Tom & Jerry * and the rest");
        assert_eq!(found(source, true, "& Jerry *", 4), "&amp; Jerry \\*");
    }
}
