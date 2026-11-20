// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's guide, which the dispatcher searches to answer how-to questions.
//! build.rs builds in the pages of guide/ and its map, so the guide always
//! describes this version. Each `##` section of a page carries its id in a
//! comment, such as `<!-- id: ports/forward -->`, and names the map entries
//! it explains in another, such as `<!-- covers: settings/editor -->`; a
//! section comes back with the links that open those entries' views.
//! scripts/check-guide.mjs reads the sections the same way.

use std::collections::HashMap;
use std::sync::OnceLock;

use maxhq_core::timeline_find::fold;
use serde_json::{json, Value};

include!(concat!(env!("OUT_DIR"), "/guide.rs"));

/// The version the guide describes: maxd's own.
pub(super) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The most sections a search gives.
const HITS: usize = 4;

/// The longest excerpt, in characters.
const EXCERPT: usize = 240;

/// The most links a section gives.
const LINKS: usize = 4;

/// What a title hit counts for, against a hit in the text.
const TITLE: f64 = 3.0;

/// The sections of a search that count, against the best one's score.
const CUTOFF: f64 = 0.25;

/// Words a question is asked with, which say nothing of what it's about.
/// The guide speaks to "you", so "I" or "how" would otherwise count as
/// rare and pull up the few sections that quote a question.
const ASKING: &[&str] = &[
    "a", "an", "and", "any", "are", "be", "can", "could", "do", "does", "for", "how", "i", "if",
    "in", "is", "it", "me", "my", "of", "on", "or", "should", "that", "the", "there", "this", "to",
    "what", "when", "where", "which", "who", "why", "will", "with", "would", "you", "your",
];

/// Searches the built-in guide, for `search_guide`.
pub(super) fn search(query: &str) -> String {
    guide().search(query).to_string()
}

/// One section of the built-in guide, or a page's list of sections, for
/// `read_guide`.
pub(super) fn read(id: &str) -> Result<String, String> {
    guide().read(id).map(|answer| answer.to_string())
}

fn guide() -> &'static Guide {
    static GUIDE: OnceLock<Guide> = OnceLock::new();
    GUIDE.get_or_init(|| Guide::new(PAGES, MAP))
}

/// One `##` section of a page.
struct Section {
    id: String,
    title: String,
    /// Its page's title.
    page: String,
    /// Its text without the heading and the markers.
    text: String,
    covers: Vec<String>,
    /// The words of its title and of its text, folded, each once.
    title_words: Vec<String>,
    words: Vec<String>,
}

/// The guide, parsed once.
struct Guide {
    /// Each page's name and title, and where its sections start and end.
    pages: Vec<(String, String, std::ops::Range<usize>)>,
    sections: Vec<Section>,
    /// Each map entry's link, by its id.
    links: HashMap<String, String>,
    /// Each link's name: the first entry's title that has it, Settings
    /// pages and views before shortcuts and tools.
    names: HashMap<String, String>,
}

impl Guide {
    fn new(pages: &[(&str, &str)], map: &str) -> Guide {
        let mut sections = Vec::new();
        let mut parsed = Vec::new();
        for (name, text) in pages {
            let start = sections.len();
            let title = parse_page(text, &mut sections);
            parsed.push((name.to_string(), title, start..sections.len()));
        }
        let map: Value = serde_json::from_str(map).expect("the guide's map is JSON");
        let mut links = HashMap::new();
        let mut names = HashMap::new();
        for part in ["settings", "views", "shortcuts", "tools"] {
            for entry in map[part].as_array().into_iter().flatten() {
                let (Some(id), Some(title), Some(link)) = (
                    entry["id"].as_str(),
                    entry["title"].as_str(),
                    entry["link"].as_str(),
                ) else {
                    continue;
                };
                links.insert(id.to_owned(), link.to_owned());
                names
                    .entry(link.to_owned())
                    .or_insert_with(|| title.to_owned());
            }
        }
        Guide {
            pages: parsed,
            sections,
            links,
            names,
        }
    }

    /// The few sections that fit `query` best. Each of its words counts
    /// for how rare it is in the guide, more in a title than in the text.
    fn search(&self, query: &str) -> Value {
        let folded = fold(query);
        let mut asked: Vec<&str> = words(&folded)
            .filter(|word| !ASKING.contains(word))
            .collect();
        asked.sort_unstable();
        asked.dedup();
        let count = self.sections.len() as f64;
        let mut scores = vec![0.0; self.sections.len()];
        for word in asked {
            let title = |section: &Section| section.title_words.iter().any(|it| close(it, word));
            let text = |section: &Section| section.words.iter().any(|it| close(it, word));
            let hits: Vec<(usize, bool, bool)> = self
                .sections
                .iter()
                .enumerate()
                .map(|(index, section)| (index, title(section), text(section)))
                .filter(|(_, title, text)| *title || *text)
                .collect();
            let found = hits.len() as f64;
            let rarity = ((count - found + 0.5) / (found + 0.5) + 1.0).ln();
            for (index, title, text) in hits {
                scores[index] +=
                    rarity * (f64::from(u8::from(title)) * TITLE + f64::from(u8::from(text)));
            }
        }
        let mut ranked: Vec<usize> = (0..self.sections.len())
            .filter(|&i| scores[i] > 0.0)
            .collect();
        ranked.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
        let best = ranked.first().map_or(0.0, |&first| scores[first]);
        let found: Vec<Value> = ranked
            .into_iter()
            .take_while(|&index| scores[index] >= best * CUTOFF)
            .take(HITS)
            .map(|index| {
                let section = &self.sections[index];
                let mut hit = json!({
                    "id": section.id,
                    "title": section.title,
                    "page": section.page,
                    "excerpt": excerpt(&section.text),
                });
                self.add_links(&mut hit, section);
                hit
            })
            .collect();
        json!({"guide_version": VERSION, "sections": found})
    }

    /// The section with the id `id`, or the list of a page's sections when
    /// `id` names a page.
    fn read(&self, id: &str) -> Result<Value, String> {
        if let Some(section) = self.sections.iter().find(|section| section.id == id) {
            let mut answer = json!({
                "guide_version": VERSION,
                "id": section.id,
                "title": section.title,
                "page": section.page,
                "text": guide_links(&section.text),
            });
            self.add_links(&mut answer, section);
            return Ok(answer);
        }
        let (_, title, range) =
            self.pages
                .iter()
                .find(|(name, ..)| name == id)
                .ok_or_else(|| {
                    format!("The guide has no section `{id}`. Give an id search_guide gave.")
                })?;
        let sections: Vec<Value> = self.sections[range.clone()]
            .iter()
            .map(|section| json!({"id": section.id, "title": section.title}))
            .collect();
        Ok(json!({"guide_version": VERSION, "page": title, "sections": sections}))
    }

    /// Puts the links that open what `section` covers on `answer`, each
    /// once, with its name.
    fn add_links(&self, answer: &mut Value, section: &Section) {
        let mut links: Vec<&str> = Vec::new();
        for link in section.covers.iter().filter_map(|id| self.links.get(id)) {
            if links.len() < LINKS && !links.contains(&link.as_str()) {
                links.push(link);
            }
        }
        if !links.is_empty() {
            answer["links"] = links
                .into_iter()
                .map(|link| json!({"name": self.names[link], "link": link}))
                .collect();
        }
    }
}

/// A section as a page has it, before it's made searchable.
#[derive(Default)]
struct Draft<'a> {
    title: &'a str,
    id: Option<&'a str>,
    covers: Vec<&'a str>,
    lines: Vec<&'a str>,
}

/// Adds the sections of a page to `sections`, and gives its `#` title.
/// Headings and markers inside a fenced code block are text.
fn parse_page(text: &str, sections: &mut Vec<Section>) -> String {
    let mut page = "";
    let mut fenced = false;
    let mut drafts: Vec<Draft> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        if !fenced {
            if let Some(title) = line.strip_prefix("# ").filter(|_| page.is_empty()) {
                page = title.trim();
                continue;
            }
            if let Some(title) = line.strip_prefix("## ") {
                drafts.push(Draft {
                    title: title.trim(),
                    ..Draft::default()
                });
                continue;
            }
        }
        let Some(draft) = drafts.last_mut() else {
            continue;
        };
        match (!fenced).then(|| marker(trimmed)).flatten() {
            Some(("id", value)) => draft.id = Some(value),
            Some(("covers", value)) => draft.covers.extend(value.split_whitespace()),
            _ => draft.lines.push(line),
        }
    }
    for draft in drafts {
        // check-guide.mjs fails on a section without an id.
        let Some(id) = draft.id else { continue };
        let text = draft.lines.join("\n").trim().to_owned();
        sections.push(Section {
            id: id.to_owned(),
            title: draft.title.to_owned(),
            page: page.to_owned(),
            title_words: unique(words(&fold(draft.title))),
            words: unique(words(&fold(&text))),
            text,
            covers: draft.covers.into_iter().map(str::to_owned).collect(),
        });
    }
    page.to_owned()
}

/// A marker's name and value, from `<!-- name: value -->`.
fn marker(line: &str) -> Option<(&str, &str)> {
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?;
    let (name, value) = inner.split_once(':')?;
    Some((name.trim(), value.trim()))
}

/// The words of folded text: its runs of letters and digits.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn unique<'a>(words: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut words: Vec<String> = words.map(str::to_owned).collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// Whether a word of the guide stands for a word asked: the same, or with
/// an ending such as "ports" for "port" or "forwarding" for "forward".
fn close(word: &str, asked: &str) -> bool {
    let (short, long) = if word.len() <= asked.len() {
        (word, asked)
    } else {
        (asked, word)
    };
    long.strip_prefix(short).is_some_and(|ending| {
        ending.is_empty()
            || short.len() >= 3 && matches!(ending, "s" | "es" | "d" | "ed" | "ing" | "er" | "ers")
    })
}

/// A section's first paragraph, in plain text, cut short.
fn excerpt(text: &str) -> String {
    let paragraph = text.split("\n\n").next().unwrap_or_default();
    let plain = guide_links(paragraph).replace("**", "");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.chars().count() <= EXCERPT {
        return plain;
    }
    let cut: String = plain.chars().take(EXCERPT).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}...")
}

/// `text` with each link to another guide page, such as
/// `[Ports and forwards](ports.md)`, as its words and the page's name,
/// which read_guide takes.
fn guide_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        let (head, tail) = rest.split_at(start);
        let target = tail[2..].split_once(')').and_then(|(target, after)| {
            let page = target.strip_suffix(".md")?;
            let open = head.rfind('[')?;
            Some((page, open, after))
        });
        match target {
            Some((page, open, after)) => {
                out.push_str(&head[..open]);
                out.push_str(&format!("{} (guide page {page})", &head[open + 1..]));
                rest = after;
            }
            None => {
                out.push_str(head);
                out.push_str("](");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORTS: &str = "# Ports and forwards

## Forward a port
<!-- id: ports/forward -->
<!-- covers: settings/editor shortcut/split-right view/devices -->

Pick a **port** an agent listens on, and see [Devices and links](devices.md).

1. Click Forward.

## Ports never offered
<!-- id: ports/never -->

```md
## Not a section
```
";

    const AGENTS: &str = "# Agents

## Start an agent
<!-- id: agents/start -->

An agent can open a port, and you can forward that port to this Mac.
";

    const MAP: &str = r#"{
        "settings": [{"id": "settings/editor", "title": "Editor", "link": "maxhq://settings/editor"}],
        "shortcuts": [{"id": "shortcut/split-right", "title": "Split right", "link": "maxhq://settings/editor"}],
        "views": [{"id": "view/devices", "title": "Devices", "link": "maxhq://view/devices"}]
    }"#;

    fn fixture() -> Guide {
        Guide::new(&[("agents", AGENTS), ("ports", PORTS)], MAP)
    }

    #[test]
    fn a_title_hit_ranks_first_and_brings_its_links() {
        let found = fixture().search("How do I forward ports?");
        let sections = found["sections"].as_array().unwrap();
        assert_eq!(sections[0]["id"], "ports/forward");
        assert_eq!(sections[0]["page"], "Ports and forwards");
        assert_eq!(
            sections[0]["excerpt"],
            "Pick a port an agent listens on, and see Devices and links (guide page devices)."
        );
        assert_eq!(
            sections[0]["links"],
            json!([
                {"name": "Editor", "link": "maxhq://settings/editor"},
                {"name": "Devices", "link": "maxhq://view/devices"},
            ])
        );
        assert_eq!(sections[1]["id"], "agents/start");
        assert!(sections[1].get("links").is_none());
        assert_eq!(found["guide_version"], VERSION);
    }

    #[test]
    fn a_search_with_no_match_finds_nothing() {
        let found = fixture().search("kubernetes");
        assert_eq!(found["sections"], json!([]));
    }

    #[test]
    fn a_read_gives_a_section_or_a_page() {
        let guide = fixture();
        let section = guide.read("ports/never").unwrap();
        assert_eq!(section["title"], "Ports never offered");
        assert_eq!(section["text"], "```md\n## Not a section\n```");
        let page = guide.read("ports").unwrap();
        assert_eq!(
            page["sections"],
            json!([
                {"id": "ports/forward", "title": "Forward a port"},
                {"id": "ports/never", "title": "Ports never offered"},
            ])
        );
        assert!(guide.read("ports/gone").unwrap_err().contains("ports/gone"));
    }

    #[test]
    fn close_words_match_and_others_dont() {
        assert!(close("ports", "port"));
        assert!(close("forwarding", "forward"));
        assert!(!close("form", "format"));
        assert!(!close("set", "settings"));
    }

    /// The built-in guide parses, and a search finds what the map names.
    #[test]
    fn the_built_in_guide_answers() {
        let guide = guide();
        assert!(guide.sections.len() > 50);
        let found = search("split the pane to the right");
        assert!(found.contains("maxhq://settings/shortcuts"), "{found}");
        let found = guide.search("Check spelling");
        assert!(
            found["sections"][0]["links"]
                .to_string()
                .contains("maxhq://settings/editor"),
            "{found}"
        );
        assert!(read("index/pages").unwrap().contains("guide page settings"));
    }

    #[test]
    fn a_question_finds_its_section_near_the_top_of_a_short_answer() {
        let mut missed = Vec::new();
        for (query, wanted) in [
            ("How do I forward a port?", "ports/forward"),
            ("Where do I change the editor?", "settings/editor"),
            ("How do I schedule a task?", "scheduled-tasks/new"),
            ("Why is my agent waiting?", "troubleshooting/usage-limit"),
            ("How do I add a phone?", "phones/pair"),
        ] {
            let found = search(query);
            let answer: Value = serde_json::from_str(&found).unwrap();
            let top: Vec<&Value> = answer["sections"]
                .as_array()
                .unwrap()
                .iter()
                .take(2)
                .collect();
            if !top.iter().any(|section| section["id"] == wanted) {
                missed.push(format!("{query}: {found}"));
            }
            // About 400 tokens at most.
            assert!(found.len() < 1600, "{found}");
        }
        assert!(missed.is_empty(), "{missed:#?}");
        assert_eq!(
            search("How do I?"),
            json!({"guide_version": VERSION, "sections": []}).to_string()
        );
    }
}
