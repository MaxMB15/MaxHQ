// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How each Claude Code tool shows on a timeline. A tool call starts as an
//! item built from its input, and its result fills the item in.

use std::path::Path;

use maxhq_protocol::attention::{AttentionAnswer, AttentionOption};
use maxhq_protocol::ids::AttentionId;
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::timeline::{
    ApprovalKind, FileEdit, FileOp, ItemBody, ItemStatus, Question, QuestionAnswer, QuestionOption,
    Todo, TodoState, WebAction,
};
use serde_json::Value;

use super::artifacts::{self, Asked};
use super::wire::{cap, clip, content_text, flag, number, text, text_or_empty};

/// How much of a command's output an item keeps.
pub const OUTPUT_CAP: usize = 64 * 1024;

/// How much of a tool's input or output a generic item keeps.
const TOOL_TEXT_CAP: usize = 4 * 1024;

/// How long a title gets before it's cut.
const TITLE_CHARS: usize = 80;

/// The attention event for a tool call that asks you something.
pub fn attention(tool_use_id: &str) -> AttentionId {
    AttentionId::derive(&format!("claude:{tool_use_id}"))
}

/// Whether the tool starts a sub-agent. Older versions call it Task.
pub fn is_subagent(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

/// The server and tool in an MCP tool's name, `mcp__<server>__<tool>`.
pub fn mcp_parts(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.split_once("__")
}

/// The item for a tool call, from its input alone. Paths inside `cwd` show
/// relative to it.
pub fn start(name: &str, id: &str, input: &Value, cwd: &Path) -> ItemBody {
    match name {
        "Bash" => ItemBody::Command {
            command: text_or_empty(input, "command").to_owned(),
            cwd: None,
            exit_code: None,
            output: String::new(),
            output_truncated: false,
            background: flag(input, "run_in_background"),
        },
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ItemBody::FileChange {
            changes: vec![planned_edit(name, input)],
        },
        "Read" => ItemBody::FileRead {
            path: text_or_empty(input, "file_path").to_owned(),
            lines: read_range(number(input, "offset"), number(input, "limit")),
            images: Vec::new(),
        },
        "Grep" => ItemBody::Search {
            query: text_or_empty(input, "pattern").to_owned(),
            scope: text(input, "path")
                .or_else(|| text(input, "glob"))
                .map(str::to_owned),
            matches: None,
        },
        "Glob" => ItemBody::Search {
            query: text_or_empty(input, "pattern").to_owned(),
            scope: text(input, "path").map(str::to_owned),
            matches: None,
        },
        "WebFetch" => ItemBody::Web {
            action: WebAction::Fetch,
            target: text_or_empty(input, "url").to_owned(),
        },
        "WebSearch" => ItemBody::Web {
            action: WebAction::Search,
            target: text_or_empty(input, "query").to_owned(),
        },
        "TodoWrite" => ItemBody::Todos {
            todos: todos(input),
        },
        "ExitPlanMode" => ItemBody::Plan {
            markdown: text_or_empty(input, "plan").to_owned(),
            streaming: false,
        },
        "AskUserQuestion" => ItemBody::Question {
            attention: attention(id),
            questions: questions(input),
            answers: None,
        },
        _ if is_subagent(name) => ItemBody::Subagent {
            subagent: None,
            agent_type: text(input, "subagent_type").map(str::to_owned),
            description: text_or_empty(input, "description").to_owned(),
            result: None,
        },
        _ => {
            let (server, tool) = match mcp_parts(name) {
                Some((server, tool)) => (Some(server.to_owned()), tool),
                None => (None, name),
            };
            let artifact =
                artifacts::is_tool(name).then(|| Box::new(artifacts::start(name, input, cwd)));
            let title = match &artifact {
                Some(call) => Some(artifacts::line(name, input, call, false)),
                None => ["description", "skill", "query", "command"]
                    .into_iter()
                    .find_map(|key| text(input, key))
                    .map(|line| clip(line, TITLE_CHARS)),
            };
            let input = (!is_empty_object(input)).then(|| cap(&input.to_string(), TOOL_TEXT_CAP).0);
            ItemBody::ToolCall {
                server,
                name: tool.to_owned(),
                title,
                input,
                output: None,
                images: Vec::new(),
                artifact,
            }
        }
    }
}

/// What a result did to its item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// The call ended with this status.
    Ended(ItemStatus),
    /// The call carries on in the background, and a task event ends it.
    Background,
}

/// Fills in `body` from a tool's result. `result` is the CLI's structured
/// result, `content` the text the model saw, and `failed` its error flag.
pub fn finish(
    body: &mut ItemBody,
    input: &Value,
    result: &Value,
    content: &str,
    failed: bool,
) -> Finish {
    let ended = if failed {
        ItemStatus::Failed
    } else {
        ItemStatus::Completed
    };
    match body {
        ItemBody::Command {
            exit_code,
            output,
            output_truncated,
            background,
            ..
        } => {
            let printed = match result {
                Value::Object(_) => {
                    let stdout = text_or_empty(result, "stdout");
                    let stderr = text_or_empty(result, "stderr");
                    match (stdout.is_empty(), stderr.is_empty()) {
                        (_, true) => stdout.to_owned(),
                        (true, false) => stderr.to_owned(),
                        (false, false) => format!("{stdout}\n{stderr}"),
                    }
                }
                _ => strip_error(content).to_owned(),
            };
            (*output, *output_truncated) = cap(&printed, OUTPUT_CAP);
            if text(result, "backgroundTaskId").is_some() {
                *background = true;
                return Finish::Background;
            }
            if flag(result, "interrupted") {
                return Finish::Ended(ItemStatus::Interrupted);
            }
            *exit_code = if failed {
                exit_status(content).or(Some(1))
            } else {
                Some(0)
            };
            Finish::Ended(ended)
        }
        ItemBody::FileChange { changes } => {
            if let Some(change) = changes.first_mut()
                && result.is_object()
            {
                applied_edit(change, input, result);
            }
            Finish::Ended(ended)
        }
        ItemBody::FileRead { lines, .. } => {
            if let Some(file) = result.get("file")
                && let (Some(first), Some(count), Some(total)) = (
                    number(file, "startLine"),
                    number(file, "numLines"),
                    number(file, "totalLines"),
                )
                && count < total
                && count > 0
            {
                *lines = Some(format!("{first}-{}", first.saturating_add(count - 1)));
            }
            Finish::Ended(ended)
        }
        ItemBody::Search { matches, .. } => {
            *matches = ["numMatches", "numLines", "numFiles"]
                .into_iter()
                .find_map(|key| number(result, key))
                .map(|count| u32::try_from(count).unwrap_or(u32::MAX));
            Finish::Ended(ended)
        }
        ItemBody::Subagent { result: said, .. } => {
            if text(result, "status") == Some("async_launched") {
                return Finish::Background;
            }
            if !content.is_empty() {
                *said = Some(content.to_owned());
            }
            Finish::Ended(ended)
        }
        ItemBody::ToolCall {
            name,
            title,
            output,
            artifact,
            ..
        } => {
            if !content.is_empty() {
                *output = Some(cap(content, TOOL_TEXT_CAP).0);
            }
            if let Some(call) = artifact {
                artifacts::finish(call, result, failed);
                *title = Some(artifacts::line(name, input, call, !failed));
            }
            Finish::Ended(ended)
        }
        ItemBody::Question {
            questions, answers, ..
        } => {
            if answers.is_none() {
                *answers = given_answers(questions, result);
            }
            Finish::Ended(ended)
        }
        _ => Finish::Ended(ended),
    }
}

/// What a permission request is for.
pub fn approval_kind(name: &str) -> ApprovalKind {
    match name {
        "Bash" => ApprovalKind::Command,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ApprovalKind::FileChange,
        "Read" | "Grep" | "Glob" => ApprovalKind::FileRead,
        "WebFetch" | "WebSearch" => ApprovalKind::Network,
        "ExitPlanMode" => ApprovalKind::Plan,
        _ if artifacts::is_tool(name) => ApprovalKind::Network,
        _ => ApprovalKind::Tool,
    }
}

/// What a permission request's card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Its one line.
    pub title: String,
    /// More to show below the title.
    pub detail: Option<String>,
    /// Why the CLI asks.
    pub reason: Option<String>,
}

/// The card for a permission request for `name` with `input`. Paths inside
/// `cwd` show relative to it.
pub fn approval_card(name: &str, input: &Value, cwd: &Path, asked: &Asked) -> Card {
    if let Some(card) = artifacts::card(name, input, cwd, asked) {
        return card;
    }
    // A tool MaxHQ has no words for reads as the request's own title.
    let title = match (&asked.title, approval_kind(name)) {
        (Some(said), ApprovalKind::Tool) => said.clone(),
        _ => approval_title(name, input, cwd),
    };
    Card {
        title,
        detail: approval_detail(name, input, cwd),
        reason: asked.reason.clone(),
    }
}

/// The one line a permission request shows. Paths inside `cwd` show
/// relative to it.
fn approval_title(name: &str, input: &Value, cwd: &Path) -> String {
    let path = || clip_middle(&approval_path(input, cwd), TITLE_CHARS);
    match name {
        "Bash" => format!(
            "Run `{}`?",
            clip(text_or_empty(input, "command"), TITLE_CHARS)
        ),
        "Write" => format!("Write {}?", path()),
        "Edit" | "MultiEdit" | "NotebookEdit" => format!("Edit {}?", path()),
        "Read" => format!("Read {}?", path()),
        "Grep" | "Glob" => format!(
            "Search for `{}`?",
            clip(text_or_empty(input, "pattern"), TITLE_CHARS)
        ),
        "WebFetch" => format!(
            "Fetch {}?",
            clip_middle(text_or_empty(input, "url"), TITLE_CHARS)
        ),
        "WebSearch" => format!(
            "Search the web for {}?",
            clip(text_or_empty(input, "query"), TITLE_CHARS)
        ),
        "ExitPlanMode" => "Ready to code?".to_owned(),
        _ => match mcp_parts(name) {
            Some((server, tool)) => format!("Use {tool} from {server}?"),
            None => format!("Use {name}?"),
        },
    }
}

/// The path a file tool's request names.
fn approval_path(input: &Value, cwd: &Path) -> String {
    let path = text(input, "file_path")
        .or_else(|| text(input, "notebook_path"))
        .or_else(|| text(input, "path"))
        .unwrap_or_default();
    short_path(path, cwd)
}

/// `text` on one line. A control character shows as a replacement mark, so
/// blank lines can't push the rest of it out of view.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                char::REPLACEMENT_CHARACTER
            } else {
                c
            }
        })
        .collect()
}

/// `text` on one line, cut in the middle to at most `max` characters, so a
/// path or address always shows how it ends.
pub(super) fn clip_middle(text: &str, max: usize) -> String {
    let line = one_line(text);
    let count = line.chars().count();
    if count <= max {
        return line;
    }
    let head: String = line.chars().take(max / 2).collect();
    let tail: String = line.chars().skip(count - (max - max / 2)).collect();
    format!("{head}…{tail}")
}

/// The whole of `text` when a title cuts it, so the card can still show the
/// middle.
fn uncut(text: &str) -> Option<String> {
    let line = one_line(text);
    (line.chars().count() > TITLE_CHARS).then(|| cap(&line, TOOL_TEXT_CAP).0)
}

/// `path` relative to `cwd` when it's inside it.
pub(super) fn short_path(path: &str, cwd: &Path) -> String {
    match Path::new(path).strip_prefix(cwd) {
        Ok(inside) if !inside.as_os_str().is_empty() => inside.display().to_string(),
        _ => path.to_owned(),
    }
}

/// The folder a permission request is for: the tool's own `cwd` when it
/// has one, or else `cwd`.
pub fn approval_cwd(input: &Value, cwd: &Path) -> Option<String> {
    text(input, "cwd")
        .filter(|own| !own.is_empty())
        .map(str::to_owned)
        .or_else(|| Some(cwd.display().to_string()).filter(|cwd| !cwd.is_empty()))
}

/// More to show on a permission request than its title.
fn approval_detail(name: &str, input: &Value, cwd: &Path) -> Option<String> {
    match name {
        "Bash" => text(input, "command").map(str::to_owned),
        "WebFetch" => {
            let url = uncut(text_or_empty(input, "url"));
            let prompt = text(input, "prompt").map(str::to_owned);
            [url, prompt]
                .into_iter()
                .flatten()
                .reduce(|url, prompt| format!("{url}\n\n{prompt}"))
        }
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Read" => {
            uncut(&approval_path(input, cwd))
        }
        "ExitPlanMode" => None,
        _ if is_empty_object(input) => None,
        _ => Some(cap(&artifacts::shown_input(input).to_string(), TOOL_TEXT_CAP).0),
    }
}

/// What an approval's deny sends when you give no reason.
pub const DENIED: &str = "The user denied this request.";

/// What a plan's "keep planning" sends.
pub const KEEP_PLANNING: &str = "The user wants to keep planning.";

/// What a prompt you dismissed sends, as it stops the turn.
pub const DISMISSED: &str =
    "The user dismissed this request and stopped the turn. Wait for their next message.";

/// Whether a refused call's result says you dismissed its prompt.
pub fn dismissed(said: &str) -> bool {
    said.trim() == DISMISSED
}

/// How the CLI's own refusals start, for a prompt you answered in its
/// terminal or a sub-agent's prompt.
const REFUSALS: [&str; 3] = [
    "The user doesn't want to proceed with this tool use.",
    "Permission for this tool use was denied.",
    "The agent proposed a plan that was rejected by the user.",
];

/// What comes before the words you gave with a refusal in the CLI.
const REFUSAL_WORDS: [&str; 2] = ["the user said:\n", "The user said:\n"];

/// The options a permission prompt offers. Allowing for the session needs
/// a rule from the CLI to keep.
pub fn tool_options(session: bool) -> Vec<AttentionOption> {
    let option = |id: &str, label: &str| AttentionOption {
        id: id.to_owned(),
        label: label.to_owned(),
        kind: None,
    };
    let mut options = vec![option("allow", "Allow once")];
    if session {
        options.push(option("allow_session", "Allow for this session"));
    }
    options.push(option("deny", "Deny"));
    options
}

/// The mode a plan's option starts in.
pub fn plan_mode(option: &str) -> Option<PermissionMode> {
    match option {
        "accept_edits" => Some(PermissionMode::AcceptEdits),
        "accept" => Some(PermissionMode::Ask),
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// The options a finished plan offers.
pub fn plan_options() -> Vec<AttentionOption> {
    [
        ("accept_edits", "Yes, and accept edits"),
        ("accept", "Yes, and ask before each edit"),
        ("deny", "No, keep planning"),
    ]
    .into_iter()
    .map(|(id, label)| AttentionOption {
        id: id.to_owned(),
        label: label.to_owned(),
        kind: None,
    })
    .collect()
}

/// The answer a refused call's result shows: the words you gave, else a
/// plain deny, or none when you dismissed the prompt.
pub fn refusal_answer(said: &str) -> Option<AttentionAnswer> {
    let said = said.trim();
    if dismissed(said) {
        return None;
    }
    let answer = if let Some(words) = REFUSAL_WORDS
        .iter()
        .find_map(|marker| said.split_once(marker).map(|(_, words)| words.trim()))
        .filter(|words| !words.is_empty())
    {
        AttentionAnswer::Text {
            text: words.to_owned(),
        }
    } else if said.is_empty()
        || said == DENIED
        || said == KEEP_PLANNING
        || REFUSALS.iter().any(|refusal| said.starts_with(refusal))
    {
        AttentionAnswer::Option {
            id: "deny".to_owned(),
        }
    } else {
        AttentionAnswer::Text {
            text: said.to_owned(),
        }
    };
    Some(answer)
}

/// The questions an AskUserQuestion call asks, numbered from `q1`.
pub fn questions(input: &Value) -> Vec<Question> {
    let Some(list) = input.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .enumerate()
        .map(|(at, question)| Question {
            id: format!("q{}", at + 1),
            header: text(question, "header").map(str::to_owned),
            text: text_or_empty(question, "question").to_owned(),
            options: question
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .map(|option| QuestionOption {
                            label: text_or_empty(option, "label").to_owned(),
                            description: text(option, "description").map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            multi: flag(question, "multiSelect"),
            other: true,
            secret: false,
            consent: None,
        })
        .collect()
}

/// The answers a finished AskUserQuestion call reports, keyed by question
/// text the way the CLI keys them.
fn given_answers(questions: &[Question], result: &Value) -> Option<Vec<QuestionAnswer>> {
    let given = result.get("answers")?.as_object()?;
    let answers = questions
        .iter()
        .filter_map(|question| {
            let values = given.get(&question.text)?.as_str()?;
            Some(QuestionAnswer {
                id: question.id.clone(),
                values: split_values(values, question),
            })
        })
        .collect();
    Some(answers)
}

/// The labels in a multi-select answer, which the CLI joins with ", ".
fn split_values(values: &str, question: &Question) -> Vec<String> {
    if !question.multi {
        return vec![values.to_owned()];
    }
    values.split(", ").map(str::to_owned).collect()
}

/// A to-do list from TodoWrite's input.
fn todos(input: &Value) -> Vec<Todo> {
    let Some(list) = input.get("todos").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .map(|todo| Todo {
            text: text_or_empty(todo, "content").to_owned(),
            state: match text(todo, "status") {
                Some("in_progress") => TodoState::InProgress,
                Some("completed") => TodoState::Done,
                _ => TodoState::Pending,
            },
        })
        .collect()
}

/// The lines a Read asks for, as "10-40".
fn read_range(offset: Option<u64>, limit: Option<u64>) -> Option<String> {
    match (offset, limit) {
        (None, None) => None,
        (first, Some(limit)) if limit > 0 => {
            let first = first.unwrap_or(1).max(1);
            Some(format!("{first}-{}", first.saturating_add(limit - 1)))
        }
        (first, _) => Some(format!("{}-", first.unwrap_or(1).max(1))),
    }
}

/// The edit a call means to make, before the CLI says what it did.
fn planned_edit(name: &str, input: &Value) -> FileEdit {
    let path = text(input, "file_path")
        .or_else(|| text(input, "notebook_path"))
        .unwrap_or_default()
        .to_owned();
    let mut patch = String::new();
    match name {
        "Write" => push_lines(&mut patch, '+', text_or_empty(input, "content")),
        "NotebookEdit" => push_lines(&mut patch, '+', text_or_empty(input, "new_source")),
        "MultiEdit" => {
            for edit in input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                push_lines(&mut patch, '-', text_or_empty(edit, "old_string"));
                push_lines(&mut patch, '+', text_or_empty(edit, "new_string"));
            }
        }
        _ => {
            push_lines(&mut patch, '-', text_or_empty(input, "old_string"));
            push_lines(&mut patch, '+', text_or_empty(input, "new_string"));
        }
    }
    let (additions, deletions) = count_changes(&patch);
    FileEdit {
        path,
        old_path: None,
        op: if name == "Write" {
            FileOp::Add
        } else {
            FileOp::Modify
        },
        additions,
        deletions,
        patch: (!patch.is_empty()).then_some(patch),
    }
}

/// Fills in an edit from what the CLI reports it did.
fn applied_edit(change: &mut FileEdit, input: &Value, result: &Value) {
    if let Some(path) = text(result, "filePath") {
        change.path = path.to_owned();
    }
    let created = text(result, "type") == Some("create");
    change.op = if created { FileOp::Add } else { FileOp::Modify };
    let hunks = result.get("structuredPatch").and_then(Value::as_array);
    let mut patch = String::new();
    for hunk in hunks.into_iter().flatten() {
        if !patch.is_empty() {
            patch.push('\n');
        }
        patch.push_str(&format!(
            "@@ -{},{} +{},{} @@",
            number(hunk, "oldStart").unwrap_or(0),
            number(hunk, "oldLines").unwrap_or(0),
            number(hunk, "newStart").unwrap_or(0),
            number(hunk, "newLines").unwrap_or(0),
        ));
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            patch.push('\n');
            patch.push_str(line.as_str().unwrap_or_default());
        }
    }
    if patch.is_empty() && created {
        let content = text(result, "content").or_else(|| text(input, "content"));
        let content = content.unwrap_or_default();
        let lines = content.lines().count();
        patch = format!("@@ -0,0 +1,{lines} @@");
        for line in content.lines() {
            patch.push_str("\n+");
            patch.push_str(line);
        }
    }
    if patch.is_empty() {
        return;
    }
    (change.additions, change.deletions) = count_changes(&patch);
    change.patch = Some(patch);
}

/// Adds each line of `text` to `patch` with `mark` in front.
fn push_lines(patch: &mut String, mark: char, text: &str) {
    for line in text.lines() {
        if !patch.is_empty() {
            patch.push('\n');
        }
        patch.push(mark);
        patch.push_str(line);
    }
}

/// The lines a patch adds and removes.
fn count_changes(patch: &str) -> (u32, u32) {
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in patch.lines() {
        if line.starts_with("@@") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => additions = additions.saturating_add(1),
            Some(b'-') => deletions = deletions.saturating_add(1),
            _ => {}
        }
    }
    (additions, deletions)
}

/// The exit status in a failed command's result, which starts "Exit code N".
fn exit_status(content: &str) -> Option<i32> {
    let rest = strip_error(content).strip_prefix("Exit code ")?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

/// `content` without the "Error: " the CLI puts in front of failures.
fn strip_error(content: &str) -> &str {
    content.strip_prefix("Error: ").unwrap_or(content)
}

/// Whether `value` is an object with nothing in it, or not an object.
fn is_empty_object(value: &Value) -> bool {
    value.as_object().is_none_or(serde_json::Map::is_empty)
}

/// The text a tool result block carries.
pub fn result_text(block: &Value) -> String {
    block.get("content").map(content_text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_command_keeps_its_output_and_exit_status() {
        let input = json!({"command": "cargo test", "description": "Run tests"});
        let mut body = start("Bash", "toolu_1", &input, Path::new("/w"));
        let result = json!({"stdout": "ok", "stderr": "warn", "interrupted": false});
        assert_eq!(
            finish(&mut body, &input, &result, "ok", false),
            Finish::Ended(ItemStatus::Completed)
        );
        let ItemBody::Command {
            output, exit_code, ..
        } = &body
        else {
            panic!("{body:?}");
        };
        assert_eq!(output, "ok\nwarn");
        assert_eq!(*exit_code, Some(0));

        let mut body = start("Bash", "toolu_2", &input, Path::new("/w"));
        let failed = json!("Error: Exit code 2\nno such file");
        let status = finish(
            &mut body,
            &input,
            &failed,
            "Exit code 2\nno such file",
            true,
        );
        assert_eq!(status, Finish::Ended(ItemStatus::Failed));
        let ItemBody::Command {
            output, exit_code, ..
        } = &body
        else {
            panic!("{body:?}");
        };
        assert_eq!(*exit_code, Some(2));
        assert_eq!(output, "Exit code 2\nno such file");
    }

    #[test]
    fn a_background_command_keeps_running() {
        let input = json!({"command": "sleep 60", "run_in_background": true});
        let mut body = start("Bash", "toolu_1", &input, Path::new("/w"));
        let result = json!({"stdout": "", "stderr": "", "backgroundTaskId": "b1"});
        assert_eq!(
            finish(&mut body, &input, &result, "", false),
            Finish::Background
        );
        assert!(matches!(
            body,
            ItemBody::Command {
                background: true,
                ..
            }
        ));
    }

    #[test]
    fn an_edit_takes_the_structured_patch() {
        let input = json!({"file_path": "/w/seed.txt", "old_string": "one", "new_string": "two"});
        let mut body = start("Edit", "toolu_1", &input, Path::new("/w"));
        let ItemBody::FileChange { changes } = &body else {
            panic!("{body:?}");
        };
        assert_eq!(changes[0].patch.as_deref(), Some("-one\n+two"));
        let result = json!({
            "filePath": "/w/seed.txt",
            "structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                "lines": ["-one", "+two", " hello"]}],
        });
        finish(&mut body, &input, &result, "", false);
        let ItemBody::FileChange { changes } = &body else {
            panic!("{body:?}");
        };
        assert_eq!(
            changes[0],
            FileEdit {
                path: "/w/seed.txt".into(),
                old_path: None,
                op: FileOp::Modify,
                additions: 1,
                deletions: 1,
                patch: Some("@@ -1,2 +1,2 @@\n-one\n+two\n hello".into()),
            }
        );
    }

    #[test]
    fn a_new_file_is_an_add_with_every_line() {
        let input = json!({"file_path": "/w/a.txt", "content": "ALPHA\nBETA\n"});
        let mut body = start("Write", "toolu_1", &input, Path::new("/w"));
        let result = json!({"type": "create", "filePath": "/w/a.txt", "content": "ALPHA\nBETA\n",
            "structuredPatch": [], "originalFile": null});
        finish(&mut body, &input, &result, "", false);
        let ItemBody::FileChange { changes } = &body else {
            panic!("{body:?}");
        };
        assert_eq!(changes[0].op, FileOp::Add);
        assert_eq!(changes[0].additions, 2);
        assert_eq!(
            changes[0].patch.as_deref(),
            Some("@@ -0,0 +1,2 @@\n+ALPHA\n+BETA")
        );
    }

    #[test]
    fn reads_searches_and_web_calls_get_their_own_kinds() {
        let input = json!({"file_path": "/w/lib.rs", "offset": 10, "limit": 31});
        assert_eq!(
            start("Read", "t", &input, Path::new("/w")),
            ItemBody::FileRead {
                path: "/w/lib.rs".into(),
                lines: Some("10-40".into()),
                images: Vec::new(),
            }
        );
        let input = json!({"pattern": "fn main", "path": "src"});
        let mut body = start("Grep", "t", &input, Path::new("/w"));
        finish(
            &mut body,
            &input,
            &json!({"mode": "files_with_matches", "numFiles": 3}),
            "",
            false,
        );
        assert_eq!(
            body,
            ItemBody::Search {
                query: "fn main".into(),
                scope: Some("src".into()),
                matches: Some(3)
            }
        );
        assert_eq!(
            start("WebSearch", "t", &json!({"query": "rust"}), Path::new("/w")),
            ItemBody::Web {
                action: WebAction::Search,
                target: "rust".into()
            }
        );
    }

    #[test]
    fn mcp_tools_name_their_server() {
        let input = json!({"libraryName": "serde"});
        let mut body = start(
            "mcp__plugin_context7_context7__resolve-library-id",
            "t",
            &input,
            Path::new("/w"),
        );
        finish(&mut body, &input, &json!([]), "Found serde", false);
        assert_eq!(
            body,
            ItemBody::ToolCall {
                server: Some("plugin_context7_context7".into()),
                name: "resolve-library-id".into(),
                title: None,
                input: Some(r#"{"libraryName":"serde"}"#.into()),
                output: Some("Found serde".into()),
                images: Vec::new(),
                artifact: None,
            }
        );
        assert_eq!(
            approval_title("mcp__github__get_issue", &json!({}), Path::new("/w")),
            "Use get_issue from github?"
        );
    }

    #[test]
    fn questions_and_their_answers_come_from_the_input_and_result() {
        let input = json!({"questions": [
            {"question": "Which color?", "header": "Color", "multiSelect": false,
             "options": [{"label": "Red", "description": "warm"}, {"label": "Blue"}]},
            {"question": "Which sizes?", "header": "Size", "multiSelect": true,
             "options": [{"label": "S"}, {"label": "M"}]},
        ]});
        let mut body = start("AskUserQuestion", "toolu_q", &input, Path::new("/w"));
        let result = json!({"answers": {"Which color?": "Blue", "Which sizes?": "S, M"}});
        finish(&mut body, &input, &result, "", false);
        let ItemBody::Question {
            attention: event,
            questions,
            answers,
        } = &body
        else {
            panic!("{body:?}");
        };
        assert_eq!(*event, attention("toolu_q"));
        assert_eq!(questions[0].id, "q1");
        assert_eq!(questions[0].options[0].description.as_deref(), Some("warm"));
        assert!(questions[1].multi);
        assert_eq!(
            answers.as_deref(),
            Some(
                &[
                    QuestionAnswer {
                        id: "q1".into(),
                        values: vec!["Blue".into()]
                    },
                    QuestionAnswer {
                        id: "q2".into(),
                        values: vec!["S".into(), "M".into()]
                    },
                ][..]
            )
        );
    }

    #[test]
    fn an_async_sub_agent_keeps_running_until_its_task_ends() {
        let input =
            json!({"description": "Find the parser", "subagent_type": "Explore", "prompt": "go"});
        let mut body = start("Agent", "t", &input, Path::new("/w"));
        let result = json!({"status": "async_launched", "agentId": "a1"});
        assert_eq!(
            finish(&mut body, &input, &result, "Launched", false),
            Finish::Background
        );
        let mut body = start("Task", "t", &input, Path::new("/w"));
        finish(
            &mut body,
            &input,
            &json!({"status": "completed"}),
            "It is in src/parse.rs",
            false,
        );
        assert_eq!(
            body,
            ItemBody::Subagent {
                subagent: None,
                agent_type: Some("Explore".into()),
                description: "Find the parser".into(),
                result: Some("It is in src/parse.rs".into()),
            }
        );
    }

    #[test]
    fn todos_and_plans_come_from_the_input() {
        let input = json!({"todos": [
            {"content": "Read", "status": "completed", "activeForm": "Reading"},
            {"content": "Write", "status": "in_progress", "activeForm": "Writing"},
            {"content": "Test", "status": "pending", "activeForm": "Testing"},
        ]});
        let ItemBody::Todos { todos } = start("TodoWrite", "t", &input, Path::new("/w")) else {
            panic!();
        };
        let states: Vec<_> = todos.iter().map(|todo| todo.state).collect();
        assert_eq!(
            states,
            [TodoState::Done, TodoState::InProgress, TodoState::Pending]
        );
        assert_eq!(
            start(
                "ExitPlanMode",
                "t",
                &json!({"plan": "1. Read"}),
                Path::new("/w")
            ),
            ItemBody::Plan {
                markdown: "1. Read".into(),
                streaming: false
            }
        );
    }

    #[test]
    fn approvals_get_short_titles() {
        let command = json!({"command": "ls -la"});
        let cwd = Path::new("/w");
        assert_eq!(approval_title("Bash", &command, cwd), "Run `ls -la`?");
        assert_eq!(
            approval_detail("Bash", &command, cwd).as_deref(),
            Some("ls -la")
        );
        let write = json!({"file_path": "/w/a.txt", "content": "x"});
        assert_eq!(approval_title("Write", &write, cwd), "Write a.txt?");
        let edit = json!({"file_path": "/elsewhere/b.txt"});
        assert_eq!(approval_title("Edit", &edit, cwd), "Edit /elsewhere/b.txt?");
        assert_eq!(approval_kind("Write"), ApprovalKind::FileChange);
        assert_eq!(approval_kind("WebFetch"), ApprovalKind::Network);
        assert_eq!(
            approval_title("ExitPlanMode", &json!({}), cwd),
            "Ready to code?"
        );
    }

    #[test]
    fn a_path_with_blank_lines_or_a_long_middle_still_shows_where_it_ends() {
        let cwd = Path::new("/w");
        let path = format!(
            "/w/notes.txt{}/../../.ssh/authorized_keys",
            "\n".repeat(500)
        );
        for (name, verb, key) in [
            ("Write", "Write", "file_path"),
            ("Edit", "Edit", "file_path"),
            ("MultiEdit", "Edit", "file_path"),
            ("NotebookEdit", "Edit", "notebook_path"),
            ("Read", "Read", "file_path"),
        ] {
            let input = json!({ key: path });
            let title = approval_title(name, &input, cwd);
            assert!(title.starts_with(&format!("{verb} notes.txt")), "{title}");
            assert!(title.ends_with("/.ssh/authorized_keys?"), "{title}");
            assert!(!title.contains('\n'), "{title}");
            assert!(
                title.chars().count() <= verb.len() + 3 + TITLE_CHARS,
                "{title}"
            );
            // The card still has the whole of it, on one line.
            let detail = approval_detail(name, &input, cwd).unwrap();
            assert!(detail.ends_with("/../../.ssh/authorized_keys"), "{detail}");
            assert_eq!(detail.chars().count(), path.chars().count() - 3);
            assert!(!detail.contains('\n'), "{detail}");
        }
    }

    #[test]
    fn a_short_path_keeps_its_title_and_has_no_detail() {
        let cwd = Path::new("/w");
        let deep = format!("/w/{}", "folder/".repeat(20));
        let inside = json!({"file_path": format!("{deep}a.txt")});
        assert_eq!(approval_detail("Edit", &inside, Path::new(&deep)), None);
        let write = json!({"file_path": "/w/a.txt"});
        assert_eq!(approval_detail("Write", &write, cwd), None);
        let lined = json!({"file_path": "/w/a\nb.txt"});
        assert_eq!(
            approval_title("Write", &lined, cwd),
            "Write a\u{fffd}b.txt?"
        );
        assert_eq!(approval_detail("Write", &lined, cwd), None);
    }

    #[test]
    fn a_long_address_is_cut_in_the_middle_and_stays_on_the_card() {
        let cwd = Path::new("/w");
        let url = format!("https://example.com/{}/end?leak=1", "x".repeat(200));
        let input = json!({"url": url, "prompt": "Read it"});
        let title = approval_title("WebFetch", &input, cwd);
        assert!(title.starts_with("Fetch https://example.com/"), "{title}");
        assert!(title.ends_with("/end?leak=1?"), "{title}");
        assert!(
            title.chars().count() <= "Fetch ?".len() + 1 + TITLE_CHARS,
            "{title}"
        );
        assert_eq!(
            approval_detail("WebFetch", &input, cwd),
            Some(format!("{url}\n\nRead it"))
        );

        let short = json!({"url": "https://example.com/a", "prompt": "Read it"});
        assert_eq!(
            approval_title("WebFetch", &short, cwd),
            "Fetch https://example.com/a?"
        );
        assert_eq!(
            approval_detail("WebFetch", &short, cwd).as_deref(),
            Some("Read it")
        );
    }

    #[test]
    fn a_huge_read_request_does_not_overflow_its_range() {
        let top = u64::MAX;
        assert_eq!(read_range(Some(top), Some(2)), Some(format!("{top}-{top}")));
    }

    #[test]
    fn a_huge_read_result_does_not_overflow_its_range() {
        let top = u64::MAX;
        let input = json!({"file_path": "/w/lib.rs"});
        let mut body = start("Read", "toolu_1", &input, Path::new("/w"));
        let result = json!({"file": {"startLine": top, "numLines": 2, "totalLines": 5}});
        finish(&mut body, &input, &result, "", false);
        let ItemBody::FileRead { lines, .. } = &body else {
            panic!("{body:?}");
        };
        assert_eq!(lines.as_deref(), Some(format!("{top}-{top}").as_str()));
    }
}
