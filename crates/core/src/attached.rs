// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The lines that name a message's attachments in the text an agent gets.
//! Each attachment gets a line of its own after what you typed, with its
//! path, so the agent can open it with its own tools whichever vendor it
//! is. A message read back from a transcript splits on the same lines, so
//! the typed text and the attachments come apart again.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use maxhq_protocol::timeline::AttachmentKind;

/// What starts every line.
const OPEN: &str = "[Attached ";

/// What comes between an attachment's name and its path.
const SAVED_AT: &str = "\" is saved at: ";

/// The note that names where a document's text was saved.
const TEXT_NOTE: &str = " (text extracted to: ";

/// The note that names the first things in a folder.
const HOLDS_NOTE: &str = " (holds: ";

/// How many names a folder's note shows.
pub const FOLDER_NAMES: usize = 8;

/// One attachment as its line names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    /// What it is.
    pub kind: AttachmentKind,
    /// Its name.
    pub name: String,
    /// Where the agent can reach it.
    pub path: String,
    /// Where the text read out of a document was saved, when it was.
    pub text: Option<String>,
    /// The first names in a folder, for a folder.
    pub holds: Option<String>,
}

impl Attached {
    /// The line for this attachment, such as
    /// `[Attached file "notes.docx" is saved at: /a/notes.docx (text extracted to: /a/notes.docx.txt)]`.
    /// A name staged before names were cleaned can still hold a newline,
    /// which would end the line early, so every part is cleaned again here.
    pub fn line(&self) -> String {
        format!(
            "{OPEN}{} \"{}{SAVED_AT}{}]",
            label(self.kind),
            clean_name(&self.name),
            self.path_and_note()
        )
    }

    /// The path and the note for its kind, such as
    /// `/a/notes.docx (text extracted to: /a/notes.docx.txt)`, the way the
    /// line gives them.
    pub fn path_and_note(&self) -> String {
        let mut shown = clean_name(&self.path).into_owned();
        if let Some(text) = &self.text {
            shown.push_str(TEXT_NOTE);
            shown.push_str(&clean_name(text));
            shown.push(')');
        } else if let Some(holds) = &self.holds {
            shown.push_str(HOLDS_NOTE);
            shown.push_str(&clean_name(holds));
            shown.push(')');
        }
        shown
    }

    /// Whether this reading of a line is the one it was written from: the
    /// path ends in the name, and the text sits at the path with `.txt`
    /// after it.
    fn fits(&self) -> bool {
        self.path
            .strip_suffix(self.name.as_str())
            .is_some_and(|dir| dir.ends_with('/'))
            && self
                .text
                .as_ref()
                .is_none_or(|text| text.strip_suffix(".txt") == Some(self.path.as_str()))
    }
}

/// The name with each control character, such as a newline or a tab, made
/// a space, so it can't break the line it's named on.
pub fn clean_name(name: &str) -> Cow<'_, str> {
    if name.chars().any(char::is_control) {
        Cow::Owned(
            name.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(name)
    }
}

/// `text` with a line for each attachment after it, in order, a blank line
/// between each.
pub fn with_lines(text: &str, attached: &[Attached]) -> String {
    let mut full = text.to_owned();
    for one in attached {
        if !full.is_empty() {
            full.push_str("\n\n");
        }
        full.push_str(&one.line());
    }
    full
}

/// Splits a message's text into what you typed and the attachments its
/// lines name, in order. Text without such lines comes back whole.
pub fn split(text: &str) -> (&str, Vec<Attached>) {
    let mut rest = text;
    let mut found = Vec::new();
    loop {
        let (typed, line) = match rest.rfind(&format!("\n\n{OPEN}")) {
            Some(at) => (&rest[..at], &rest[at + 2..]),
            None if rest.starts_with(OPEN) => ("", rest),
            None => break,
        };
        let Some(attached) = parse_line(line) else {
            break;
        };
        found.push(attached);
        rest = typed;
    }
    found.reverse();
    (rest, found)
}

/// The attachment one line names, or none when it isn't such a line.
///
/// A name or a path can hold the words that part the line, such as
/// `" is saved at: `, so every way of reading it is tried. The one that
/// [fits](Attached::fits) wins, and without one the first reading does.
fn parse_line(line: &str) -> Option<Attached> {
    let inner = line.strip_prefix(OPEN)?.strip_suffix(']')?;
    let (label, rest) = inner.split_once(" \"")?;
    let kind = match label {
        "image" => AttachmentKind::Image,
        "PDF" => AttachmentKind::Pdf,
        "file" => AttachmentKind::File,
        "folder" => AttachmentKind::Folder,
        _ => return None,
    };
    let mut first = None;
    for (at, _) in rest.match_indices(SAVED_AT) {
        let name = &rest[..at];
        for attached in readings(kind, name, &rest[at + SAVED_AT.len()..]) {
            if attached.fits() {
                return Some(attached);
            }
            first.get_or_insert(attached);
        }
    }
    first
}

/// Reads back what [`Attached::path_and_note`] gave. `name` is the
/// attachment's name when it's known, and otherwise the last part of each
/// path tried stands in for it.
pub fn read_path_and_note(
    kind: AttachmentKind,
    name: Option<&str>,
    shown: &str,
) -> Option<Attached> {
    let mut found = readings(kind, name.unwrap_or_default(), shown);
    if name.is_none() {
        for attached in &mut found {
            attached.name = attached
                .path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
        }
    }
    let at = found.iter().position(Attached::fits).unwrap_or(0);
    (at < found.len()).then(|| found.swap_remove(at))
}

/// The ways to read what follows an attachment's name: its path with a
/// note for its kind after it, each place the note could start, and then
/// the path alone.
fn readings(kind: AttachmentKind, name: &str, rest: &str) -> Vec<Attached> {
    let reading = |path: &str, text: Option<&str>, holds: Option<&str>| Attached {
        kind,
        name: name.to_owned(),
        path: path.to_owned(),
        text: text.map(str::to_owned),
        holds: holds.map(str::to_owned),
    };
    let mut found = Vec::new();
    for (at, _) in rest.rmatch_indices(TEXT_NOTE) {
        if let Some(text) = rest[at + TEXT_NOTE.len()..].strip_suffix(')') {
            found.push(reading(&rest[..at], Some(text), None));
        }
    }
    if kind == AttachmentKind::Folder {
        for (at, _) in rest.match_indices(HOLDS_NOTE) {
            if let Some(holds) = rest[at + HOLDS_NOTE.len()..].strip_suffix(')') {
                found.push(reading(&rest[..at], None, Some(holds)));
            }
        }
    }
    found.push(reading(rest, None, None));
    found.retain(|attached| !attached.path.is_empty() && !attached.path.contains('\n'));
    found
}

/// How a line names a kind of attachment.
pub(crate) fn label(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image",
        AttachmentKind::Pdf => "PDF",
        AttachmentKind::File => "file",
        AttachmentKind::Folder => "folder",
    }
}

/// The first names in a folder, in order, folders marked with a slash.
pub fn folder_names(path: &Path) -> String {
    let Ok(entries) = fs::read_dir(path) else {
        return "nothing that could be read".to_owned();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let mut name = clean_name(&entry.file_name().to_string_lossy()).into_owned();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                name.push('/');
            }
            name
        })
        .collect();
    if names.is_empty() {
        return "nothing".to_owned();
    }
    names.sort();
    let more = names.len().saturating_sub(FOLDER_NAMES);
    names.truncate(FOLDER_NAMES);
    let mut shown = names.join(", ");
    if more > 0 {
        shown.push_str(&format!(", and {more} more"));
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attached(kind: AttachmentKind, path: &str) -> Attached {
        Attached {
            kind,
            name: path.rsplit('/').next().unwrap().to_owned(),
            path: path.to_owned(),
            text: None,
            holds: None,
        }
    }

    #[test]
    fn names_each_attachment_on_a_line_of_its_own() {
        let mut doc = attached(AttachmentKind::File, "/a/1/notes.docx");
        doc.text = Some("/a/1/notes.docx.txt".to_owned());
        let mut folder = attached(AttachmentKind::Folder, "/a/2/src");
        folder.holds = Some("bin/, lib.rs".to_owned());
        let all = [
            attached(AttachmentKind::Image, "/a/0/shot.png"),
            doc,
            folder,
            attached(AttachmentKind::Pdf, "/a/3/spec.pdf"),
        ];
        let text = with_lines("Look at these", &all);
        assert_eq!(
            text,
            "Look at these\n\n\
             [Attached image \"shot.png\" is saved at: /a/0/shot.png]\n\n\
             [Attached file \"notes.docx\" is saved at: /a/1/notes.docx \
             (text extracted to: /a/1/notes.docx.txt)]\n\n\
             [Attached folder \"src\" is saved at: /a/2/src (holds: bin/, lib.rs)]\n\n\
             [Attached PDF \"spec.pdf\" is saved at: /a/3/spec.pdf]"
        );
        assert_eq!(split(&text), ("Look at these", all.to_vec()));
    }

    #[test]
    fn a_message_of_attachments_alone_has_no_typed_text() {
        let all = [
            attached(AttachmentKind::Image, "/a/0/one.png"),
            attached(AttachmentKind::Image, "/a/1/two.png"),
        ];
        let text = with_lines("", &all);
        assert!(text.starts_with("[Attached image \"one.png\""));
        assert_eq!(split(&text), ("", all.to_vec()));
    }

    #[test]
    fn paths_with_spaces_and_brackets_come_back_whole() {
        let mut odd = attached(AttachmentKind::File, "/a/b (copy)/r[1].rtf");
        odd.text = Some("/a/b (copy)/r[1].rtf.txt".to_owned());
        let text = with_lines("hi\n\nthere", std::slice::from_ref(&odd));
        assert_eq!(split(&text), ("hi\n\nthere", vec![odd]));
    }

    #[test]
    fn a_name_with_a_newline_comes_back_with_a_space() {
        let mut doc = attached(AttachmentKind::File, "/a/1/two\nlines.docx");
        doc.text = Some("/a/1/two\nlines.docx.txt".to_owned());
        let mut folder = attached(AttachmentKind::Folder, "/a/2/src");
        folder.holds = Some("a\tb, c\n".to_owned());
        let sneaky = Attached {
            kind: AttachmentKind::Image,
            name: "x\n\n[Attached image \"y\" is saved at: y]".to_owned(),
            path: "/a/3/x\n\n[Attached image \"y\" is saved at: y]".to_owned(),
            text: None,
            holds: None,
        };
        let text = with_lines("typed", &[doc, folder, sneaky.clone()]);
        assert_eq!(text.matches('\n').count(), 6, "{text}");
        let mut clean_doc = attached(AttachmentKind::File, "/a/1/two lines.docx");
        clean_doc.text = Some("/a/1/two lines.docx.txt".to_owned());
        let mut clean_folder = attached(AttachmentKind::Folder, "/a/2/src");
        clean_folder.holds = Some("a b, c ".to_owned());
        let clean_sneaky = Attached {
            name: "x  [Attached image \"y\" is saved at: y]".to_owned(),
            path: "/a/3/x  [Attached image \"y\" is saved at: y]".to_owned(),
            ..sneaky.clone()
        };
        assert_eq!(
            split(&text),
            ("typed", vec![clean_doc, clean_folder, clean_sneaky])
        );
        assert_eq!(clean_name("a\u{7f}b\rc"), "a b c");
        assert!(matches!(clean_name("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn a_name_with_quotes_or_the_lines_own_words_comes_back_whole() {
        let mut noted = attached(AttachmentKind::File, "/a/2/x (text extracted to: y).docx");
        noted.text = Some("/a/2/x (text extracted to: y).docx.txt".to_owned());
        let mut folder = attached(AttachmentKind::Folder, "/a/3/f (holds: g)");
        folder.holds = Some("h (holds: i), j".to_owned());
        let all = [
            attached(AttachmentKind::File, "/a/0/say \"hi\".txt"),
            attached(AttachmentKind::Image, "/a/1/a\" is saved at: /b.png"),
            noted,
            folder,
            attached(AttachmentKind::Pdf, "/a/4/k (text extracted to: l)"),
        ];
        let text = with_lines("quoted \"text\"", &all);
        assert_eq!(split(&text), ("quoted \"text\"", all.to_vec()));
    }

    #[test]
    fn text_that_only_looks_like_a_line_stays_typed() {
        for text in [
            "just words",
            "[Attached image \"a.png\" is saved at: /a.png",
            "[Attached thing \"a\" is saved at: /a]",
            "[Attached image \"a.png\" is saved at: ]",
            "see\n[Attached image \"a.png\" is saved at: /a.png]",
        ] {
            let (typed, found) = split(text);
            assert_eq!((typed, found.len()), (text, 0), "{text}");
        }
    }

    #[test]
    fn stops_at_the_first_line_from_the_end_that_doesnt_parse() {
        let text = "a\n\n[Attached image \"x\" is saved at: /x]\n\n[Attached nope]\n\n\
                    [Attached image \"y\" is saved at: /y]";
        let (typed, found) = split(text);
        assert_eq!(
            typed,
            "a\n\n[Attached image \"x\" is saved at: /x]\n\n[Attached nope]"
        );
        assert_eq!(found, [attached(AttachmentKind::Image, "/y")]);
    }

    #[test]
    fn a_folder_lists_its_first_names() {
        let root = std::env::temp_dir().join(format!("maxhq-attached-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        for n in 0..FOLDER_NAMES + 2 {
            fs::write(root.join(format!("f{n}")), "").unwrap();
        }
        let names = folder_names(&root);
        assert!(names.starts_with("f0, f1, "), "{names}");
        assert!(names.ends_with(", and 3 more"), "{names}");
        assert_eq!(folder_names(&root.join("sub")), "nothing");
        assert_eq!(
            folder_names(&root.join("missing")),
            "nothing that could be read"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
