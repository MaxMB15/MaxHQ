// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

//! Parses a document part into a small tree, ported from the `xml` helper
//! in MIT-licensed code, see THIRD_PARTY_NOTICES, which used saxes.
//! Elements keep their local names, and their attributes can be found by
//! prefixed or local name. EPUB parts know a short list of HTML entities
//! rather than the full table, and keep any other entity in their text as
//! written.

use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::extract::Stop;

/// The most elements a part can have.
const MAX_NODES: usize = 200_000;

/// How deep elements can nest, counting the tree's root.
const MAX_DEPTH: usize = 128;

/// The HTML entities EPUB parts can use: Latin-1 and the common
/// typographic ones.
const HTML_ENTITIES: &[(&str, &str)] = &[
    ("nbsp", "\u{a0}"),
    ("iexcl", "\u{a1}"),
    ("cent", "\u{a2}"),
    ("pound", "\u{a3}"),
    ("curren", "\u{a4}"),
    ("yen", "\u{a5}"),
    ("brvbar", "\u{a6}"),
    ("sect", "\u{a7}"),
    ("uml", "\u{a8}"),
    ("copy", "\u{a9}"),
    ("ordf", "\u{aa}"),
    ("laquo", "\u{ab}"),
    ("not", "\u{ac}"),
    ("shy", "\u{ad}"),
    ("reg", "\u{ae}"),
    ("macr", "\u{af}"),
    ("deg", "\u{b0}"),
    ("plusmn", "\u{b1}"),
    ("sup2", "\u{b2}"),
    ("sup3", "\u{b3}"),
    ("acute", "\u{b4}"),
    ("micro", "\u{b5}"),
    ("para", "\u{b6}"),
    ("middot", "\u{b7}"),
    ("cedil", "\u{b8}"),
    ("sup1", "\u{b9}"),
    ("ordm", "\u{ba}"),
    ("raquo", "\u{bb}"),
    ("frac14", "\u{bc}"),
    ("frac12", "\u{bd}"),
    ("frac34", "\u{be}"),
    ("iquest", "\u{bf}"),
    ("Agrave", "\u{c0}"),
    ("Aacute", "\u{c1}"),
    ("Acirc", "\u{c2}"),
    ("Atilde", "\u{c3}"),
    ("Auml", "\u{c4}"),
    ("Aring", "\u{c5}"),
    ("AElig", "\u{c6}"),
    ("Ccedil", "\u{c7}"),
    ("Egrave", "\u{c8}"),
    ("Eacute", "\u{c9}"),
    ("Ecirc", "\u{ca}"),
    ("Euml", "\u{cb}"),
    ("Igrave", "\u{cc}"),
    ("Iacute", "\u{cd}"),
    ("Icirc", "\u{ce}"),
    ("Iuml", "\u{cf}"),
    ("ETH", "\u{d0}"),
    ("Ntilde", "\u{d1}"),
    ("Ograve", "\u{d2}"),
    ("Oacute", "\u{d3}"),
    ("Ocirc", "\u{d4}"),
    ("Otilde", "\u{d5}"),
    ("Ouml", "\u{d6}"),
    ("times", "\u{d7}"),
    ("Oslash", "\u{d8}"),
    ("Ugrave", "\u{d9}"),
    ("Uacute", "\u{da}"),
    ("Ucirc", "\u{db}"),
    ("Uuml", "\u{dc}"),
    ("Yacute", "\u{dd}"),
    ("THORN", "\u{de}"),
    ("szlig", "\u{df}"),
    ("agrave", "\u{e0}"),
    ("aacute", "\u{e1}"),
    ("acirc", "\u{e2}"),
    ("atilde", "\u{e3}"),
    ("auml", "\u{e4}"),
    ("aring", "\u{e5}"),
    ("aelig", "\u{e6}"),
    ("ccedil", "\u{e7}"),
    ("egrave", "\u{e8}"),
    ("eacute", "\u{e9}"),
    ("ecirc", "\u{ea}"),
    ("euml", "\u{eb}"),
    ("igrave", "\u{ec}"),
    ("iacute", "\u{ed}"),
    ("icirc", "\u{ee}"),
    ("iuml", "\u{ef}"),
    ("eth", "\u{f0}"),
    ("ntilde", "\u{f1}"),
    ("ograve", "\u{f2}"),
    ("oacute", "\u{f3}"),
    ("ocirc", "\u{f4}"),
    ("otilde", "\u{f5}"),
    ("ouml", "\u{f6}"),
    ("divide", "\u{f7}"),
    ("oslash", "\u{f8}"),
    ("ugrave", "\u{f9}"),
    ("uacute", "\u{fa}"),
    ("ucirc", "\u{fb}"),
    ("uuml", "\u{fc}"),
    ("yacute", "\u{fd}"),
    ("thorn", "\u{fe}"),
    ("yuml", "\u{ff}"),
    ("OElig", "\u{152}"),
    ("oelig", "\u{153}"),
    ("Scaron", "\u{160}"),
    ("scaron", "\u{161}"),
    ("Yuml", "\u{178}"),
    ("fnof", "\u{192}"),
    ("circ", "\u{2c6}"),
    ("tilde", "\u{2dc}"),
    ("ensp", "\u{2002}"),
    ("emsp", "\u{2003}"),
    ("thinsp", "\u{2009}"),
    ("zwnj", "\u{200c}"),
    ("zwj", "\u{200d}"),
    ("lrm", "\u{200e}"),
    ("rlm", "\u{200f}"),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("sbquo", "\u{201a}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("bdquo", "\u{201e}"),
    ("dagger", "\u{2020}"),
    ("Dagger", "\u{2021}"),
    ("bull", "\u{2022}"),
    ("hellip", "\u{2026}"),
    ("permil", "\u{2030}"),
    ("prime", "\u{2032}"),
    ("Prime", "\u{2033}"),
    ("lsaquo", "\u{2039}"),
    ("rsaquo", "\u{203a}"),
    ("euro", "\u{20ac}"),
    ("trade", "\u{2122}"),
    ("larr", "\u{2190}"),
    ("uarr", "\u{2191}"),
    ("rarr", "\u{2192}"),
    ("darr", "\u{2193}"),
    ("harr", "\u{2194}"),
    ("minus", "\u{2212}"),
    ("ne", "\u{2260}"),
    ("le", "\u{2264}"),
    ("ge", "\u{2265}"),
    ("infin", "\u{221e}"),
    ("asymp", "\u{2248}"),
];

/// An element, with its children in document order.
#[derive(Debug)]
pub(super) struct Node {
    /// The name without its prefix.
    pub(super) name: String,
    /// Each attribute twice, under its prefixed and its local name.
    attrs: Vec<(String, String)>,
    /// The element's text and elements.
    pub(super) children: Vec<Child>,
    /// How many spaces an ODF `text:s` element stands for.
    pub(super) spaces: Option<u64>,
}

/// A piece of an element's content.
#[derive(Debug)]
pub(super) enum Child {
    /// An element inside it.
    Node(Node),
    /// Text, with entities resolved and runs merged.
    Text(String),
}

impl Node {
    /// An element with the given name and nothing in it.
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            attrs: Vec::new(),
            children: Vec::new(),
            spaces: None,
        }
    }

    /// The tree a part that can't be read stands in with.
    pub(super) fn empty() -> Self {
        Self::new("root")
    }

    /// An attribute's value, by prefixed or local name. When names clash,
    /// the last one wins.
    pub(super) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The elements directly inside this one.
    pub(super) fn elements(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter_map(|child| match child {
            Child::Node(node) => Some(node),
            Child::Text(_) => None,
        })
    }

    /// Every element below this one with one of `names`, in document order,
    /// without looking inside the ones it finds.
    pub(super) fn all(&self, names: &[&str]) -> Vec<&Node> {
        let mut found = Vec::new();
        self.collect(names, &mut found);
        found
    }

    /// Adds the elements [`Node::all`] finds to `found`.
    fn collect<'a>(&'a self, names: &[&str], found: &mut Vec<&'a Node>) {
        for node in self.elements() {
            if names.contains(&node.name.as_str()) {
                found.push(node);
            } else {
                node.collect(names, found);
            }
        }
    }

    /// The first element below this one called `name`.
    pub(super) fn first(&self, name: &str) -> Option<&Node> {
        self.elements().find_map(|node| {
            if node.name == name {
                Some(node)
            } else {
                node.first(name)
            }
        })
    }

    /// The first run of text directly inside this element.
    pub(super) fn first_text(&self) -> Option<&str> {
        self.children.iter().find_map(|child| match child {
            Child::Text(text) => Some(text.as_str()),
            Child::Node(_) => None,
        })
    }

    /// Adds text, joining it to text just before it.
    fn push_text(&mut self, text: &str) {
        if let Some(Child::Text(last)) = self.children.last_mut() {
            last.push_str(text);
        } else {
            self.children.push(Child::Text(text.to_owned()));
        }
    }
}

/// Parses `source` into a tree under a root called `root`. With `odf`,
/// `text:s` elements record how many spaces they stand for. With `html`,
/// text can use the HTML entities this module knows.
pub(super) fn parse(source: &str, odf: bool, html: bool) -> Result<Node, Stop> {
    let mut reader = Reader::from_str(source);
    let mut stack = vec![Node::empty()];
    let mut nodes = 0;
    let mut roots = 0;
    loop {
        let event = reader.read_event().map_err(invalid)?;
        match event {
            Event::Start(tag) | Event::Empty(tag) if stack.len() == 1 && roots > 0 => {
                return Err(rejected(&format!(
                    "A second root element, {}",
                    tag.name().0
                )));
            }
            Event::Start(tag) => {
                nodes += 1;
                let node = open(&tag, nodes, stack.len(), odf, html)?;
                roots += usize::from(stack.len() == 1);
                stack.push(node);
            }
            Event::Empty(tag) => {
                nodes += 1;
                let node = open(&tag, nodes, stack.len(), odf, html)?;
                roots += usize::from(stack.len() == 1);
                let parent = stack.last_mut().expect("the root");
                parent.children.push(Child::Node(node));
            }
            Event::End(_) => {
                let node = stack.pop().expect("an open element");
                let parent = stack
                    .last_mut()
                    .ok_or_else(|| rejected("An unmatched end tag"))?;
                parent.children.push(Child::Node(node));
            }
            Event::Text(text) => add_text(&mut stack, &text.xml10_content())?,
            Event::CData(data) => add_text(&mut stack, &data.xml10_content())?,
            Event::GeneralRef(reference) => {
                if reference.is_char_ref() {
                    let c = reference
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .ok_or_else(|| rejected("An invalid character reference"))?;
                    add_text(&mut stack, c.encode_utf8(&mut [0; 4]))?;
                } else if let Some(value) = entity(&reference, html) {
                    add_text(&mut stack, value)?;
                } else if html {
                    add_text(&mut stack, &format!("&{};", &*reference))?;
                } else {
                    return Err(rejected("An unknown entity"));
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
        }
    }
    if stack.len() != 1 || roots != 1 {
        return Err(rejected("An unclosed or missing root element"));
    }
    Ok(stack.pop().expect("the root"))
}

/// Builds the element a start tag opens, after checking the part's limits.
fn open(tag: &BytesStart, nodes: usize, depth: usize, odf: bool, html: bool) -> Result<Node, Stop> {
    if nodes > MAX_NODES || depth > MAX_DEPTH {
        return Err(Stop::Truncated);
    }
    let mut node = Node::new(local(tag.name().0));
    for attr in tag.attributes() {
        let attr = attr.map_err(invalid)?;
        let resolve = |name: &str| entity(name, html);
        let value = match attr.normalized_value_with(XmlVersion::Implicit1_0, 1, resolve) {
            Ok(value) => value.into_owned(),
            // An EPUB keeps an entity it doesn't know as written.
            Err(_) if html => attr.value.into_owned(),
            Err(error) => return Err(invalid(error)),
        };
        let key = attr.key.0;
        node.attrs.push((key.to_owned(), value.clone()));
        node.attrs.push((local(key).to_owned(), value));
    }
    if odf && node.name == "s" {
        node.spaces = Some(repeat_count(node.attr("c"))?);
    }
    Ok(node)
}

/// Adds text to the open element. Outside the root, only whitespace can
/// appear.
fn add_text(stack: &mut [Node], text: &str) -> Result<(), Stop> {
    match stack {
        [_root] if text.trim_matches(['\t', '\n', '\r', ' ']).is_empty() => Ok(()),
        [_root] => Err(rejected("Text outside the root element")),
        [.., open] => {
            open.push_text(text);
            Ok(())
        }
        [] => Err(rejected("Text outside the root element")),
    }
}

/// The text a named entity stands for.
fn entity(name: &str, html: bool) -> Option<&'static str> {
    resolve_xml_entity(name).or_else(|| {
        html.then(|| HTML_ENTITIES.iter().find(|(key, _)| *key == name))
            .flatten()
            .map(|(_, value)| *value)
    })
}

/// A name without its prefix.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Reads a repeat count, such as ODF's `number-rows-repeated`. A count
/// that isn't there means one.
pub(super) fn repeat_count(value: Option<&str>) -> Result<u64, Stop> {
    let Some(value) = value else {
        return Ok(1);
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(rejected("Invalid repeat count"));
    }
    let count = value.bytes().fold(0u64, |count, digit| {
        count
            .saturating_mul(10)
            .saturating_add(u64::from(digit - b'0'))
    });
    if count < 1 {
        return Err(rejected("Invalid repeat count"));
    }
    Ok(count)
}

/// A rejection for XML that doesn't parse.
fn invalid(error: impl std::fmt::Display) -> Stop {
    Stop::Rejected(format!("Invalid XML: {error}"))
}

/// A rejection that names what's wrong with the XML.
fn rejected(reason: &str) -> Stop {
    Stop::Rejected(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text inside a tree, ignoring its structure.
    fn flat(node: &Node) -> String {
        node.children
            .iter()
            .map(|child| match child {
                Child::Text(text) => text.clone(),
                Child::Node(node) => flat(node),
            })
            .collect()
    }

    #[test]
    fn keeps_local_names_and_both_attribute_names() {
        let root = parse(
            r#"<w:p xmlns:w="w"><w:pStyle w:val="Heading2"/></w:p>"#,
            false,
            false,
        )
        .expect("a tree");
        let style = root.first("pStyle").expect("a style");
        assert_eq!(style.attr("w:val"), Some("Heading2"));
        assert_eq!(style.attr("val"), Some("Heading2"));
        assert_eq!(root.all(&["p"]).len(), 1);
    }

    #[test]
    fn resolves_entities_and_keeps_cdata() {
        let root =
            parse("<a>A &amp; B &#x3b1;<![CDATA[ &nbsp;]]></a>", false, false).expect("a tree");
        assert_eq!(flat(&root), "A & B \u{3b1} &nbsp;");
        assert!(matches!(
            parse("<a>&nbsp;</a>", false, false),
            Err(Stop::Rejected(_))
        ));
    }

    #[test]
    fn knows_html_entities_in_epub_parts() {
        let root = parse(
            "<a>caf&eacute;&nbsp;&mdash;&NotEqualTilde;</a>",
            false,
            true,
        )
        .expect("a tree");
        assert_eq!(flat(&root), "caf\u{e9}\u{a0}\u{2014}&NotEqualTilde;");
    }

    #[test]
    fn counts_odf_spaces() {
        let root = parse(r#"<p>a<text:s text:c="3"/>b<text:s/></p>"#, true, false).expect("a tree");
        let spaces: Vec<_> = root.all(&["s"]).iter().map(|node| node.spaces).collect();
        assert_eq!(spaces, [Some(3), Some(1)]);
        assert!(parse(r#"<p><s c="0"/></p>"#, true, false).is_err());
        assert!(parse(r#"<p><s c="x"/></p>"#, true, false).is_err());
    }

    #[test]
    fn stops_at_deep_or_large_trees() {
        let deep = format!("{}{}", "<div>".repeat(129), "</div>".repeat(129));
        assert!(matches!(parse(&deep, false, false), Err(Stop::Truncated)));
        let fine = format!("{}{}", "<div>".repeat(128), "</div>".repeat(128));
        assert!(parse(&fine, false, false).is_ok());
        let large = format!("<a>{}</a>", "<p/>".repeat(200_000));
        assert!(matches!(parse(&large, false, false), Err(Stop::Truncated)));
    }

    #[test]
    fn rejects_malformed_xml() {
        for source in [
            "",
            "<a>",
            "<a></b>",
            "<a/><b/>",
            "text<a/>",
            "<a b='&bogus;'/>",
        ] {
            assert!(
                matches!(parse(source, false, false), Err(Stop::Rejected(_))),
                "{source}"
            );
        }
        assert!(parse(" \n<a/>\n", false, false).is_ok());
    }

    #[test]
    fn reads_repeat_counts() {
        assert_eq!(repeat_count(None).ok(), Some(1));
        assert_eq!(repeat_count(Some("12")).ok(), Some(12));
        for value in ["", "0", "-1", "1.5", " 2"] {
            assert!(repeat_count(Some(value)).is_err(), "{value}");
        }
    }
}
