// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The shell rules Cursor saved in its own settings, for
//! [`ClientMessage::CursorRules`](maxhq_protocol::ClientMessage::CursorRules).
//! The Cursor CLI keeps its settings in `cli-config.json`, in the folder
//! `CURSOR_CONFIG_DIR` names, else in `cursor` under `XDG_CONFIG_HOME`,
//! else in `~/.cursor`. Always allow in an earlier MaxHQ added
//! `Shell(...)` entries to the file's `permissions.allow` list, and Cursor
//! runs a command that starts with one without asking. `maxd` starts
//! Cursor with its own environment, so the same rule here finds the file
//! Cursor reads.
//!
//! This lists those entries and removes one by cutting its text out, so
//! every other key and entry, their order and spacing, and the file's mode
//! stay. The new file goes in place whole, and only when it reads back as
//! the old settings less that entry. A file in a shape this doesn't know
//! stays as it is. The server lets only the app and proven phones ask.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use maxhq_core::fs::{read_text, write_atomic, Atomic};
use maxhq_protocol::cursor_rules::{CursorRuleCall, CursorRules};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::message::ActionOutcome;
use serde_json::Value;
use tracing::warn;

/// The settings file's name in Cursor's folder.
const FILE: &str = "cli-config.json";

/// The largest settings file read. Cursor's holds a few hundred bytes.
const MOST: u64 = 1 << 20;

/// How long reading or rewriting the file may take before the answer
/// gives up, as on a home folder on a network disk that went away.
const ANSWER_IN: Duration = Duration::from_secs(5);

const OTHER_DEVICE: &str = "Those Cursor rules are on another device.";
const NO_HOME: &str = "maxd can't tell your home folder, so it can't find Cursor's settings.";
const RELATIVE: &str =
    "Cursor's settings folder is set to a relative path, so maxd can't tell where it is.";
const TOO_LONG: &str = "Reading Cursor's settings took too long.";
const UNFINISHED: &str = "maxd couldn't finish that. Its log says why.";

/// Keeps two removals from rewriting the file at once.
static WRITING: Mutex<()> = Mutex::new(());

/// Answers `CursorRules` for this device, `me`.
pub async fn answer(me: DeviceId, device: DeviceId, call: CursorRuleCall) -> ActionOutcome {
    if device != me {
        return refused(OTHER_DEVICE);
    }
    let home = std::env::home_dir();
    let dir = std::env::var_os("CURSOR_CONFIG_DIR");
    let xdg = std::env::var_os("XDG_CONFIG_HOME");
    let work = tokio::task::spawn_blocking(move || run(&call, dir, xdg, home.as_deref()));
    match tokio::time::timeout(ANSWER_IN, work).await {
        Ok(Ok(Ok(rules))) => ActionOutcome::CursorRules { rules },
        Ok(Ok(Err(reason))) => refused(reason),
        Ok(Err(err)) => {
            warn!(%err, "reading Cursor's rules ended without an answer");
            refused(UNFINISHED)
        }
        Err(_) => refused(TOO_LONG),
    }
}

/// Does `call` on the settings file the environment names.
fn run(
    call: &CursorRuleCall,
    dir: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<&Path>,
) -> Result<CursorRules, String> {
    let path = config_path(dir, xdg, home)?;
    let name = shown(&path, home);
    let rules = match call {
        CursorRuleCall::List => list(&path),
        CursorRuleCall::Remove { rule } => remove(&path, rule),
    };
    match rules {
        Ok(rules) => Ok(CursorRules { path: name, rules }),
        Err(problem) => Err(problem.reason(&name)),
    }
}

/// Where the Cursor CLI keeps its settings, in its config folder.
fn config_path(
    dir: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    Ok(config_dir(dir, xdg, home)?.join(FILE))
}

/// The Cursor CLI's config folder, found as it finds it: `dir`
/// (`CURSOR_CONFIG_DIR`) when that's set, else `cursor` under `xdg`
/// (`XDG_CONFIG_HOME`), else `.cursor` in the home folder.
pub(super) fn config_dir(
    dir: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    let set = |value: Option<OsString>| {
        value
            .filter(|value| !value.to_string_lossy().trim().is_empty())
            .map(PathBuf::from)
    };
    let dir = match (set(dir), set(xdg), home) {
        (Some(dir), _, _) => dir,
        (None, Some(xdg), _) => xdg.join("cursor"),
        (None, None, Some(home)) => home.join(".cursor"),
        (None, None, None) => return Err(NO_HOME.to_owned()),
    };
    // Cursor reads a relative one from the folder it runs in, which is
    // each agent's own.
    if !dir.is_absolute() {
        return Err(RELATIVE.to_owned());
    }
    Ok(dir)
}

/// `path` with `~` for the home folder it's in.
fn shown(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// Why a list or a removal didn't go through.
#[derive(Debug)]
enum Problem {
    NotJson,
    Shape,
    NotShell,
    Changed,
    Read(io::Error),
    Write(io::Error),
}

impl Problem {
    /// What the refusal says, for the file shown as `name`.
    fn reason(&self, name: &str) -> String {
        match self {
            Self::NotJson => format!("{name} isn't JSON maxd can read, so maxd leaves it alone."),
            Self::Shape => {
                format!("{name} isn't in a shape maxd knows, so maxd leaves it alone.")
            }
            Self::NotShell => format!("maxd removes only Shell(...) rules from {name}."),
            Self::Changed => format!("{name} changed while maxd read it. Try again."),
            Self::Read(err) => format!("maxd couldn't read {name}: {err}"),
            Self::Write(err) => format!("maxd couldn't save {name}: {err}"),
        }
    }
}

/// Whether an allow entry is a shell rule, such as `Shell(git status)`.
fn is_shell(entry: &str) -> bool {
    entry.starts_with("Shell(") && entry.ends_with(')')
}

/// The file's text, or none when it isn't there.
fn read(path: &Path) -> Result<Option<String>, Problem> {
    match read_text(path, MOST) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Problem::Read(err)),
    }
}

/// The settings in `text`, none for a blank file, which Cursor reads as
/// no settings.
fn parse(text: &str) -> Result<Option<Value>, Problem> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(text)
        .map(Some)
        .map_err(|_| Problem::NotJson)
}

/// The allow list in `settings`: an object whose `permissions`, when
/// there, is an object whose `allow`, when there, is a list of strings.
fn allowed(settings: &Value) -> Result<Vec<&str>, Problem> {
    let Value::Object(settings) = settings else {
        return Err(Problem::Shape);
    };
    let Some(permissions) = settings.get("permissions") else {
        return Ok(Vec::new());
    };
    let Value::Object(permissions) = permissions else {
        return Err(Problem::Shape);
    };
    let Some(allow) = permissions.get("allow") else {
        return Ok(Vec::new());
    };
    let Value::Array(allow) = allow else {
        return Err(Problem::Shape);
    };
    allow
        .iter()
        .map(|entry| entry.as_str().ok_or(Problem::Shape))
        .collect()
}

/// The shell rules in `text`, in its order.
fn shell_rules(text: &str) -> Result<Vec<String>, Problem> {
    let Some(settings) = parse(text)? else {
        return Ok(Vec::new());
    };
    Ok(allowed(&settings)?
        .into_iter()
        .filter(|entry| is_shell(entry))
        .map(str::to_owned)
        .collect())
}

/// The shell rules in the file at `path`. A file that isn't there has
/// none.
fn list(path: &Path) -> Result<Vec<String>, Problem> {
    match read(path)? {
        Some(text) => shell_rules(&text),
        None => Ok(Vec::new()),
    }
}

/// Takes `rule` out of the file at `path`, and gives the shell rules left.
fn remove(path: &Path, rule: &str) -> Result<Vec<String>, Problem> {
    if !is_shell(rule) {
        return Err(Problem::NotShell);
    }
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    // A link, as a dotfiles checkout makes, stays a link: the file it
    // names is the one rewritten.
    let path = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Problem::Read(err)),
    };
    let Some(text) = read(&path)? else {
        return Ok(Vec::new());
    };
    let Some(kept) = without(&text, rule)? else {
        return shell_rules(&text);
    };
    let mode = maxhq_os::fsmode::mode_of(&fs::metadata(&path).map_err(Problem::Read)?) & 0o777;
    // Cursor writes the whole file when you change a setting there, so
    // one that changed since the read above is left for another try.
    if read(&path)?.as_deref() != Some(text.as_str()) {
        return Err(Problem::Changed);
    }
    write_atomic(&path, kept.as_bytes(), Atomic::exact(mode)).map_err(Problem::Write)?;
    shell_rules(&kept)
}

/// `text` with each copy of `rule` cut out of its allow list, with the
/// comma and spacing that went with it, or none when it holds no `rule`.
/// The rest stays byte for byte, and the result must read as the same
/// settings less those entries.
fn without(text: &str, rule: &str) -> Result<Option<String>, Problem> {
    let Some(settings) = parse(text)? else {
        return Ok(None);
    };
    if !allowed(&settings)?.contains(&rule) {
        return Ok(None);
    }
    let mut kept = text.to_owned();
    loop {
        let spans = allow_spans(&kept)?;
        let found = spans.items.iter().position(|item| {
            serde_json::from_str::<String>(&kept[item.clone()])
                .ok()
                .as_deref()
                == Some(rule)
        });
        let Some(at) = found else { break };
        kept.replace_range(spans.cut(at), "");
    }
    let mut expected = settings;
    if let Some(Value::Array(allow)) = expected.pointer_mut("/permissions/allow") {
        allow.retain(|entry| entry.as_str() != Some(rule));
    }
    if parse(&kept)? != Some(expected) {
        return Err(Problem::Shape);
    }
    Ok(Some(kept))
}

/// Where the allow list is in a settings file's text, and its entries.
struct Spans {
    /// The list, from `[` to `]`.
    list: Range<usize>,
    /// Each entry.
    items: Vec<Range<usize>>,
}

impl Spans {
    /// The text to cut for the entry at `at`: up to the next entry, or
    /// from the one before for the last, or the inside of the brackets
    /// for the only one, which leaves `[]`.
    fn cut(&self, at: usize) -> Range<usize> {
        let items = &self.items;
        if items.len() == 1 {
            self.list.start + 1..self.list.end - 1
        } else if at + 1 < items.len() {
            items[at].start..items[at + 1].start
        } else {
            items[at - 1].end..items[at].end
        }
    }
}

/// Finds the allow list in `text`. A file with two `permissions` keys, or
/// two `allow` keys in it, is refused: which one counts is a guess.
fn allow_spans(text: &str) -> Result<Spans, Problem> {
    let one = |name: &str, object: Range<usize>| {
        let mut scan = Scan::at(text, object.start);
        let mut found = None;
        let mut twice = false;
        scan.object(|key, value| {
            if key == name {
                twice |= found.replace(value).is_some();
            }
            Ok(())
        })?;
        match found {
            Some(value) if !twice => Ok((scan, value)),
            _ => Err(Problem::Shape),
        }
    };
    let (mut scan, permissions) = one("permissions", 0..text.len())?;
    scan.space();
    if scan.at != text.len() {
        return Err(Problem::Shape);
    }
    let (_, list) = one("allow", permissions)?;
    let items = Scan::at(text, list.start).array()?;
    Ok(Spans { list, items })
}

/// A reader of JSON text that finds where each part is, for text
/// `serde_json` read already.
struct Scan<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Scan<'a> {
    fn at(text: &'a str, at: usize) -> Self {
        Self {
            text: text.as_bytes(),
            at,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    /// Steps over spacing.
    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// Steps over `byte`, after any spacing.
    fn eat(&mut self, byte: u8) -> Result<(), Problem> {
        self.space();
        if self.peek() != Some(byte) {
            return Err(Problem::Shape);
        }
        self.at += 1;
        Ok(())
    }

    /// Steps over a `,` and says there's more, or over `close` and says
    /// there isn't.
    fn more(&mut self, close: u8) -> Result<bool, Problem> {
        self.space();
        match self.peek() {
            Some(b',') => {
                self.at += 1;
                Ok(true)
            }
            Some(byte) if byte == close => {
                self.at += 1;
                Ok(false)
            }
            _ => Err(Problem::Shape),
        }
    }

    /// Steps over one value, and gives where it is.
    fn value(&mut self) -> Result<Range<usize>, Problem> {
        self.space();
        let start = self.at;
        match self.peek() {
            Some(b'"') => {
                self.at += 1;
                loop {
                    match self.peek() {
                        Some(b'"') => break,
                        Some(b'\\') => self.at += 2,
                        Some(_) => self.at += 1,
                        None => return Err(Problem::Shape),
                    }
                }
                self.at += 1;
            }
            Some(b'{') => self.object(|_, _| Ok(()))?,
            Some(b'[') => {
                self.array()?;
            }
            Some(_) => {
                while matches!(self.peek(), Some(byte) if !b",]} \t\n\r".contains(&byte)) {
                    self.at += 1;
                }
            }
            None => {}
        }
        if self.at == start || self.at > self.text.len() {
            return Err(Problem::Shape);
        }
        Ok(start..self.at)
    }

    /// Steps over an object, telling `each` every key and where its value
    /// is.
    fn object(
        &mut self,
        mut each: impl FnMut(String, Range<usize>) -> Result<(), Problem>,
    ) -> Result<(), Problem> {
        self.eat(b'{')?;
        self.space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(());
        }
        loop {
            let key = self.value()?;
            let key = serde_json::from_slice(&self.text[key]).map_err(|_| Problem::Shape)?;
            self.eat(b':')?;
            let value = self.value()?;
            each(key, value)?;
            if !self.more(b'}')? {
                return Ok(());
            }
        }
    }

    /// Steps over an array, and gives where each item is.
    fn array(&mut self) -> Result<Vec<Range<usize>>, Problem> {
        self.eat(b'[')?;
        self.space();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(items);
        }
        loop {
            items.push(self.value()?);
            if !self.more(b']')? {
                return Ok(items);
            }
        }
    }
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{absolute, TempDir};

    /// What Cursor writes: two-space JSON and a newline.
    const CURSOR: &str = r#"{
  "version": 1,
  "editor": {
    "vimMode": false
  },
  "permissions": {
    "allow": [
      "Shell(git status)",
      "Read(src/**)",
      "Shell(pnpm test)",
      "Shell(ls)"
    ],
    "deny": [
      "Shell(rm)"
    ]
  },
  "model": {
    "modelId": "example"
  }
}
"#;

    /// A home folder with Cursor's settings in it, as `text`.
    fn home_with(text: &str) -> (TempDir, PathBuf) {
        let home = TempDir::new();
        let dir = home.path().join(".cursor");
        fs::create_dir(&dir).unwrap();
        let path = dir.join(FILE);
        fs::write(&path, text).unwrap();
        (home, path)
    }

    /// `~` then `parts`, as [`shown`] writes a path in the home folder.
    fn tilde(parts: &[&str]) -> String {
        let path = parts
            .iter()
            .fold(PathBuf::from("~"), |path, part| path.join(part));
        path.display().to_string()
    }

    fn list_in(home: &Path) -> Result<CursorRules, String> {
        run(&CursorRuleCall::List, None, None, Some(home))
    }

    fn remove_in(home: &Path, rule: &str) -> Result<CursorRules, String> {
        let call = CursorRuleCall::Remove { rule: rule.into() };
        run(&call, None, None, Some(home))
    }

    /// The mode bits of `path`, which Windows gives as 0600 for every
    /// file.
    fn mode(path: &Path) -> u32 {
        maxhq_os::fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777
    }

    /// The mode a file set to 0640 reads back with.
    const SHARED: u32 = if cfg!(unix) { 0o640 } else { 0o600 };

    #[test]
    fn cursor_s_folder_is_the_one_its_cli_finds() {
        let home = PathBuf::from(absolute("/home/dev"));
        let home = home.as_path();
        let path = |dir: Option<&str>, xdg: Option<&str>, home: Option<&Path>| {
            config_path(dir.map(Into::into), xdg.map(Into::into), home)
        };
        let at = |path: &str| Ok(PathBuf::from(absolute(path)));
        assert_eq!(
            path(None, None, Some(home)),
            at("/home/dev/.cursor/cli-config.json")
        );
        let xdg = absolute("/home/dev/.config");
        assert_eq!(
            path(None, Some(&xdg), Some(home)),
            at("/home/dev/.config/cursor/cli-config.json")
        );
        assert_eq!(
            path(Some(&absolute("/srv/cursor")), Some(&xdg), Some(home)),
            at("/srv/cursor/cli-config.json")
        );
        // A blank one counts as not set, as it does for Cursor.
        assert_eq!(
            path(Some("  "), Some(""), Some(home)),
            at("/home/dev/.cursor/cli-config.json")
        );
        assert_eq!(path(None, None, None), Err(NO_HOME.to_owned()));
        assert_eq!(
            path(Some("cursor"), None, Some(home)),
            Err(RELATIVE.to_owned())
        );
        assert_eq!(
            shown(&home.join(".cursor").join(FILE), Some(home)),
            tilde(&[".cursor", FILE])
        );
        assert_eq!(
            shown(Path::new("/srv/cursor/cli-config.json"), Some(home)),
            "/srv/cursor/cli-config.json"
        );
    }

    #[test]
    fn only_the_shell_rules_of_the_allow_list_are_listed() {
        let (home, _) = home_with(CURSOR);
        assert_eq!(
            list_in(home.path()),
            Ok(CursorRules {
                path: tilde(&[".cursor", FILE]),
                rules: vec![
                    "Shell(git status)".into(),
                    "Shell(pnpm test)".into(),
                    "Shell(ls)".into(),
                ],
            })
        );
    }

    #[test]
    fn a_file_that_isn_t_there_has_no_rules_and_stays_away() {
        let home = TempDir::new();
        let none = CursorRules {
            path: tilde(&[".cursor", FILE]),
            rules: Vec::new(),
        };
        assert_eq!(list_in(home.path()), Ok(none.clone()));
        assert_eq!(remove_in(home.path(), "Shell(ls)"), Ok(none));
        assert!(!home.path().join(".cursor").exists());
    }

    #[test]
    fn a_removed_rule_leaves_everything_else_as_it_was() {
        let (home, path) = home_with(CURSOR);
        maxhq_os::fsmode::set_mode(&path, 0o640).unwrap();

        let left = remove_in(home.path(), "Shell(pnpm test)").unwrap();
        assert_eq!(left.rules, ["Shell(git status)", "Shell(ls)"]);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            CURSOR.replace("      \"Shell(pnpm test)\",\n", "")
        );
        assert_eq!(mode(&path), SHARED);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE], "no temp file is left behind");

        // The first and the last cut their own comma.
        remove_in(home.path(), "Shell(git status)").unwrap();
        let left = remove_in(home.path(), "Shell(ls)").unwrap();
        assert!(left.rules.is_empty());
        let expected = CURSOR
            .replace("      \"Shell(git status)\",\n", "")
            .replace("      \"Shell(pnpm test)\",\n", "")
            .replace("\"Read(src/**)\",\n      \"Shell(ls)\"", "\"Read(src/**)\"");
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(mode(&path), SHARED);
    }

    #[test]
    fn unknown_keys_and_other_entries_stay_byte_for_byte() {
        let text = concat!(
            "{\"later\":{\"a\":[1,2.50,{\"b\":null}],\"c\":\"\\\"Shell(ls)\\\"\"},",
            "\"permissions\":{\"ask\":[\"Shell(ls)\"],\"allow\":[\"Shell(ls)\"],",
            "\"deny\":[\"Shell(ls)\"]},\"version\":2}",
        );
        let (home, path) = home_with(text);
        assert_eq!(
            remove_in(home.path(), "Shell(ls)").unwrap().rules,
            Vec::<String>::new()
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            text.replace("\"allow\":[\"Shell(ls)\"]", "\"allow\":[]")
        );
    }

    #[test]
    fn each_copy_of_a_rule_goes() {
        let text = "{\"permissions\":{\"allow\":[\"Shell(ls)\",\"Read(a)\",\"Shell(ls)\"]}}\n";
        let (home, path) = home_with(text);
        remove_in(home.path(), "Shell(ls)").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"permissions\":{\"allow\":[\"Read(a)\"]}}\n"
        );
    }

    #[test]
    fn a_rule_that_isn_t_there_leaves_the_file_alone() {
        let (home, path) = home_with(CURSOR);
        maxhq_os::fsmode::set_mode(&path, 0o600).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let left = remove_in(home.path(), "Shell(make)").unwrap();
        assert_eq!(left.rules.len(), 3);
        assert_eq!(fs::read_to_string(&path).unwrap(), CURSOR);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn only_shell_rules_can_be_removed() {
        let (home, path) = home_with(CURSOR);
        let refused = remove_in(home.path(), "Read(src/**)").unwrap_err();
        assert_eq!(
            refused,
            format!(
                "maxd removes only Shell(...) rules from {}.",
                tilde(&[".cursor", FILE])
            )
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), CURSOR);
    }

    #[test]
    fn a_file_in_a_shape_maxd_doesn_t_know_stays_as_it_is() {
        let shapes = [
            ("{\"permissions\":{\"allow\":\"Shell(ls)\"}}", "isn't in a shape"),
            ("{\"permissions\":{\"allow\":[\"Shell(ls)\",7]}}", "isn't in a shape"),
            ("{\"permissions\":[\"Shell(ls)\"]}", "isn't in a shape"),
            ("[\"Shell(ls)\"]", "isn't in a shape"),
            (
                "{\"permissions\":{\"allow\":[\"Shell(ls)\"]},\"permissions\":{\"allow\":[\"Shell(ls)\"]}}",
                "isn't in a shape",
            ),
            (
                "{\"permissions\":{\"allow\":[\"Shell(ls)\"],\"allow\":[\"Shell(ls)\"]}}",
                "isn't in a shape",
            ),
            ("{\"permissions\":{\"allow\":[\"Shell(ls)\",]}}", "isn't JSON"),
            ("// Cursor's settings\n{\"permissions\":{}}", "isn't JSON"),
        ];
        for (text, reason) in shapes {
            let (home, path) = home_with(text);
            let refused = remove_in(home.path(), "Shell(ls)").unwrap_err();
            assert!(refused.contains(reason), "{text}: {refused}");
            assert!(
                refused.starts_with(&format!("{} ", tilde(&[".cursor", FILE]))),
                "{refused}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        // Listing a file it can't read says so too.
        let (home, _) = home_with("{\"permissions\":{\"allow\":{}}}");
        assert!(list_in(home.path())
            .unwrap_err()
            .contains("isn't in a shape"));
    }

    #[test]
    fn a_blank_file_or_one_without_permissions_has_no_rules() {
        for text in [
            "",
            "\n",
            "{}",
            "{\"permissions\":{}}",
            "{\"permissions\":{\"deny\":[]}}",
        ] {
            let (home, path) = home_with(text);
            assert_eq!(list_in(home.path()).unwrap().rules, Vec::<String>::new());
            assert_eq!(
                remove_in(home.path(), "Shell(ls)").unwrap().rules,
                Vec::<String>::new()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_linked_file_is_changed_and_the_link_kept() {
        let (home, _) = home_with("");
        let dotfiles = home.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join(FILE);
        fs::write(&real, CURSOR).unwrap();
        let link = home.path().join(".cursor").join(FILE);
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        remove_in(home.path(), "Shell(ls)").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(list_in(home.path()).unwrap().rules.len(), 2);
        assert!(!fs::read_to_string(&real).unwrap().contains("Shell(ls)"));
    }

    #[test]
    fn the_folder_cursor_config_dir_names_is_the_one_read() {
        let home = TempDir::new();
        let dir = home.path().join("elsewhere");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(FILE), CURSOR).unwrap();
        let listed = run(
            &CursorRuleCall::List,
            Some(dir.clone().into_os_string()),
            None,
            Some(home.path()),
        )
        .unwrap();
        assert_eq!(listed.path, tilde(&["elsewhere", FILE]));
        assert_eq!(listed.rules.len(), 3);
    }

    #[tokio::test]
    async fn another_device_s_rules_are_refused_here() {
        let me = DeviceId::derive("mac");
        let answer = answer(me, DeviceId::derive("devbox"), CursorRuleCall::List).await;
        assert_eq!(answer, refused(OTHER_DEVICE));
    }
}
