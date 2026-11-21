// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Claude Code's tools for pages on the user's claude.ai account:
//! Artifact, ArtifactData, ArtifactComments and ArtifactCheck. A call
//! shows as a tool row that names its page, and a permission request about
//! a page shows the question Claude Code's own dialog asks, over plain
//! lines instead of the input's JSON. The fields Claude Code puts on any
//! permission request are read here too.
//!
//! All text from the CLI goes through `clean` before it reaches an item,
//! and a link is kept only when `claude_link` says it's a claude.ai one.

use std::path::Path;

use maxhq_protocol::timeline::ArtifactCall;
use serde_json::Value;

use super::tools::{clip_middle, short_path, Card};
use super::wire::{cap, clip, flag, text};
use crate::doctext::strip_vt;

/// The four tools, as `--disallowedTools` takes them.
pub const TOOLS: &str = "Artifact,ArtifactComments,ArtifactData,ArtifactCheck";

/// Where Claude Code puts the asks a page call needs in its input: one
/// object, or a list of them when the call needs several.
const ASK: &str = "__artifactAsk";

/// How every key Claude Code adds to a call's input for itself starts.
const PRIVATE: &str = "__artifact";

/// How long a page's title or a short field gets.
const LINE_CHARS: usize = 120;

/// How long a request's own title or description gets.
const SAID_CHARS: usize = 200;

/// How long a reply, a prompt or a path gets on a card.
const TEXT_CHARS: usize = 1000;

/// How long the reason a request gives gets.
const REASON_CHARS: usize = 600;

/// How much of a card's detail is kept.
const DETAIL_CAP: usize = 4 * 1024;

/// The longest link kept.
const LINK_MAX: usize = 2048;

/// How long an action's name or a version gets.
const WORD_CHARS: usize = 40;

/// What a page is called when nothing names it.
const SOME_PAGE: &str = "a claude.ai page";

/// The note an ask about a page from another organization adds.
const OUTSIDE_ORG: &str = "This artifact was created outside your organization.";

/// Whether `name` is one of the four tools.
pub fn is_tool(name: &str) -> bool {
    TOOLS.split(',').any(|tool| tool == name)
}

/// `text` safe to show on one line: no terminal escape codes, no control
/// characters, no marks that hide text or turn it around, each run of
/// space made one, and at most `max` characters. None when nothing's left.
pub fn clean(text: &str, max: usize) -> Option<String> {
    let plain: String = strip_vt(text)
        .chars()
        .filter(|c| !hidden(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then(|| clip(&line, max))
}

/// Marks that show nothing themselves but can hide text or reverse it.
fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// `url` when it's a claude.ai link MaxHQ may offer to open: https, the
/// claude.ai host itself with no name, password or port before it, and
/// nothing after it that could end the link or hide another one.
pub fn claude_link(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://claude.ai/")?;
    let plain = rest.chars().all(|c| {
        c.is_ascii_graphic() && !matches!(c, '\\' | '"' | '<' | '>' | '`' | '{' | '}' | '|' | '^')
    });
    (plain && url.len() <= LINK_MAX).then_some(url)
}

/// The page part of a call, from its input alone.
pub fn start(tool: &str, input: &Value, cwd: &Path) -> ArtifactCall {
    let action = text(input, "action")
        .and_then(|action| clean(action, WORD_CHARS))
        .unwrap_or_else(|| if tool == "Artifact" { "publish" } else { "use" }.to_owned());
    ArtifactCall {
        detail: named(tool, &action, input, cwd, LINE_CHARS)
            .into_iter()
            .next()
            .map(|(_, value)| value),
        title: text(input, "title").and_then(|title| clean(title, LINE_CHARS)),
        url: text(input, "url").and_then(claude_link).map(str::to_owned),
        version: None,
        owned: None,
        action,
    }
}

/// Fills in a call's page part from its result, which wins over what the
/// input said. A failed call, or one whose result is empty, keeps what its
/// input said.
pub fn finish(call: &mut ArtifactCall, result: &Value, failed: bool) {
    if failed || !result.is_object() {
        return;
    }
    let found = [
        result,
        &result["read"],
        &result["file_read"],
        &result["written"],
        &result["file_list"],
    ];
    if let Some(title) = found
        .iter()
        .find_map(|at| text(at, "title"))
        .and_then(|title| clean(title, LINE_CHARS))
    {
        call.title = Some(title);
    }
    if let Some(url) = found
        .iter()
        .find_map(|at| text(at, "url"))
        .and_then(claude_link)
    {
        call.url = Some(url.to_owned());
    }
    let versions = [
        &result["version"],
        &result["artifactRead"]["ver"],
        &result["file_read"]["ver"],
        &result["file_list"]["ver"],
    ];
    if let Some(version) = versions.into_iter().find_map(version) {
        call.version = Some(version);
    }
    if let Some(owned) = result.get("owned").and_then(Value::as_bool) {
        call.owned = Some(owned);
    }
    if let Some(listed) = listed(result) {
        call.detail = Some(listed);
    }
}

/// A version as a result gives it, a number or a string.
fn version(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(version) => clean(version, WORD_CHARS),
        _ => None,
    }
}

/// How many pages or files a list found.
fn listed(result: &Value) -> Option<String> {
    match result.get("artifacts").and_then(Value::as_array) {
        Some(pages) => Some(count(pages.len(), "page", "pages")),
        None => {
            let files = result["file_list"].get("files")?.as_array()?;
            Some(count(files.len(), "file", "files"))
        }
    }
}

/// `count` things, as `1 page` or `3 pages`.
fn count(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// What else a call names besides its page, each with a label for a card:
/// the file it publishes or reads, the data it reads or writes, the reply
/// it posts, and the like. Each value is cut to `max` characters.
fn named(
    tool: &str,
    action: &str,
    input: &Value,
    cwd: &Path,
    max: usize,
) -> Vec<(&'static str, String)> {
    let field = |label: &'static str, key: &str| {
        text(input, key)
            .and_then(|value| clean(value, max))
            .map(|value| (label, value))
    };
    let found = match (tool, action) {
        ("ArtifactData", _) => vec![data(input, max).map(|data| ("Data", data))],
        ("ArtifactComments", "reply") => vec![field("Reply", "text")],
        (_, "list") => vec![field("Scope", "scope")],
        (_, "quickstart") => vec![field("Kind", "intent")],
        _ => vec![
            text(input, "file_path")
                .or_else(|| text(input, "path"))
                .and_then(|path| clean(&short_path(path, cwd), usize::MAX))
                .map(|path| ("File", clip_middle(&path, max))),
            field("Looking for", "prompt"),
        ],
    };
    found.into_iter().flatten().collect()
}

/// The data an ArtifactData call names: `collection/doc_id`, the
/// collection alone, or how many writes a batch makes and where.
fn data(input: &Value, max: usize) -> Option<String> {
    if let Some(writes) = input.get("writes").and_then(Value::as_array) {
        let writes = count(writes.len(), "write", "writes");
        return Some(match collection(input) {
            Some(collection) => format!("{collection}, {writes}"),
            None => writes,
        });
    }
    let collection = text(input, "collection")?;
    match text(input, "doc_id") {
        Some(doc) => clean(&format!("{collection}/{doc}"), max),
        None => clean(collection, max),
    }
}

/// The one collection an ArtifactData call reads or writes, none when a
/// batch writes to several.
fn collection(input: &Value) -> Option<String> {
    let named = match input.get("writes").and_then(Value::as_array) {
        Some(writes) => {
            let mut each = writes.iter().map(|write| text(write, "collection"));
            let first = each.next()??;
            each.all(|other| other == Some(first)).then_some(first)?
        }
        None => text(input, "collection")?,
    };
    clean(named, LINE_CHARS)
}

/// A page's title in quotes.
fn quoted(title: &str) -> String {
    format!("\"{title}\"")
}

/// The plain line a call's row shows, for older apps and the excerpt: in
/// the present while it runs, and in the past once it went well.
pub fn line(tool: &str, input: &Value, call: &ArtifactCall, done: bool) -> String {
    let page = call
        .title
        .as_deref()
        .map_or_else(|| SOME_PAGE.to_owned(), quoted);
    say(tool, input, call, &page, done)
}

/// What a call does to the page `page` names, in the present or the past.
fn say(tool: &str, input: &Value, call: &ArtifactCall, page: &str, done: bool) -> String {
    let data = || collection(input).unwrap_or_else(|| "the data".to_owned());
    let unnamed = call.title.is_none() && call.url.is_none();
    let (now, then, what) = match (tool, call.action.as_str()) {
        ("Artifact", "publish") => match &call.detail {
            Some(file) if unnamed => ("Publish", "Published", format!("{file} to claude.ai")),
            _ => ("Publish", "Published", page.to_owned()),
        },
        ("Artifact", "read") => ("Read", "Read", page.to_owned()),
        ("Artifact", "open") => ("Open", "Opened", page.to_owned()),
        ("Artifact", "delete") => ("Delete", "Deleted", page.to_owned()),
        ("Artifact", "pin") => ("Pin", "Pinned", page.to_owned()),
        ("Artifact", "unpin") => ("Unpin", "Unpinned", page.to_owned()),
        ("Artifact", "list") if unnamed => ("List", "Listed", "claude.ai pages".to_owned()),
        ("Artifact", "list") => ("List", "Listed", format!("the files in {page}")),
        ("Artifact", "quickstart") => (
            "Look up",
            "Looked up",
            "how to make a claude.ai page".to_owned(),
        ),
        ("ArtifactData", "get" | "list" | "query") => {
            ("Read", "Read", format!("{} in {page}", data()))
        }
        ("ArtifactData", "set" | "update" | "batch") => {
            ("Write to", "Wrote to", format!("{} in {page}", data()))
        }
        ("ArtifactData", "delete") => (
            "Delete from",
            "Deleted from",
            format!("{} in {page}", data()),
        ),
        ("ArtifactComments", "list" | "get") => ("Read", "Read", format!("the comments on {page}")),
        ("ArtifactComments", "reply") => ("Reply to", "Replied to", format!("a comment on {page}")),
        ("ArtifactComments", "resolve") => ("Resolve", "Resolved", format!("a comment on {page}")),
        ("ArtifactComments", "watch") => ("Watch", "Watched", format!("the comments on {page}")),
        ("ArtifactComments", "unwatch") => (
            "Stop watching",
            "Stopped watching",
            format!("the comments on {page}"),
        ),
        ("ArtifactComments", "status") => ("Check", "Checked", format!("the comments on {page}")),
        ("ArtifactCheck", "preview") => ("Preview", "Previewed", page.to_owned()),
        ("ArtifactCheck", _) => ("Check", "Checked", page.to_owned()),
        _ => ("Use", "Used", format!("{tool} on {page}")),
    };
    format!("{} {what}", if done { then } else { now })
}

/// What a permission request says about itself besides the call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// Its own title, on one line.
    pub title: Option<String>,
    /// Its description, on one line.
    pub description: Option<String>,
    /// Why the CLI asks, on one line.
    pub reason: Option<String>,
    /// Whether turning it down comes first, so one stray tap can't allow
    /// it.
    pub deny_first: bool,
    /// Whether it may not offer to keep a rule, even for the session.
    pub no_rule: bool,
    /// Whether only its card may answer it: no one-tap answer from a
    /// notification or the inbox.
    pub person_only: bool,
}

impl Asked {
    /// Reads the fields of a `can_use_tool` request.
    pub fn read(request: &Value) -> Self {
        let line = |key: &str, max: usize| text(request, key).and_then(|said| clean(said, max));
        Self {
            title: line("title", SAID_CHARS),
            description: line("description", SAID_CHARS),
            reason: line("decision_reason", REASON_CHARS),
            deny_first: flag(request, "default_to_no"),
            no_rule: flag(request, "suppress_always_allow_rule"),
            person_only: flag(request, "requires_user_interaction"),
        }
    }
}

/// The asks a call's input carries.
fn asks(input: &Value) -> Vec<&Value> {
    match input.get(ASK) {
        Some(Value::Array(asks)) => asks.iter().filter(|ask| ask.is_object()).collect(),
        Some(ask @ Value::Object(_)) => vec![ask],
        _ => Vec::new(),
    }
}

/// The question Claude Code's own dialog asks for one ask, word for word,
/// or None for a class it doesn't know.
fn question(ask: &Value) -> Option<String> {
    let title = text(ask, "title").and_then(|title| clean(title, LINE_CHARS));
    let (page, start) = match &title {
        Some(title) => (quoted(title), quoted(title)),
        None => ("this artifact".to_owned(), "This artifact".to_owned()),
    };
    let about = text(ask, "about");
    let missing = text(ask, "missing");
    let asked = match text(ask, "class")? {
        "delete" => {
            let link = text(ask, "url").and_then(claude_link);
            let what = match (&title, link) {
                (None, Some(link)) => clip_middle(link, LINE_CHARS),
                _ => page,
            };
            format!("Permanently delete {what}?")
        }
        "public_change" if text(ask, "basis") == Some("unattended") => {
            format!("Allow Claude to change {page}?")
        }
        "public_change" if about == Some("outside_audience") => {
            format!("{start} can be seen outside your organization. Allow Claude to change it?")
        }
        "public_change" if about == Some("someone_else") && missing == Some("owned") => {
            format!("Claude couldn't check who made {page}. Allow Claude to change it?")
        }
        "public_change" if about == Some("someone_else") => {
            format!("{start} was made by someone else. Allow Claude to change it?")
        }
        "public_change" if matches!(missing, Some("public" | "outsideAudience")) => {
            format!("Claude couldn't check who can see {page}. Allow Claude to change it?")
        }
        "public_change" => format!("{start} is public. Allow Claude to change it?"),
        "access_change" => format!("Allow Claude to change who can open or comment on {page}?"),
        "standing_job" => format!("Allow Claude to keep replying to comments on {page}?"),
        "foreign_read" if missing == Some("owned") => {
            format!("Claude couldn't check who made {page}. Let Claude read it?")
        }
        "foreign_read" => format!("{start} was made by someone else. Let Claude read it?"),
        _ => return None,
    };
    Some(match text(ask, "remember") {
        Some("artifact") => format!(
            "{} for the rest of this conversation?",
            asked.strip_suffix('?').unwrap_or(&asked)
        ),
        _ => asked,
    })
}

/// The card for a permission request about a page call, or about any call
/// whose input carries page asks. None leaves it to the usual card.
pub fn card(tool: &str, input: &Value, cwd: &Path, asked: &Asked) -> Option<Card> {
    let asks = asks(input);
    if asks.is_empty() && !is_tool(tool) {
        return None;
    }
    let call = start(tool, input, cwd);
    let link = call.url.as_deref().or_else(|| {
        asks.iter()
            .find_map(|ask| text(ask, "url").and_then(claude_link))
    });
    let page = call
        .title
        .as_deref()
        .map(quoted)
        .or_else(|| link.map(|link| clip_middle(link, LINE_CHARS)))
        .unwrap_or_else(|| SOME_PAGE.to_owned());
    let action = format!("{}?", say(tool, input, &call, &page, false));
    // An ask of a class Claude Code added later reads as the request's own
    // words, else as what the call does.
    let fallback = asked
        .title
        .as_ref()
        .or(asked.description.as_ref())
        .unwrap_or(&action);
    let mut questions: Vec<String> = Vec::new();
    for ask in &asks {
        let question = question(ask).unwrap_or_else(|| fallback.clone());
        if !questions.contains(&question) {
            questions.push(question);
        }
    }
    let title = match questions.as_slice() {
        [] => action,
        [only] => only.clone(),
        [first, rest @ ..] => format!("{first} (and {} more)", rest.len()),
    };

    let mut lines: Vec<String> = Vec::new();
    if questions.len() > 1 {
        lines.extend(questions.iter().cloned());
    }
    if asks
        .iter()
        .any(|ask| text(ask, "about") == Some("outside_org"))
    {
        lines.push(OUTSIDE_ORG.to_owned());
    }
    if let Some(link) = link.filter(|link| !title.contains(link)) {
        lines.push(link.to_owned());
    }
    if let Some(name) = call.title.as_deref().map(quoted)
        && !title.contains(&name)
    {
        lines.push(format!("Page: {name}"));
    }
    for (label, value) in named(tool, &call.action, input, cwd, TEXT_CHARS) {
        lines.push(format!("{label}: {value}"));
    }
    if !is_tool(tool) {
        let shown = shown_input(input);
        if shown.as_object().is_some_and(|fields| !fields.is_empty()) {
            lines.push(shown.to_string());
        }
    }
    let detail = (!lines.is_empty()).then(|| cap(&lines.join("\n"), DETAIL_CAP).0);
    // Claude Code's reason for a page ask is its questions again.
    let reason = asked
        .reason
        .clone()
        .filter(|reason| *reason != title && *reason != questions.join(" "));
    Some(Card {
        title,
        detail,
        reason,
    })
}

/// `input` without the keys Claude Code adds to it for itself, for
/// showing. The CLI still gets the input whole.
pub fn shown_input(input: &Value) -> Value {
    match input {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| !key.starts_with(PRIVATE))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::timeline::{ApprovalKind, ItemBody};
    use serde_json::json;

    use super::*;
    use crate::drive::claude::tools::{self, Finish};

    const CWD: &str = "/work/example-app";
    const PAGE: &str = "https://claude.ai/artifact/EXAMPLE1";
    const OTHER: &str = "https://claude.ai/artifact/EXAMPLE2";

    /// The row a call shows as it starts.
    fn started(tool: &str, input: &Value) -> ItemBody {
        tools::start(tool, "toolu_1", input, Path::new(CWD))
    }

    /// The row a call shows once `result` comes.
    fn ended(tool: &str, input: &Value, result: &Value, failed: bool) -> ItemBody {
        let mut body = started(tool, input);
        let finish = tools::finish(&mut body, input, result, "the result text", failed);
        assert!(matches!(finish, Finish::Ended(_)));
        body
    }

    /// A row's own title and page part.
    fn parts(body: &ItemBody) -> (String, ArtifactCall) {
        let ItemBody::ToolCall {
            server: None,
            title: Some(title),
            artifact: Some(call),
            ..
        } = body
        else {
            panic!("not a page row: {body:?}");
        };
        (title.clone(), (**call).clone())
    }

    fn call(action: &str) -> ArtifactCall {
        ArtifactCall {
            action: action.to_owned(),
            title: None,
            url: None,
            detail: None,
            version: None,
            owned: None,
        }
    }

    /// The card for a request with `fields` besides the call's.
    fn card_for(tool: &str, input: Value, fields: Value) -> Card {
        let asked = Asked::read(&fields);
        tools::approval_card(tool, &input, Path::new(CWD), &asked)
    }

    #[test]
    fn only_a_plain_claude_ai_link_is_kept() {
        assert_eq!(claude_link(PAGE), Some(PAGE));
        assert_eq!(
            claude_link("https://claude.ai/code/artifact/0199a7c2?v=2#top"),
            Some("https://claude.ai/code/artifact/0199a7c2?v=2#top")
        );
        for odd in [
            "http://claude.ai/artifact/EXAMPLE1",
            "https://claude.ai.example.com/artifact/EXAMPLE1",
            "https://evil@claude.ai/artifact/EXAMPLE1",
            "https://claude.ai@evil.example.com/artifact/EXAMPLE1",
            "https://claude.ai:8443/artifact/EXAMPLE1",
            "javascript:alert(1)//https://claude.ai/",
            "https://claude.ai/artifact/EXAMPLE1\nhttps://evil.example.com",
            "https://claude.ai/artifact/EXAMPLE1 click",
            "https://claude.ai/artifact/\"><script>",
            "https://claude.ai/\\\\evil.example.com",
            "HTTPS://CLAUDE.AI/artifact/EXAMPLE1",
            " https://claude.ai/artifact/EXAMPLE1",
            "claude.ai/artifact/EXAMPLE1",
        ] {
            assert_eq!(claude_link(odd), None, "{odd:?}");
        }
        let long = format!("https://claude.ai/{}", "a".repeat(LINK_MAX));
        assert_eq!(claude_link(&long), None);
    }

    #[test]
    fn text_from_the_cli_is_one_plain_line() {
        assert_eq!(
            clean("\u{1b}[31mWeekly\u{1b}[0m\n\n report\t\u{7}", 80).as_deref(),
            Some("Weekly report")
        );
        assert_eq!(
            clean("abc\u{202E}fdp.exe\u{2066}x\u{200B}", 80).as_deref(),
            Some("abcfdp.exex")
        );
        assert_eq!(clean(" \u{1b}[0m \n", 80), None);
        assert_eq!(clean("abcdef", 3).as_deref(), Some("abc…"));
    }

    #[test]
    fn a_publish_names_its_file_then_the_page_it_made() {
        let input = json!({
            "file_path": format!("{CWD}/scratch/report.html"),
            "files": {"img/chart.png": format!("{CWD}/scratch/chart.png")},
            "icon": "chart",
        });
        let body = started("Artifact", &input);
        let (title, call) = parts(&body);
        assert_eq!(title, "Publish scratch/report.html to claude.ai");
        assert_eq!(call.action, "publish");
        assert_eq!(call.detail.as_deref(), Some("scratch/report.html"));
        assert_eq!(call.url, None);
        // The row's input stays as the CLI gave it.
        let ItemBody::ToolCall {
            name, input: raw, ..
        } = &body
        else {
            unreachable!()
        };
        assert_eq!(name, "Artifact");
        assert!(raw.as_deref().unwrap().contains("report.html"));

        let result = json!({
            "url": PAGE,
            "path": format!("{CWD}/scratch/report.html"),
            "artifact_id": "0199a7c2-0000-7000-8000-00000000000a",
            "title": "Weekly report",
            "version": 2,
            "owned": true,
        });
        let body = ended("Artifact", &input, &result, false);
        let (title, call) = parts(&body);
        assert_eq!(title, "Published \"Weekly report\"");
        assert_eq!(call.title.as_deref(), Some("Weekly report"));
        assert_eq!(call.url.as_deref(), Some(PAGE));
        assert_eq!(call.version.as_deref(), Some("2"));
        assert_eq!(call.owned, Some(true));
        assert_eq!(call.detail.as_deref(), Some("scratch/report.html"));
        let ItemBody::ToolCall { output, .. } = &body else {
            unreachable!()
        };
        assert_eq!(output.as_deref(), Some("the result text"));
    }

    #[test]
    fn a_publish_outside_the_folder_keeps_its_path_and_an_update_names_its_page() {
        let input = json!({
            "file_path": "/tmp/scratch/launch.html",
            "url": PAGE,
            "title": "Launch checklist",
        });
        let (title, call) = parts(&started("Artifact", &input));
        assert_eq!(title, "Publish \"Launch checklist\"");
        assert_eq!(call.detail.as_deref(), Some("/tmp/scratch/launch.html"));
        assert_eq!(call.url.as_deref(), Some(PAGE));
    }

    #[test]
    fn a_failed_call_keeps_what_its_input_said() {
        let input = json!({"file_path": "report.html", "url": PAGE, "title": "Weekly report"});
        let result = json!({"url": OTHER, "title": "Something else", "version": 9});
        let body = ended("Artifact", &input, &result, true);
        let (title, call) = parts(&body);
        assert_eq!(title, "Publish \"Weekly report\"");
        assert_eq!(call.url.as_deref(), Some(PAGE));
        assert_eq!(call.title.as_deref(), Some("Weekly report"));
        assert_eq!(call.version, None);
    }

    #[test]
    fn a_read_takes_its_page_from_the_result_or_keeps_the_input() {
        let input = json!({"action": "read", "url": PAGE, "prompt": "the open questions"});
        let (title, call) = parts(&started("Artifact", &input));
        assert_eq!(title, "Read a claude.ai page");
        assert_eq!(call.action, "read");
        assert_eq!(call.detail.as_deref(), Some("the open questions"));

        let result = json!({
            "read": {"url": PAGE, "title": "Launch checklist"},
            "artifactRead": {"slug": "EXAMPLE1", "ver": "1794856476-0001"},
            "owned": false,
        });
        let body = ended("Artifact", &input, &result, false);
        let (title, call) = parts(&body);
        assert_eq!(title, "Read \"Launch checklist\"");
        assert_eq!(call.version.as_deref(), Some("1794856476-0001"));
        assert_eq!(call.owned, Some(false));

        // Some reads come back with no structured result.
        let body = ended("Artifact", &input, &Value::Null, false);
        let (title, call) = parts(&body);
        assert_eq!(title, "Read a claude.ai page");
        assert_eq!(call.url.as_deref(), Some(PAGE));
        assert_eq!(call.version, None);

        // A read of one file names that file.
        let input = json!({"action": "read", "url": PAGE, "path": "img/chart.png"});
        let result = json!({"file_read": {
            "url": PAGE, "title": "Launch checklist", "path": "img/chart.png", "ver": 3,
        }});
        let body = ended("Artifact", &input, &result, false);
        let (_, call) = parts(&body);
        assert_eq!(call.detail.as_deref(), Some("img/chart.png"));
        assert_eq!(call.version.as_deref(), Some("3"));
        assert_eq!(call.title.as_deref(), Some("Launch checklist"));
    }

    #[test]
    fn a_list_counts_the_pages_or_files_it_found() {
        let input = json!({"action": "list", "scope": "mine", "limit": 5});
        let (title, call) = parts(&started("Artifact", &input));
        assert_eq!(title, "List claude.ai pages");
        assert_eq!(call.detail.as_deref(), Some("mine"));
        let result = json!({"artifacts": [
            {"url": PAGE, "title": "Weekly report"},
            {"url": OTHER, "title": "Launch checklist"},
        ]});
        let body = ended("Artifact", &input, &result, false);
        let (title, call) = parts(&body);
        assert_eq!(title, "Listed claude.ai pages");
        assert_eq!(call.detail.as_deref(), Some("2 pages"));
        assert_eq!(call.url, None);

        let input = json!({"action": "list", "scope": "files", "url": PAGE});
        let result =
            json!({"file_list": {"url": PAGE, "ver": 4, "files": [{"path": "index.html"}]}});
        let body = ended("Artifact", &input, &result, false);
        let (title, call) = parts(&body);
        assert_eq!(title, "Listed the files in a claude.ai page");
        assert_eq!(call.detail.as_deref(), Some("1 file"));
        assert_eq!(call.version.as_deref(), Some("4"));
    }

    #[test]
    fn open_delete_pin_and_quickstart_read_plainly() {
        let input = json!({"action": "open", "url": PAGE});
        let result = json!({"opened": true, "url": PAGE, "title": "Weekly report", "owned": true});
        let (title, call) = parts(&ended("Artifact", &input, &result, false));
        assert_eq!(title, "Opened \"Weekly report\"");
        assert_eq!(call.owned, Some(true));

        let input = json!({"action": "delete", "url": PAGE});
        let result = json!({"deleted": true, "url": PAGE, "title": "Old draft"});
        let (title, _) = parts(&ended("Artifact", &input, &result, false));
        assert_eq!(title, "Deleted \"Old draft\"");

        let (title, _) = parts(&started(
            "Artifact",
            &json!({"action": "unpin", "url": PAGE}),
        ));
        assert_eq!(title, "Unpin a claude.ai page");

        let input = json!({"action": "quickstart", "intent": "slides"});
        let result = json!({"quickstart": {"intent": "slides"}});
        let (title, call) = parts(&ended("Artifact", &input, &result, false));
        assert_eq!(title, "Looked up how to make a claude.ai page");
        assert_eq!(call.detail.as_deref(), Some("slides"));
    }

    #[test]
    fn data_calls_name_the_collection_and_document() {
        let input = json!({"action": "list", "url": PAGE, "collection": "answers"});
        let result = json!({"db_read": {"op": "list", "collection": "answers", "docs": []}});
        let (title, call) = parts(&ended("ArtifactData", &input, &result, false));
        assert_eq!(title, "Read answers in a claude.ai page");
        assert_eq!(call.action, "list");
        assert_eq!(call.detail.as_deref(), Some("answers"));

        let input = json!({
            "action": "set", "url": PAGE, "collection": "notes",
            "doc_id": "2026-11-21", "data": {"text": "hello"},
        });
        let result = json!({"written": {"url": PAGE, "title": "Weekly report"}, "owned": true,
            "db_write": {"op": "set", "collection": "notes", "doc_id": "2026-11-21"}});
        let (title, call) = parts(&ended("ArtifactData", &input, &result, false));
        assert_eq!(title, "Wrote to notes in \"Weekly report\"");
        assert_eq!(call.detail.as_deref(), Some("notes/2026-11-21"));
        assert_eq!(call.owned, Some(true));

        let input = json!({"action": "batch", "url": PAGE, "writes": [
            {"op": "set", "collection": "notes", "doc_id": "a", "data": {}},
            {"op": "delete", "collection": "notes", "doc_id": "b"},
        ]});
        let (title, call) = parts(&started("ArtifactData", &input));
        assert_eq!(title, "Write to notes in a claude.ai page");
        assert_eq!(call.detail.as_deref(), Some("notes, 2 writes"));

        let input = json!({"action": "batch", "url": PAGE, "writes": [
            {"op": "set", "collection": "notes", "doc_id": "a", "data": {}},
            {"op": "set", "collection": "votes", "doc_id": "b", "data": {}},
        ]});
        let (title, call) = parts(&started("ArtifactData", &input));
        assert_eq!(title, "Write to the data in a claude.ai page");
        assert_eq!(call.detail.as_deref(), Some("2 writes"));

        let input = json!({"action": "delete", "url": PAGE, "collection": "notes", "doc_id": "a"});
        let (title, _) = parts(&ended("ArtifactData", &input, &json!({}), false));
        assert_eq!(title, "Deleted from notes in a claude.ai page");
    }

    #[test]
    fn comment_and_check_calls_say_what_they_do() {
        let input =
            json!({"action": "reply", "url": PAGE, "thread_id": "t1", "text": "Done, see v3."});
        let (title, call) = parts(&ended("ArtifactComments", &input, &json!({}), false));
        assert_eq!(title, "Replied to a comment on a claude.ai page");
        assert_eq!(call.detail.as_deref(), Some("Done, see v3."));
        let (title, _) = parts(&started(
            "ArtifactComments",
            &json!({"action": "unwatch", "url": PAGE}),
        ));
        assert_eq!(title, "Stop watching the comments on a claude.ai page");
        let (title, _) = parts(&started(
            "ArtifactComments",
            &json!({"action": "list", "url": PAGE}),
        ));
        assert_eq!(title, "Read the comments on a claude.ai page");
        let (title, _) = parts(&started(
            "ArtifactComments",
            &json!({"action": "archive", "url": PAGE}),
        ));
        assert_eq!(title, "Use ArtifactComments on a claude.ai page");
        let (title, _) = parts(&started(
            "ArtifactCheck",
            &json!({"action": "preview", "file_path": "a.html"}),
        ));
        assert_eq!(title, "Preview a claude.ai page");
    }

    #[test]
    fn an_odd_link_or_title_from_the_cli_is_cleaned_or_dropped() {
        let input = json!({"action": "read", "url": "https://claude.ai.example.com/artifact/X"});
        let result = json!({"read": {"url": "javascript:alert(1)", "title": "\u{1b}[1mWeekly\nreport\u{202E}"}});
        let (title, call) = parts(&ended("Artifact", &input, &result, false));
        assert_eq!(call.url, None);
        assert_eq!(call.title.as_deref(), Some("Weekly report"));
        assert_eq!(title, "Read \"Weekly report\"");
    }

    #[test]
    fn page_tools_ask_as_network_calls() {
        for tool in [
            "Artifact",
            "ArtifactData",
            "ArtifactComments",
            "ArtifactCheck",
        ] {
            assert_eq!(tools::approval_kind(tool), ApprovalKind::Network);
        }
        assert_eq!(tools::approval_kind("ArtifactX"), ApprovalKind::Tool);
    }

    /// The question one ask reads as.
    fn asked(ask: Value) -> Option<String> {
        question(&ask)
    }

    #[test]
    fn each_ask_reads_as_claude_codes_own_question() {
        let t = |fields: Value| {
            let mut ask = json!({"slug": "EXAMPLE1", "url": PAGE, "title": "Weekly report"});
            ask.as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            asked(ask).unwrap()
        };
        assert_eq!(
            t(json!({"class": "delete"})),
            "Permanently delete \"Weekly report\"?"
        );
        assert_eq!(
            t(json!({"class": "public_change"})),
            "\"Weekly report\" is public. Allow Claude to change it?"
        );
        assert_eq!(
            t(json!({"class": "public_change", "basis": "unattended", "about": "someone_else"})),
            "Allow Claude to change \"Weekly report\"?"
        );
        assert_eq!(
            t(json!({"class": "public_change", "about": "outside_audience"})),
            "\"Weekly report\" can be seen outside your organization. Allow Claude to change it?"
        );
        assert_eq!(
            t(json!({"class": "public_change", "about": "someone_else"})),
            "\"Weekly report\" was made by someone else. Allow Claude to change it?"
        );
        assert_eq!(
            t(json!({"class": "public_change", "about": "someone_else", "missing": "owned"})),
            "Claude couldn't check who made \"Weekly report\". Allow Claude to change it?"
        );
        assert_eq!(
            t(json!({"class": "public_change", "missing": "outsideAudience"})),
            "Claude couldn't check who can see \"Weekly report\". Allow Claude to change it?"
        );
        assert_eq!(
            t(json!({"class": "access_change"})),
            "Allow Claude to change who can open or comment on \"Weekly report\"?"
        );
        assert_eq!(
            t(json!({"class": "standing_job"})),
            "Allow Claude to keep replying to comments on \"Weekly report\"?"
        );
        assert_eq!(
            t(json!({"class": "foreign_read"})),
            "\"Weekly report\" was made by someone else. Let Claude read it?"
        );
        assert_eq!(
            t(json!({"class": "foreign_read", "missing": "owned"})),
            "Claude couldn't check who made \"Weekly report\". Let Claude read it?"
        );
        assert_eq!(
            t(json!({"class": "foreign_read", "remember": "artifact"})),
            "\"Weekly report\" was made by someone else. Let Claude read it for the rest of this conversation?"
        );
        assert_eq!(
            asked(json!({"class": "something_new", "title": "Weekly report"})),
            None
        );
        assert_eq!(asked(json!({"title": "Weekly report"})), None);

        // Untitled, a page is "this artifact", and a delete names its link.
        assert_eq!(
            asked(json!({"class": "foreign_read", "url": PAGE})).as_deref(),
            Some("This artifact was made by someone else. Let Claude read it?")
        );
        assert_eq!(
            asked(json!({"class": "standing_job"})).as_deref(),
            Some("Allow Claude to keep replying to comments on this artifact?")
        );
        assert_eq!(
            asked(json!({"class": "delete", "url": PAGE})),
            Some(format!("Permanently delete {PAGE}?"))
        );
        assert_eq!(
            asked(json!({"class": "delete", "url": "https://evil.example.com/x"})).as_deref(),
            Some("Permanently delete this artifact?")
        );
    }

    #[test]
    fn an_ask_shows_its_question_and_plain_lines_without_private_keys() {
        let input = json!({
            "action": "read",
            "url": PAGE,
            "prompt": "the open questions",
            "__artifactAsk": {
                "class": "foreign_read", "remember": "artifact", "slug": "EXAMPLE1",
                "url": PAGE, "title": "Launch checklist", "reachable": true, "askedIn": "main",
                "about": "outside_org",
            },
            "__artifactSeen": {"x": 1},
        });
        let question = "\"Launch checklist\" was made by someone else. Let Claude read it for the rest of this conversation?";
        let card = card_for(
            "Artifact",
            input,
            json!({
                "title": "Claude wants to read an artifact",
                "decision_reason": question,
                "decision_reason_type": "other",
            }),
        );
        assert_eq!(card.title, question);
        assert_eq!(
            card.detail.as_deref(),
            Some(&*format!(
                "{OUTSIDE_ORG}\n{PAGE}\nLooking for: the open questions"
            ))
        );
        // The reason only said the question again.
        assert_eq!(card.reason, None);
    }

    #[test]
    fn several_asks_show_the_first_and_list_them_all() {
        let input = json!({
            "file_path": format!("{CWD}/report.html"),
            "url": PAGE,
            "title": "Weekly report",
            "__artifactAsk": [
                {"class": "public_change", "url": PAGE, "title": "Weekly report", "slug": "EXAMPLE1"},
                {"class": "access_change", "url": PAGE, "title": "Weekly report", "slug": "EXAMPLE1"},
                {"class": "standing_job", "url": PAGE, "title": "Weekly report", "slug": "EXAMPLE1"},
            ],
        });
        let card = card_for(
            "Artifact",
            input,
            json!({"decision_reason": "\u{1b}[33mPublic page\u{1b}[0m and\r\nshared"}),
        );
        assert_eq!(
            card.title,
            "\"Weekly report\" is public. Allow Claude to change it? (and 2 more)"
        );
        assert_eq!(
            card.detail.as_deref(),
            Some(&*format!(
                "\"Weekly report\" is public. Allow Claude to change it?\n\
                 Allow Claude to change who can open or comment on \"Weekly report\"?\n\
                 Allow Claude to keep replying to comments on \"Weekly report\"?\n\
                 {PAGE}\n\
                 File: report.html"
            ))
        );
        assert_eq!(card.reason.as_deref(), Some("Public page and shared"));
    }

    #[test]
    fn an_ask_of_an_unknown_class_reads_as_the_requests_own_words() {
        let input = json!({
            "action": "delete", "url": PAGE,
            "__artifactAsk": {"class": "something_new", "url": PAGE},
        });
        let card = card_for(
            "Artifact",
            input.clone(),
            json!({"title": "Claude wants to\n\u{1b}[1mdelete\u{1b}[0m a page"}),
        );
        assert_eq!(card.title, "Claude wants to delete a page");
        let card = card_for(
            "Artifact",
            input.clone(),
            json!({"description": "Deletes a page"}),
        );
        assert_eq!(card.title, "Deletes a page");
        let card = card_for("Artifact", input, json!({}));
        assert_eq!(card.title, format!("Delete {PAGE}?"));
        assert_eq!(card.detail, None);
    }

    #[test]
    fn a_page_call_without_asks_says_what_it_does() {
        let input = json!({"action": "set", "url": PAGE, "collection": "notes", "doc_id": "a"});
        let card = card_for("ArtifactData", input, json!({}));
        assert_eq!(card.title, format!("Write to notes in {PAGE}?"));
        assert_eq!(card.detail.as_deref(), Some("Data: notes/a"));

        let input = json!({"action": "reply", "url": PAGE, "text": "Thanks!"});
        let card = card_for("ArtifactComments", input, json!({}));
        assert_eq!(card.title, format!("Reply to a comment on {PAGE}?"));
        assert_eq!(card.detail.as_deref(), Some("Reply: Thanks!"));

        let input = json!({"file_path": "report.html", "title": "Weekly report"});
        let card = card_for("Artifact", input, json!({}));
        assert_eq!(card.title, "Publish \"Weekly report\"?");
        assert_eq!(card.detail.as_deref(), Some("File: report.html"));

        let card = card_for("Artifact", json!({"action": "list"}), json!({}));
        assert_eq!(card.title, "List claude.ai pages?");
        assert_eq!(card.detail, None);
    }

    #[test]
    fn another_tool_with_an_ask_hides_the_private_keys() {
        let input = json!({
            "url": PAGE,
            "__artifactAsk": {"class": "foreign_read", "url": PAGE},
        });
        let card = card_for("WebFetch", input, json!({}));
        assert_eq!(
            card.title,
            "This artifact was made by someone else. Let Claude read it?"
        );
        let detail = card.detail.unwrap();
        assert!(!detail.contains("__artifact"), "{detail}");
        assert!(detail.contains(PAGE), "{detail}");
    }

    #[test]
    fn other_tools_keep_their_cards_and_gain_a_reason() {
        let fields = json!({
            "title": "Claude wants to run a command",
            "decision_reason": "\u{1b}[31mThis command\u{1b}[0m deletes files",
        });
        let card = card_for("Bash", json!({"command": "rm -rf build"}), fields.clone());
        assert_eq!(card.title, "Run `rm -rf build`?");
        assert_eq!(card.detail.as_deref(), Some("rm -rf build"));
        assert_eq!(card.reason.as_deref(), Some("This command deletes files"));

        // A tool MaxHQ doesn't know takes the request's own title.
        let card = card_for("Workflow", json!({"script": "x"}), fields);
        assert_eq!(card.title, "Claude wants to run a command");
        let card = card_for("Workflow", json!({"script": "x"}), json!({}));
        assert_eq!(card.title, "Use Workflow?");
        let card = card_for(
            "mcp__docs__search",
            json!({}),
            json!({"title": "Search\n the docs"}),
        );
        assert_eq!(card.title, "Search the docs");
        assert_eq!(card.reason, None);
    }

    #[test]
    fn a_request_says_how_it_may_be_answered() {
        let asked = Asked::read(&json!({
            "default_to_no": true,
            "suppress_always_allow_rule": true,
            "requires_user_interaction": true,
        }));
        assert!(asked.deny_first && asked.no_rule && asked.person_only);
        assert_eq!(Asked::read(&json!({})), Asked::default());
        let long = "why ".repeat(500);
        let reason = Asked::read(&json!({"decision_reason": long}))
            .reason
            .unwrap();
        assert!(
            reason.chars().count() <= REASON_CHARS + 1,
            "{}",
            reason.len()
        );
    }

    #[test]
    fn shown_input_drops_only_the_private_keys() {
        let input = json!({"url": PAGE, "__artifactAsk": {}, "__artifactX": 1, "_artifact": 2});
        assert_eq!(shown_input(&input), json!({"url": PAGE, "_artifact": 2}));
    }

    #[test]
    fn a_line_without_a_page_title_says_a_claude_ai_page() {
        let mut call = call("publish");
        assert_eq!(
            line("Artifact", &json!({}), &call, true),
            "Published a claude.ai page"
        );
        call.title = Some("Weekly report".to_owned());
        assert_eq!(
            line("Artifact", &json!({}), &call, true),
            "Published \"Weekly report\""
        );
    }
}
