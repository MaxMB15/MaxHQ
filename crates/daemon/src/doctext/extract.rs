// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

//! Reads text out of documents, ported from MIT-licensed code, see
//! THIRD_PARTY_NOTICES. It covers Word, Excel and PowerPoint files, their
//! OpenDocument counterparts, EPUB, RTF and Jupyter notebooks, and keeps
//! headings, tables, slide notes and reading order within fixed limits. As
//! in that code, it runs in a child process: the hidden `maxd` subcommand
//! in the module above calls it, and the daemon gives that child its time
//! limit.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde_json::Value;

use super::xml::{self, Child, Node};
use super::{rtf, zip};

/// The extensions with a text extractor, lowercase and without the dot.
pub const EXTENSIONS: &[&str] = &[
    "docx", "xlsx", "pptx", "odt", "ods", "odp", "rtf", "epub", "ipynb",
];

/// The largest document read, in bytes. A larger one gives only the
/// truncation notice.
const MAX_INPUT: u64 = 16 * 1024 * 1024;

/// How many bytes an archive's text parts can expand to in all.
pub(super) const MAX_EXPANDED: u64 = 32 * 1024 * 1024;

/// The most characters of text kept.
const MAX_OUTPUT: usize = 1_000_000;

/// The most rows read from a table or sheet.
const MAX_ROWS: usize = 10_000;

/// The most columns read from a row.
const MAX_COLUMNS: usize = 512;

/// The most notebook cells read.
const MAX_CELLS: usize = 10_000;

/// What ends text that was cut short.
const TRUNCATED: &str = "\n\n[Extracted text truncated at document extraction limits.]";

/// Elements whose content is never read.
const HIDDEN: &[&str] = &["script", "style", "binary-data"];

/// Elements that hold text but not a cell's own text, such as Excel's
/// phonetic guides.
const TEXTLESS: &[&str] = &["script", "style", "binary-data", "rPh"];

/// Elements that end a line.
const BREAKS: &[&str] = &["br", "line-break", "cr"];

/// Elements that hold one table cell.
const CELLS: &[&str] = &["tc", "table-cell", "covered-table-cell", "td", "th"];

/// Why a document gave no text.
#[derive(Debug)]
pub enum Failure {
    /// The document can't be read this way, so trying again won't help.
    Rejected(String),
    /// Reading the file failed, which might pass.
    Io(std::io::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => f.write_str(reason),
            Self::Io(error) => write!(f, "couldn't read the document: {error}"),
        }
    }
}

impl std::error::Error for Failure {}

/// Why reading stopped before the end of a document.
#[derive(Debug)]
pub(super) enum Stop {
    /// A limit was reached, so the text so far is kept with a notice.
    Truncated,
    /// The document can't be read this way.
    Rejected(String),
    /// Reading the file failed.
    Io(io::Error),
}

/// Reads a document's text, picking the format by the path's extension.
pub fn extract(path: &Path) -> Result<String, Failure> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut out = Out::new();
    let truncated = match document(path, &extension, &mut out) {
        Ok(()) => false,
        Err(Stop::Truncated) => true,
        Err(Stop::Rejected(reason)) => return Err(Failure::Rejected(reason)),
        Err(Stop::Io(error)) => return Err(Failure::Io(error)),
    };
    out.finish(truncated)
}

/// Reads the document at `path` into `out`.
fn document(path: &Path, extension: &str, out: &mut Out) -> Result<(), Stop> {
    if !EXTENSIONS.contains(&extension) {
        return Err(rejected("Unsupported document"));
    }
    let file = File::open(path).map_err(Stop::Io)?;
    if file.metadata().map_err(Stop::Io)?.len() > MAX_INPUT {
        return Err(Stop::Truncated);
    }
    let mut data = Vec::new();
    file.take(MAX_INPUT + 1)
        .read_to_end(&mut data)
        .map_err(Stop::Io)?;
    if data.len() as u64 > MAX_INPUT {
        return Err(Stop::Truncated);
    }
    match extension {
        "ipynb" => notebook(&String::from_utf8_lossy(&data), out),
        "rtf" => rtf::read(&data, out),
        _ => archive(&data, extension, out),
    }
}

/// Collects text within the output budget. Text inside table cells has
/// its whitespace collapsed, so a cell stays on one line.
pub(super) struct Out {
    /// The text so far.
    result: String,
    /// How many more characters fit.
    remaining: usize,
    /// How many cells deep the text is.
    depth: usize,
    /// Whether the outermost cell has shown text yet.
    started: bool,
    /// Whether a space is owed before the cell's next text.
    space: bool,
}

impl Out {
    /// An empty collector with the full budget.
    pub(super) fn new() -> Self {
        Self {
            result: String::new(),
            remaining: MAX_OUTPUT,
            depth: 0,
            started: false,
            space: false,
        }
    }

    /// Adds text, stopping once the budget runs out. Text is charged
    /// before it's tidied, so runs of whitespace use up the budget too.
    pub(super) fn text(&mut self, raw: &str) -> Result<(), Stop> {
        let end = raw
            .char_indices()
            .nth(self.remaining)
            .map_or(raw.len(), |(at, _)| at);
        let kept = &raw[..end];
        self.remaining -= kept.chars().count();
        if self.depth > 0 {
            let value = collapse_spaces(kept);
            let trimmed = value.trim_matches(' ');
            if !trimmed.is_empty() {
                if self.started && (self.space || value.starts_with(' ')) {
                    self.result.push(' ');
                }
                self.result.push_str(trimmed);
                self.started = true;
            }
            self.space = value.ends_with(' ') || (value.is_empty() && self.space);
        } else {
            self.result.extend(kept.chars().filter(|c| *c != '\0'));
        }
        if self.remaining == 0 {
            return Err(Stop::Truncated);
        }
        Ok(())
    }

    /// Adds a cell's stored value, unless the cell already showed text.
    fn fallback(&mut self, value: &str) -> Result<(), Stop> {
        if self.started {
            return Ok(());
        }
        self.text(value)
    }

    /// Marks the start of a cell. Text in a cell nested inside another is
    /// set apart by a space.
    fn cell_start(&mut self) {
        if self.depth == 0 {
            self.started = false;
            self.space = false;
        } else if self.started {
            self.space = true;
        }
        self.depth += 1;
    }

    /// Marks the end of a cell.
    fn cell_end(&mut self) {
        self.depth -= 1;
        if self.depth > 0 && self.started {
            self.space = true;
        }
    }

    /// The tidied text, with the notice when it was cut short.
    fn finish(self, truncated: bool) -> Result<String, Failure> {
        let collapsed = collapse_blank_lines(&self.result);
        let text = js_trim(&collapsed);
        if text.is_empty() && !truncated {
            return Err(Failure::Rejected("No text extracted".to_owned()));
        }
        let mut text = text.to_owned();
        if truncated {
            text.push_str(TRUNCATED);
        }
        Ok(text)
    }

    /// The text collected so far, before it's tidied.
    #[cfg(test)]
    pub(super) fn into_raw(self) -> String {
        self.result
    }
}

/// Runs `write` as one table cell.
fn in_cell(out: &mut Out, write: impl FnOnce(&mut Out) -> Result<(), Stop>) -> Result<(), Stop> {
    out.cell_start();
    write(out)?;
    out.cell_end();
    Ok(())
}

/// The parts of an unpacked archive, and notes on the ones that couldn't
/// be read.
struct Parts {
    /// The text of each part, by name.
    files: HashMap<String, String>,
    /// A note for each part that was skipped.
    notes: Vec<String>,
    /// Whether parts count OpenDocument spaces.
    odf: bool,
    /// Whether parts know HTML entities.
    html: bool,
}

impl Parts {
    /// Whether the archive has a part called `name`.
    fn has(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    /// Parses a part. One that's missing or isn't XML gets a note and
    /// reads as empty.
    fn read(&mut self, name: &str) -> Result<Node, Stop> {
        let parsed = match self.files.get(name) {
            Some(data) if !data.is_empty() => xml::parse(data, self.odf, self.html),
            _ => Err(rejected("Missing document part")),
        };
        match parsed {
            Ok(node) => Ok(node),
            Err(Stop::Truncated) => Err(Stop::Truncated),
            Err(_) => {
                self.skip(name);
                Ok(Node::empty())
            }
        }
    }

    /// Notes a part that couldn't be read.
    fn skip(&mut self, name: &str) {
        self.notes.push(format!(
            "\n[Skipped document part: {name} (missing or invalid XML).]\n"
        ));
    }

    /// The targets of a relationships part, by id, leaving out external
    /// ones.
    fn relations(&mut self, name: &str) -> Result<HashMap<String, String>, Stop> {
        let root = self.read(name)?;
        let relations = root
            .all(&["Relationship"])
            .into_iter()
            .filter(|rel| rel.attr("TargetMode") != Some("External"))
            .filter_map(|rel| Some((rel.attr("Id")?.to_owned(), rel.attr("Target")?.to_owned())))
            .collect();
        Ok(relations)
    }
}

/// Reads an Office or OpenDocument file, or an EPUB, from its archive.
fn archive(data: &[u8], extension: &str, out: &mut Out) -> Result<(), Stop> {
    let mut parts = Parts {
        files: zip::parts(data)?,
        notes: Vec::new(),
        odf: matches!(extension, "odt" | "ods" | "odp"),
        html: extension == "epub",
    };
    match extension {
        "docx" => paragraphs(&parts.read("word/document.xml")?, out)?,
        "odt" => paragraphs(&parts.read("content.xml")?, out)?,
        "ods" => {
            let content = parts.read("content.xml")?;
            for sheet in content.all(&["table"]) {
                let name = sheet.attr("name").unwrap_or("Untitled");
                out.text(&format!("\n\n## Sheet: {name}\n"))?;
                table(sheet, out)?;
            }
        }
        "odp" => {
            let content = parts.read("content.xml")?;
            for (index, page) in content.all(&["page"]).into_iter().enumerate() {
                out.text(&format!("\n\n## Slide {}\n", index + 1))?;
                paragraphs(page, out)?;
            }
        }
        "xlsx" => workbook(&mut parts, out)?,
        "pptx" => presentation(&mut parts, out)?,
        "epub" => book(&mut parts, out)?,
        _ => return Err(rejected("Unsupported document")),
    }
    for note in &parts.notes {
        out.text(note)?;
    }
    Ok(())
}

/// What a workbook's sheets share: strings, cell formats and its date
/// system.
struct Book<'a> {
    /// The shared strings, by index.
    strings: Vec<&'a Node>,
    /// Whether each shared string has text.
    populated: Vec<bool>,
    /// The cell formats, by style index.
    formats: Vec<&'a Node>,
    /// The format code of each custom number format, by id.
    custom: HashMap<&'a str, &'a str>,
    /// The year serial dates count from.
    date_system: &'static str,
}

/// Reads an Excel workbook, sheet by sheet in the workbook's order.
fn workbook(parts: &mut Parts, out: &mut Out) -> Result<(), Stop> {
    let workbook = parts.read("xl/workbook.xml")?;
    let date1904 = workbook
        .first("workbookPr")
        .and_then(|pr| pr.attr("date1904"));
    let styles = if parts.has("xl/styles.xml") {
        Some(parts.read("xl/styles.xml")?)
    } else {
        None
    };
    let shared = if parts.has("xl/sharedStrings.xml") {
        Some(parts.read("xl/sharedStrings.xml")?)
    } else {
        None
    };
    let strings = shared
        .as_ref()
        .map(|root| root.all(&["si"]))
        .unwrap_or_default();
    let book = Book {
        populated: strings.iter().map(|string| has_cell_text(string)).collect(),
        strings,
        formats: styles
            .as_ref()
            .and_then(|root| root.first("cellXfs"))
            .map(|formats| formats.elements().collect())
            .unwrap_or_default(),
        custom: styles
            .iter()
            .flat_map(|root| root.all(&["numFmt"]))
            .filter_map(|format| Some((format.attr("numFmtId")?, format.attr("formatCode")?)))
            .collect(),
        date_system: if matches!(date1904, Some("1" | "true")) {
            "1904"
        } else {
            "1900"
        },
    };
    let rels = parts.relations("xl/_rels/workbook.xml.rels")?;
    for sheet in workbook.all(&["sheet"]) {
        let name = sheet.attr("name").unwrap_or("Untitled");
        out.text(&format!("\n## Sheet: {name}\n"))?;
        let Some(target) = target(sheet, &rels) else {
            parts.skip(&format!("sheet relationship {name}"));
            continue;
        };
        let path = match target.strip_prefix('/') {
            Some(path) => path.to_owned(),
            None => normalize(&format!("xl/{target}")),
        };
        rows(&parts.read(&path)?, &book, out)?;
    }
    Ok(())
}

/// The part a sheet or slide points to through its relationship id.
fn target<'a>(node: &Node, rels: &'a HashMap<String, String>) -> Option<&'a str> {
    let id = node.attr("r:id").or_else(|| node.attr("id"))?;
    rels.get(id)
        .map(String::as_str)
        .filter(|target| !target.is_empty())
}

/// Reads a sheet's rows, placing cells by their column letters and
/// leaving out empty ones.
fn rows(sheet: &Node, book: &Book, out: &mut Out) -> Result<(), Stop> {
    let mut count = 0;
    for row in sheet.all(&["row"]) {
        let mut cells: Vec<Option<&Node>> = Vec::new();
        let mut next_column = 0usize;
        let mut columns_truncated = false;
        for cell in row.elements().filter(|node| node.name == "c") {
            let letters = cell
                .attr("r")
                .map(|r| {
                    r.bytes()
                        .take_while(u8::is_ascii_uppercase)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let column = if letters.is_empty() {
                next_column
            } else {
                letters.iter().fold(0usize, |n, letter| {
                    n.saturating_mul(26)
                        .saturating_add(usize::from(letter - b'A' + 1))
                }) - 1
            };
            next_column = column.saturating_add(1);
            if !populated(cell, book) {
                continue;
            }
            if column >= MAX_COLUMNS {
                columns_truncated = true;
                continue;
            }
            if cells.len() <= column {
                cells.resize(column + 1, None);
            }
            cells[column] = Some(cell);
        }
        if cells.is_empty() && !columns_truncated {
            continue;
        }
        count += 1;
        if count > MAX_ROWS {
            return Err(Stop::Truncated);
        }
        for (column, cell) in cells.into_iter().enumerate() {
            if column > 0 {
                out.text("\t")?;
            }
            if let Some(cell) = cell {
                sheet_cell(cell, book, out)?;
            }
        }
        out.text("\n")?;
        if columns_truncated {
            return Err(Stop::Truncated);
        }
    }
    Ok(())
}

/// Whether a sheet cell has anything to show.
fn populated(cell: &Node, book: &Book) -> bool {
    let value = cell.first("v");
    match cell.attr("t") {
        Some("s") => {
            let raw = value.and_then(Node::first_text).map(js_trim).unwrap_or("");
            !raw.is_empty()
                && js_index(raw)
                    .and_then(|index| book.populated.get(index))
                    .is_some_and(|populated| *populated)
        }
        Some("inlineStr") => has_cell_text(cell),
        _ => value.is_some_and(has_cell_text) || cell.all(&["f"]).into_iter().any(has_cell_text),
    }
}

/// Reads one sheet cell: a shared or inline string, or a value or its
/// formulas, with a note of the number format a number uses.
fn sheet_cell(cell: &Node, book: &Book, out: &mut Out) -> Result<(), Stop> {
    let value = cell.first("v");
    let raw = value.and_then(Node::first_text).unwrap_or("");
    let kind = cell.attr("t");
    match kind {
        Some("s") => {
            if let Some(shared) = js_index(raw).and_then(|index| book.strings.get(index)) {
                in_cell(out, |out| text(shared, out))?;
            }
        }
        Some("inlineStr") => in_cell(out, |out| text(cell, out))?,
        _ => {
            match value.filter(|value| has_cell_text(value)) {
                Some(value) => in_cell(out, |out| text(value, out))?,
                None => {
                    for formula in cell.all(&["f"]) {
                        in_cell(out, |out| text(formula, out))?;
                    }
                }
            }
            let format_id = js_index(cell.attr("s").unwrap_or("0"))
                .and_then(|index| book.formats.get(index))
                .and_then(|format| format.attr("numFmtId"))
                .filter(|id| !id.is_empty() && *id != "0");
            if let Some(id) = format_id
                && matches!(kind, None | Some("n"))
                && !js_trim(raw).is_empty()
            {
                let format = match book.custom.get(id) {
                    Some(code) if !code.is_empty() => format!("; format={code}"),
                    _ => String::new(),
                };
                let system = book.date_system;
                out.text(&format!(
                    " [Excel numFmtId={id}{format}; date system {system}]"
                ))?;
            }
        }
    }
    Ok(())
}

/// Reads a PowerPoint deck, slide by slide in the deck's order, with each
/// slide's notes.
fn presentation(parts: &mut Parts, out: &mut Out) -> Result<(), Stop> {
    let rels = parts.relations("ppt/_rels/presentation.xml.rels")?;
    let presentation = parts.read("ppt/presentation.xml")?;
    for (index, slide) in presentation.all(&["sldId"]).into_iter().enumerate() {
        let number = index + 1;
        out.text(&format!("\n\n## Slide {number}\n"))?;
        let Some(target) = target(slide, &rels) else {
            parts.skip(&format!("slide relationship {number}"));
            continue;
        };
        let name = match target.strip_prefix('/') {
            Some(name) => name.to_owned(),
            None => normalize(&format!("ppt/{target}")),
        };
        paragraphs(&parts.read(&name)?, out)?;
        let slide_rels = join(&[
            dirname(&name),
            "_rels",
            &format!("{}.rels", basename(&name)),
        ]);
        if !parts.has(&slide_rels) {
            continue;
        }
        for rel in parts.read(&slide_rels)?.all(&["Relationship"]) {
            let target = rel.attr("Target").unwrap_or("");
            let notes = rel
                .attr("Type")
                .is_some_and(|kind| kind.ends_with("/notesSlide"));
            if !notes || rel.attr("TargetMode") == Some("External") || target.is_empty() {
                continue;
            }
            out.text("Notes:\n")?;
            let path = match target.strip_prefix('/') {
                Some(path) => path.to_owned(),
                None => join(&[dirname(&name), target]),
            };
            paragraphs(&parts.read(&path)?, out)?;
        }
    }
    Ok(())
}

/// Reads an EPUB's chapters in spine order.
fn book(parts: &mut Parts, out: &mut Out) -> Result<(), Stop> {
    let container = parts.read("META-INF/container.xml")?;
    let rootfile = container
        .first("rootfile")
        .and_then(|rootfile| rootfile.attr("full-path"))
        .filter(|path| !path.is_empty());
    let Some(rootfile) = rootfile else {
        if parts.notes.is_empty() {
            parts.skip("EPUB package");
        }
        return Ok(());
    };
    let package = parts.read(rootfile)?;
    let items: HashMap<&str, &str> = package
        .all(&["item"])
        .into_iter()
        .filter_map(|item| Some((item.attr("id")?, item.attr("href")?)))
        .collect();
    for (index, itemref) in package.all(&["itemref"]).into_iter().enumerate() {
        let number = index + 1;
        out.text(&format!("\n\n## Chapter {number}\n"))?;
        let decoded = itemref
            .attr("idref")
            .and_then(|id| items.get(id))
            .filter(|href| !href.is_empty())
            .and_then(|href| decode_uri_component(href.split('#').next().unwrap_or_default()));
        let Some(decoded) = decoded else {
            parts.skip(&format!("chapter {number}"));
            continue;
        };
        let name = join(&[dirname(rootfile), &decoded]);
        paragraphs(&parts.read(&name)?, out)?;
    }
    Ok(())
}

/// Reads an element's text, turning tabs and breaks into characters.
fn text(node: &Node, out: &mut Out) -> Result<(), Stop> {
    let name = node.name.as_str();
    if TEXTLESS.contains(&name) {
        return Ok(());
    }
    if name == "tab" || name == "cell" {
        return out.text("\t");
    }
    if BREAKS.contains(&name) {
        return out.text("\n");
    }
    if let Some(count) = node.spaces {
        let shown = usize::try_from(count).unwrap_or(usize::MAX).min(MAX_OUTPUT);
        out.text(&" ".repeat(shown))?;
        if count > MAX_OUTPUT as u64 {
            return Err(Stop::Truncated);
        }
        return Ok(());
    }
    for child in &node.children {
        match child {
            Child::Text(value) => out.text(value)?,
            Child::Node(node) => text(node, out)?,
        }
    }
    Ok(())
}

/// Reads a block of paragraphs, headings and tables, a line each.
fn paragraphs(node: &Node, out: &mut Out) -> Result<(), Stop> {
    if HIDDEN.contains(&node.name.as_str()) {
        return Ok(());
    }
    for child in &node.children {
        let child = match child {
            Child::Text(value) => {
                out.text(value)?;
                continue;
            }
            Child::Node(child) => child,
        };
        let name = child.name.as_str();
        if BREAKS.contains(&name) {
            out.text("\n")?;
        } else if name == "tbl" || name == "table" {
            table(child, out)?;
        } else if name == "p" || name == "h" || html_heading(name).is_some() {
            if let Some(marker) = heading(child)? {
                out.text(&marker)?;
            }
            text(child, out)?;
            out.text("\n")?;
        } else {
            paragraphs(child, out)?;
            if matches!(name, "li" | "div" | "section") {
                out.text("\n")?;
            }
        }
    }
    Ok(())
}

/// The `#` marker for a heading paragraph, from an ODF outline level, an
/// HTML heading or a Word heading style.
fn heading(node: &Node) -> Result<Option<String>, Stop> {
    let level = if node.name == "h" {
        let level = node.attr("outline-level").map_or(f64::NAN, js_number);
        if level == 0.0 || level.is_nan() {
            1.0
        } else {
            level
        }
    } else if let Some(level) = html_heading(&node.name) {
        level
    } else {
        node.first("pStyle")
            .and_then(|style| style.attr("val"))
            .and_then(style_heading)
            .unwrap_or(0.0)
    };
    if level == 0.0 {
        return Ok(None);
    }
    let count = level.min(6.0).trunc();
    if count < 0.0 {
        return Err(rejected("Invalid heading level"));
    }
    Ok(Some(format!("{} ", "#".repeat(count as usize))))
}

/// The level of an HTML heading element, `h1` to `h6`.
fn html_heading(name: &str) -> Option<f64> {
    match name.as_bytes() {
        [b'h', level @ b'1'..=b'6'] => Some(f64::from(level - b'0')),
        _ => None,
    }
}

/// The level of a Word heading style, `Heading1` to `Heading6` in any case.
fn style_heading(style: &str) -> Option<f64> {
    match style.as_bytes() {
        [name @ .., level @ b'1'..=b'6'] if name.eq_ignore_ascii_case(b"heading") => {
            Some(f64::from(level - b'0'))
        }
        _ => None,
    }
}

/// Reads a table's rows, a line each with tabs between cells. Repeated
/// rows and columns repeat, and empty trailing cells and rows drop out.
fn table(node: &Node, out: &mut Out) -> Result<(), Stop> {
    let mut count = 0;
    for row in node.all(&["tr", "table-row"]) {
        let mut cells: Vec<&Node> = row
            .elements()
            .filter(|cell| CELLS.contains(&cell.name.as_str()))
            .collect();
        // ODS writers pad sheets with repeated empty rows and trailing cells.
        while cells.last().is_some_and(|cell| !has_cell_text(cell)) {
            cells.pop();
        }
        if cells.is_empty() {
            continue;
        }
        let repeat = xml::repeat_count(row.attr("number-rows-repeated"))?;
        for _ in 0..repeat {
            count += 1;
            if count > MAX_ROWS {
                return Err(Stop::Truncated);
            }
            let mut column = 0;
            for cell in &cells {
                let repeat = xml::repeat_count(cell.attr("number-columns-repeated"))?;
                for _ in 0..repeat {
                    if column >= MAX_COLUMNS {
                        return Err(Stop::Truncated);
                    }
                    if column > 0 {
                        out.text("\t")?;
                    }
                    column += 1;
                    out.cell_start();
                    paragraphs(cell, out)?;
                    out.fallback(&odf_value(cell))?;
                    out.cell_end();
                }
            }
            out.text("\n")?;
        }
    }
    Ok(())
}

/// The value an ODF cell stores, which shows when the cell has no text.
fn odf_value(cell: &Node) -> Cow<'_, str> {
    let attr = |name: &str| {
        cell.attr(&format!("office:{name}"))
            .or_else(|| cell.attr(name))
            .unwrap_or_default()
    };
    let kind = cell
        .attr("office:value-type")
        .or_else(|| cell.attr("value-type"));
    let value = match kind {
        Some("void") => "",
        Some("string") => attr("string-value"),
        Some("boolean") => attr("boolean-value"),
        Some("date") => attr("date-value"),
        Some("time") => attr("time-value"),
        Some("currency") if js_trim(attr("value")).is_empty() => "",
        Some("currency") => {
            let value = format!("{} {}", attr("value"), attr("currency"));
            return Cow::Owned(js_trim(&value).to_owned());
        }
        Some("percentage") if js_trim(attr("value")).is_empty() => "",
        Some("percentage") => {
            return Cow::Owned(format!("{} [ODF value-type=percentage]", attr("value")));
        }
        _ => attr("value"),
    };
    Cow::Borrowed(value)
}

/// Whether a cell, or anything in it, has text to show.
fn has_cell_text(node: &Node) -> bool {
    if !js_trim(&odf_value(node)).is_empty() {
        return true;
    }
    // Space markers can have enormous repeat counts; emptiness never requires expanding them.
    if TEXTLESS.contains(&node.name.as_str()) || node.spaces.is_some() {
        return false;
    }
    node.children.iter().any(|child| match child {
        Child::Text(value) => !js_trim(value).is_empty(),
        Child::Node(node) => has_cell_text(node),
    })
}

/// Reads a Jupyter notebook's cells, with their outputs and tracebacks.
fn notebook(source: &str, out: &mut Out) -> Result<(), Stop> {
    let book: Value = serde_json::from_str(source).map_err(|_| rejected("Invalid notebook"))?;
    let cells = book
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| rejected("Invalid notebook"))?;
    for (index, cell) in cells.iter().enumerate() {
        if index >= MAX_CELLS {
            return Err(Stop::Truncated);
        }
        if !cell.is_object() && !cell.is_array() {
            continue;
        }
        let code = cell.get("cell_type").and_then(Value::as_str) == Some("code");
        let kind = if code { "code" } else { "markdown" };
        out.text(&format!("## Cell {} ({kind})\n", index + 1))?;
        if let Some(source) = cell.get("source") {
            for line in lines(source) {
                out.text(line)?;
            }
        }
        if let Some(outputs) = cell.get("outputs").and_then(Value::as_array) {
            out.text("\nOutput:\n")?;
            for output in outputs {
                if !output.is_object() && !output.is_array() {
                    continue;
                }
                cell_output(output, out)?;
                out.text("\n")?;
            }
        }
        out.text("\n\n")?;
    }
    Ok(())
}

/// Reads one cell output: its text, its plain text form or its error.
fn cell_output(output: &Value, out: &mut Out) -> Result<(), Stop> {
    let plain = output
        .get("data")
        .filter(|data| data.is_object())
        .and_then(|data| data.get("text/plain"));
    if let Some(text) = output.get("text").or(plain) {
        for line in lines(text) {
            out.text(line)?;
        }
    } else if output.get("output_type").and_then(Value::as_str) == Some("error") {
        if let Some(name) = output.get("ename") {
            for line in lines(name) {
                out.text(line)?;
            }
        }
        if let Some(value) = output.get("evalue") {
            out.text(": ")?;
            for line in lines(value) {
                out.text(line)?;
            }
        }
        out.text("\n")?;
        if let Some(traceback) = output.get("traceback") {
            for line in lines(traceback) {
                out.text(&strip_vt(line))?;
                out.text("\n")?;
            }
        }
    }
    Ok(())
}

/// The strings in a notebook field that holds a string or a list of them.
fn lines(value: &Value) -> Vec<&str> {
    match value {
        Value::String(line) => vec![line],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Removes terminal escape codes, such as colours, from a line of text.
/// It follows the pattern Node's `util.stripVTControlCharacters` uses.
pub fn strip_vt(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut stripped = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if matches!(chars[i], '\u{1b}' | '\u{9b}')
            && let Some(end) = escape_end(&chars, i + 1)
        {
            i = end;
            continue;
        }
        stripped.push(chars[i]);
        i += 1;
    }
    stripped
}

/// Where the escape code whose body starts at `start` ends, if it is one.
fn escape_end(chars: &[char], start: usize) -> Option<usize> {
    let prefix = chars[start..]
        .iter()
        .take_while(|c| matches!(c, '[' | ']' | '(' | ')' | '#' | ';' | '?'))
        .count();
    (0..=prefix)
        .rev()
        .find_map(|len| string_end(chars, start + len).or_else(|| control_end(chars, start + len)))
}

/// Where an escape string, such as a window title, that starts at `at`
/// ends with its terminator.
fn string_end(chars: &[char], at: usize) -> Option<usize> {
    let param = |c: &char| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '/' | '#' | '&' | '.' | ':' | '=' | '?' | '%' | '@' | '~' | '_'
            )
    };
    let terminator = |at: usize| match chars.get(at) {
        Some('\u{7}' | '\u{9c}') => Some(at + 1),
        Some('\u{1b}') if chars.get(at + 1) == Some(&'\\') => Some(at + 2),
        _ => None,
    };
    let mut end = at;
    while chars.get(end) == Some(&';') && chars.get(end + 1).is_some_and(param) {
        end += 1;
        end += chars[end..].iter().take_while(|c| param(c)).count();
    }
    if let Some(end) = terminator(end) {
        return Some(end);
    }
    let word = chars[at.min(chars.len())..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if word == 0 {
        return None;
    }
    let mut end = at + word;
    while chars.get(end) == Some(&';') {
        end += 1;
        end += chars[end..].iter().take_while(|c| param(c)).count();
    }
    terminator(end)
}

/// Where a control sequence, such as a colour, that starts at `at` ends:
/// up to four digit groups split by semicolons, then a final character.
fn control_end(chars: &[char], at: usize) -> Option<usize> {
    let last = |at: usize| {
        chars.get(at).is_some_and(|c| {
            c.is_ascii_digit()
                || matches!(c, 'A'..='P' | 'R'..='T' | 'Z' | 'c' | 'f'..='n' | 'q'..='u' | 'y')
                || matches!(c, '=' | '>' | '<' | '~')
        })
    };
    let digits = |from: usize| {
        chars[from.min(chars.len())..]
            .iter()
            .take(4)
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let first = digits(at);
    if first == 0 {
        return last(at).then_some(at + 1);
    }
    let mut groups = vec![(at, first)];
    let mut end = at + first;
    while chars.get(end) == Some(&';') {
        let count = digits(end + 1);
        groups.push((end + 1, count));
        end += 1 + count;
    }
    if last(end) {
        return Some(end + 1);
    }
    // Backtracking gives the last digit of the deepest group that can spare
    // one to the final character.
    for (index, (start, count)) in groups.into_iter().enumerate().rev() {
        if count > usize::from(index == 0) {
            return Some(start + count);
        }
    }
    Some(at + 1)
}

/// Collapses runs of whitespace to single spaces and drops NULs.
fn collapse_spaces(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_space = false;
    for c in value.chars().filter(|c| *c != '\0') {
        if js_space(c) {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    collapsed
}

/// Collapses three or more line breaks, with only spaces and tabs between
/// them, to two.
fn collapse_blank_lines(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut collapsed = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\n' {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        let mut blanks = 0;
        loop {
            let mut next = end;
            while next < bytes.len() && matches!(bytes[next], b' ' | b'\t') {
                next += 1;
            }
            if bytes.get(next) != Some(&b'\n') {
                break;
            }
            blanks += 1;
            end = next + 1;
        }
        if blanks >= 2 {
            collapsed.push_str(&text[copied..i]);
            collapsed.push_str("\n\n");
            copied = end;
            i = end;
        } else {
            i += 1;
        }
    }
    collapsed.push_str(&text[copied..]);
    collapsed
}

/// Whether JavaScript counts a character as whitespace.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Trims whitespace the way JavaScript's `trim` does.
fn js_trim(value: &str) -> &str {
    value.trim_matches(js_space)
}

/// Reads a number the way JavaScript's `Number` does, giving NaN for text
/// that isn't one.
fn js_number(value: &str) -> f64 {
    let value = js_trim(value);
    if value.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = value.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |n, c| {
                n * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or_default())
            });
        }
    }
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    if unsigned == "Infinity" {
        return if value.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    if !mantissa_ok || !exponent_ok {
        return f64::NAN;
    }
    value.parse().unwrap_or(f64::NAN)
}

/// The list index text stands for, the way JavaScript indexes an array
/// with `Number(text)`.
fn js_index(value: &str) -> Option<usize> {
    let number = js_number(value);
    (number >= 0.0 && number.fract() == 0.0 && number < 9e15).then_some(number as usize)
}

/// Decodes `%XX` escapes the way JavaScript's `decodeURIComponent` does,
/// failing on a bad escape or bytes that aren't UTF-8.
fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes.get(i + 1..i + 3)?;
            if !pair.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            decoded.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Tidies a path inside an archive the way `path.posix.normalize` does.
fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            ".." if absolute => {}
            segment => segments.push(segment),
        }
    }
    let mut normalized = segments.join("/");
    if normalized.is_empty() && !absolute {
        normalized.push('.');
    }
    if path.ends_with('/') && !normalized.is_empty() {
        normalized.push('/');
    }
    if absolute {
        normalized.insert(0, '/');
    }
    normalized
}

/// Joins paths inside an archive the way `path.posix.join` does.
fn join(paths: &[&str]) -> String {
    let joined = paths
        .iter()
        .copied()
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    normalize(&joined)
}

/// The folder a path inside an archive is in, the way `path.posix.dirname`
/// gives it.
fn dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "." } else { "/" };
    }
    match trimmed.rfind('/') {
        None => ".",
        Some(at) => match trimmed[..at].trim_end_matches('/') {
            "" => "/",
            parent => parent,
        },
    }
}

/// The last part of a path inside an archive.
fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
}

/// A rejection that names what's wrong with the document.
fn rejected(reason: &str) -> Stop {
    Stop::Rejected(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};

    use super::super::zip::build::Archive;
    use super::*;
    use crate::testing::TempDir;

    /// Reads a file holding `data`, saved with `extension`.
    fn read_file(extension: &str, data: &[u8]) -> Result<String, Failure> {
        let dir = TempDir::new();
        let path = dir.path().join(format!("document.{extension}"));
        fs::write(&path, data).expect("a document");
        extract(&path)
    }

    /// Reads an archive of `parts`, saved with `extension`.
    fn read_parts(extension: &str, parts: &[(&str, &[u8])]) -> Result<String, Failure> {
        let mut archive = Archive::default();
        for (name, data) in parts {
            archive.add(name, data, false, 0, None);
        }
        read_file(extension, &archive.finish())
    }

    /// The text of an archive of `parts`, saved with `extension`.
    #[track_caller]
    fn read(extension: &str, parts: &[(&str, &str)]) -> String {
        let parts: Vec<(&str, &[u8])> = parts
            .iter()
            .map(|(name, data)| (*name, data.as_bytes()))
            .collect();
        read_parts(extension, &parts).unwrap_or_else(|error| panic!("{extension}: {error}"))
    }

    /// Why a document was turned down.
    #[track_caller]
    fn rejection(result: Result<String, Failure>) -> String {
        match result {
            Err(Failure::Rejected(reason)) => reason,
            Err(error) => panic!("expected a rejection, got {error}"),
            Ok(text) => panic!("expected a rejection, got {} characters", text.len()),
        }
    }

    /// Checks that `text` holds each of `expected`, in order.
    #[track_caller]
    fn assert_in_order(text: &str, expected: &[&str]) {
        let mut from = 0;
        for want in expected {
            match text[from..].find(want) {
                Some(at) => from += at + want.len(),
                None => {
                    let start: String = text.chars().take(300).collect();
                    panic!("{want:?} missing after byte {from} of {start:?}");
                }
            }
        }
    }

    /// Checks that text cut short stays within the limits, with one notice.
    #[track_caller]
    fn assert_truncated(text: &str) {
        assert!(text.ends_with(TRUNCATED));
        assert_eq!(text.matches("[Extracted text truncated").count(), 1);
        assert!(text.chars().count() < MAX_OUTPUT + 100);
    }

    #[test]
    fn reads_word_headings_breaks_and_nested_tables() {
        let document = r#"<w:document xmlns:w="urn:w"><w:body><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Report &amp; totals</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>12</w:t></w:r></w:p><w:p><w:r><w:t>34</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>Before</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>After</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Neighbor</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        assert_eq!(
            read("DOCX", &[("word/document.xml", document)]),
            "## Report & totals\nNorth\t12 34\nBefore Inner After\tNeighbor"
        );
        let breaks = r#"<w:document xmlns:w="urn:w"><w:body><w:p><w:r><w:t>First</w:t><w:cr/><w:t>Second</w:t><w:br/><w:t>Third</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            read("docx", &[("word/document.xml", breaks)]),
            "First\nSecond\nThird"
        );
        let media = Archive::default()
            .stored(
                "word/document.xml",
                b"<document><p>Readable despite embedded media</p></document>",
            )
            .deflated("word/media/black.bmp", &[0; 64 * 1024]);
        let mut media = media;
        media.add(
            "word/embeddings/unused.bin",
            b"x",
            true,
            0,
            Some(33 * 1024 * 1024),
        );
        let text = read_file("docx", &media.finish()).expect("text");
        assert_eq!(text, "Readable despite embedded media");
    }

    #[test]
    fn reads_opendocument_text_sheets_and_slides() {
        let text = r#"<office:document><text:h text:outline-level="2">Orchard</text:h><text:p>19<text:s text:c="3"/>violet<text:s/>crates</text:p></office:document>"#;
        assert_eq!(
            read("odt", &[("content.xml", text)]),
            "## Orchard\n19   violet crates"
        );
        let sheet = r#"<office:document><table:table table:name="Cargo"><table:table-row><table:table-cell><text:p>Violet</text:p></table:table-cell><table:table-cell office:value="19"/><table:table-cell office:value="27"><text:p/></table:table-cell></table:table-row></table:table></office:document>"#;
        assert_eq!(
            read("ods", &[("content.xml", sheet)]),
            "## Sheet: Cargo\nViolet\t19\t27"
        );
        let padded = r#"<office:document><table:table table:name="Padded rows"><table:table-row><table:table-cell><text:p>Left</text:p></table:table-cell><table:table-cell table:number-columns-repeated="2"/><table:table-cell><text:p>Right</text:p></table:table-cell></table:table-row><table:table-row table:number-rows-repeated="1048575"><table:table-cell/></table:table-row></table:table></office:document>"#;
        assert_eq!(
            read("ods", &[("content.xml", padded)]),
            "## Sheet: Padded rows\nLeft\t\t\tRight"
        );
        let slides = "<office:document><draw:page><text:p>First</text:p></draw:page><draw:page><text:p>Second</text:p></draw:page></office:document>";
        assert_eq!(
            read("odp", &[("content.xml", slides)]),
            "## Slide 1\nFirst\n\n## Slide 2\nSecond"
        );
        let negative =
            r#"<office:document><text:h text:outline-level="-1">Sunk</text:h></office:document>"#;
        let result = read_parts("odt", &[("content.xml", negative.as_bytes())]);
        assert_eq!(rejection(result), "Invalid heading level");
    }

    #[test]
    fn shows_stored_odf_values_when_cells_have_no_text() {
        let content = r#"<document><page><table><table-row>
              <table-cell office:value-type="string" office:string-value="Harbor"/>
              <table-cell office:value-type="boolean" office:boolean-value="false"/>
              <table-cell office:value-type="date" office:date-value="2026-10-20"/>
              <table-cell office:value-type="time" office:time-value="PT2H30M"/>
              <table-cell office:value-type="currency" office:value="125.50" office:currency="USD"/>
              <table-cell office:value-type="percentage" office:value="0.25"/>
              <table-cell office:value-type="float" office:value="0"/>
              <table-cell office:value-type="string" office:string-value="Fallback"><p> &#10; </p></table-cell>
              <table-cell office:value-type="string" office:string-value="Unshown"><p>Displayed</p></table-cell>
              <table-cell office:value-type="void" office:value="Unshown"/>
              <table-cell office:value-type="currency" office:value=" " office:currency="Unshown"/>
              <table-cell office:value-type="percentage" office:value=" "/>
            </table-row></table></page></document>"#;
        let row = "Harbor\tfalse\t2026-10-20\tPT2H30M\t125.50 USD\t0.25 [ODF value-type=percentage]\t0\tFallback\tDisplayed";
        for extension in ["ods", "odt", "odp"] {
            let text = read(extension, &[("content.xml", content)]);
            assert_in_order(&text, &[row]);
            assert!(!text.contains("Unshown"), "{extension}");
        }
    }

    #[test]
    fn repeats_table_rows_and_columns_up_to_their_limits() {
        for extension in ["ods", "odt", "odp", "docx"] {
            let part = if extension == "docx" {
                "word/document.xml"
            } else {
                "content.xml"
            };
            for rows in [false, true] {
                let (row, cell) = if rows {
                    (r#" number-rows-repeated="10001""#, "")
                } else {
                    ("", r#" number-columns-repeated="513""#)
                };
                let content = format!("<document><page><table><table-row{row}><table-cell{cell}><p>Repeated value</p></table-cell></table-row></table></page></document>");
                let text = read(extension, &[(part, &content)]);
                let count = if rows { MAX_ROWS } else { MAX_COLUMNS };
                assert_eq!(text.matches("Repeated value").count(), count, "{extension}");
                assert_truncated(&text);
            }
        }
    }

    #[test]
    fn drops_padding_without_expanding_it() {
        let padding = r#"<document><page><table><table-row><table-cell><p>Kept</p></table-cell><table-cell number-columns-repeated="16383"> &#10; <p> &#9; </p> <s c="1048575"/> </table-cell></table-row><table-row number-rows-repeated="1048575"><table-cell> &#10; <p> &#9; </p> </table-cell></table-row></table></page></document>"#;
        for extension in ["ods", "odt", "odp", "docx"] {
            let part = if extension == "docx" {
                "word/document.xml"
            } else {
                "content.xml"
            };
            let text = read(extension, &[(part, padding)]);
            assert_in_order(&text, &["Kept"]);
            assert!(
                !text.contains('\t') && !text.contains("[Extracted"),
                "{extension}"
            );
        }
        let sheet = r#"<office:document><table:table table:name="Padded"><table:table-row><table:table-cell><text:p>Populated</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16383"/></table:table-row><table:table-row table:number-rows-repeated="1048575"><table:table-cell table:number-columns-repeated="16384"><text:p/></table:table-cell></table:table-row></table:table></office:document>"#;
        assert_eq!(
            read("ods", &[("content.xml", sheet)]),
            "## Sheet: Padded\nPopulated"
        );
        let spaces = format!(
            r#"<office:document><text:p>Before whitespace</text:p><table:table><table:table-row table:number-rows-repeated="10000"><table:table-cell table:number-columns-repeated="512"><text:p>{}</text:p></table:table-cell></table:table-row></table:table><text:p>Unneeded</text:p></office:document>"#,
            " ".repeat(10_000)
        );
        assert_eq!(
            read("odt", &[("content.xml", &spaces)]),
            "Before whitespace\nUnneeded"
        );
    }

    #[test]
    fn reads_excel_sheets_in_workbook_order_with_number_formats() {
        let parts = [
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="Inventory" r:id="a"/><sheet name="Budget" r:id="b"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="a" Target="worksheets/second.xml"/><Relationship Id="b" Target="worksheets/first.xml"/></Relationships>"#,
            ),
            ("xl/sharedStrings.xml", "<sst><si><t>Lantern</t></si></sst>"),
            (
                "xl/styles.xml",
                r#"<styleSheet><numFmts><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts><cellXfs><xf numFmtId="0"/><xf numFmtId="14"/><xf numFmtId="9"/><xf numFmtId="164"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/second.xml",
                r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="C1"><v>37</v></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/first.xml",
                r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>Ferry</t></is></c><c><v>628</v></c><c s="1"><v>45292</v></c><c s="2"><v>0.25</v></c><c s="3"><v>45293</v></c></row></sheetData></worksheet>"#,
            ),
        ];
        assert_eq!(
            read("xlsx", &parts),
            "## Sheet: Inventory\nLantern\t\t37\n\n## Sheet: Budget\nFerry\t628\t45292 [Excel numFmtId=14; date system 1900]\t0.25 [Excel numFmtId=9; date system 1900]\t45293 [Excel numFmtId=164; format=yyyy-mm-dd; date system 1900]"
        );
    }

    #[test]
    fn reads_excel_formulas_phonetic_guides_and_1904_dates() {
        let rels =
            r#"<Relationships><Relationship Id="a" Target="worksheets/a.xml"/></Relationships>"#;
        let formulas = [
            ("xl/workbook.xml", r#"<workbook><sheet name="Formulas" r:id="a"/></workbook>"#),
            ("xl/_rels/workbook.xml.rels", rels),
            ("xl/styles.xml", r#"<styleSheet><cellXfs><xf numFmtId="9"/></cellXfs></styleSheet>"#),
            ("xl/worksheets/a.xml", "<worksheet><row><c><f>SUM(11,22)</f><v/></c></row><row><c><f>SUM(33,44)</f><v> \n </v></c></row><row><c><f>SUM(55,66)</f></c></row><row><c><f>SUM(11,22)</f><v>33</v></c><c><f>SUM(0,0)</f><v>0</v></c></row></worksheet>"),
        ];
        assert_eq!(
            read("xlsx", &formulas),
            "## Sheet: Formulas\nSUM(11,22)\nSUM(33,44)\nSUM(55,66)\n33 [Excel numFmtId=9; date system 1900]\t0 [Excel numFmtId=9; date system 1900]"
        );
        let phonetics = [
            ("xl/workbook.xml", r#"<workbook><sheet name="Phonetics" r:id="a"/></workbook>"#),
            ("xl/_rels/workbook.xml.rels", rels),
            ("xl/sharedStrings.xml", "<sst><si><t>\u{6771}\u{4eac}</t><rPh sb=\"0\" eb=\"2\"><t>\u{3068}\u{3046}</t></rPh></si></sst>"),
            ("xl/worksheets/a.xml", "<worksheet><row><c t=\"s\"><v>0</v></c><c t=\"inlineStr\"><is><t>\u{6771}\u{4eac}</t><rPh sb=\"0\" eb=\"2\"><t>\u{3068}\u{3046}</t></rPh></is></c></row></worksheet>"),
        ];
        assert_eq!(
            read("xlsx", &phonetics),
            "## Sheet: Phonetics\n\u{6771}\u{4eac}\t\u{6771}\u{4eac}"
        );
        let dates = [
            (
                "xl/workbook.xml",
                r#"<workbook><workbookPr date1904="true"/><sheets><sheet name="Dates" r:id="a"/></sheets></workbook>"#,
            ),
            ("xl/_rels/workbook.xml.rels", rels),
            (
                "xl/styles.xml",
                r#"<styleSheet><cellXfs><xf numFmtId="14"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/a.xml",
                "<worksheet><sheetData><row><c><v>1</v></c></row></sheetData></worksheet>",
            ),
        ];
        assert_eq!(
            read("xlsx", &dates),
            "## Sheet: Dates\n1 [Excel numFmtId=14; date system 1904]"
        );
    }

    #[test]
    fn stops_excel_sheets_at_their_row_and_column_limits() {
        let workbook =
            |name: &str| format!(r#"<workbook><sheet name="{name}" r:id="a"/></workbook>"#);
        let rels =
            r#"<Relationships><Relationship Id="a" Target="worksheets/a.xml"/></Relationships>"#;
        let rows = format!(
            "<worksheet>{}</worksheet>",
            "<row><c><v>7</v></c></row>".repeat(10_001)
        );
        let text = read(
            "xlsx",
            &[
                ("xl/workbook.xml", &workbook("Limited")),
                ("xl/_rels/workbook.xml.rels", rels),
                ("xl/worksheets/a.xml", &rows),
            ],
        );
        assert_eq!(text.matches('7').count(), MAX_ROWS);
        assert_truncated(&text);
        let columns = r#"<worksheet><row><c r="A1"><v>7</v></c><c r="SS1"><v>Beyond</v></c></row></worksheet>"#;
        let text = read(
            "xlsx",
            &[
                ("xl/workbook.xml", &workbook("Limited")),
                ("xl/_rels/workbook.xml.rels", rels),
                ("xl/worksheets/a.xml", columns),
            ],
        );
        assert_eq!(text, format!("## Sheet: Limited\n7{TRUNCATED}"));
        let padding = format!(
            r#"<worksheet>{}<row><c t="s"><v>0</v></c><c t="s"><v>1</v></c><c r="XFD2"><v> &#10; </v></c></row></worksheet>"#,
            r#"<row><c t="s"><v>0</v></c><c r="XFD1" t="inlineStr"><is><t> &#9; </t></is></c></row>"#.repeat(10_001)
        );
        let text = read(
            "xlsx",
            &[
                ("xl/workbook.xml", &workbook("Padding")),
                ("xl/_rels/workbook.xml.rels", rels),
                (
                    "xl/sharedStrings.xml",
                    "<sst><si><t> \n </t></si><si><t>Present</t></si></sst>",
                ),
                ("xl/worksheets/a.xml", &padding),
            ],
        );
        assert_eq!(text, "## Sheet: Padding\n\tPresent");
    }

    #[test]
    fn reads_slides_in_deck_order_with_their_notes() {
        let notes =
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
        let layout =
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
        let slide2_rels = format!(
            r#"<Relationships><Relationship Type="{layout}" Target="../notesSlides/ignored.xml"/><Relationship Type="{notes}" TargetMode="External" Target="https://example.com/notes.xml"/><Relationship Type="{notes}" Target="../notesSlides/notes7.xml"/></Relationships>"#
        );
        let parts = [
            (
                "ppt/presentation.xml",
                r#"<p:presentation><p:sldIdLst><p:sldId id="1" r:id="b"/><p:sldId id="2" r:id="a"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="a" Target="slides/slide1.xml"/><Relationship Id="b" Target="slides/slide2.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                "<p:sld><a:p><a:r><a:t>Departure</a:t></a:r></a:p></p:sld>",
            ),
            (
                "ppt/slides/slide2.xml",
                "<p:sld><a:p><a:r><a:t>Juniper Pier</a:t></a:r></a:p></p:sld>",
            ),
            ("ppt/slides/_rels/slide2.xml.rels", &slide2_rels),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                r#"<Relationships><Relationship Type="http://purl.oclc.org/ooxml/officeDocument/relationships/notesSlide" Target="/ppt/notesSlides/notes3.xml"/></Relationships>"#,
            ),
            (
                "ppt/notesSlides/notes7.xml",
                "<p:notes><a:p><a:r><a:t>evacuation code ORCHID-81</a:t></a:r></a:p></p:notes>",
            ),
            (
                "ppt/notesSlides/notes3.xml",
                "<p:notes><a:p><a:r><a:t>Board at gate 4</a:t></a:r></a:p></p:notes>",
            ),
            (
                "ppt/notesSlides/ignored.xml",
                "<p:notes><a:p>Unrelated notes</a:p></p:notes>",
            ),
        ];
        assert_eq!(
            read("pptx", &parts),
            "## Slide 1\nJuniper Pier\nNotes:\nevacuation code ORCHID-81\n\n## Slide 2\nDeparture\nNotes:\nBoard at gate 4"
        );
    }

    #[test]
    fn notes_parts_that_are_missing_or_broken() {
        for name in ["missing", "bad"] {
            let rels = format!(
                r#"<Relationships><Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/{name}.xml"/></Relationships>"#
            );
            let parts = [
                (
                    "ppt/presentation.xml",
                    r#"<presentation><sldId r:id="a"/></presentation>"#,
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    r#"<Relationships><Relationship Id="a" Target="slides/a.xml"/></Relationships>"#,
                ),
                ("ppt/slides/a.xml", "<slide><p>Slide text</p></slide>"),
                ("ppt/slides/_rels/a.xml.rels", &rels),
                (
                    "ppt/notesSlides/bad.xml",
                    "<notes><p>Discard broken notes</notes>",
                ),
            ];
            assert_eq!(
                read("pptx", &parts),
                format!("## Slide 1\nSlide text\nNotes:\n\n[Skipped document part: ppt/notesSlides/{name}.xml (missing or invalid XML).]")
            );
        }
        let deck = read(
            "pptx",
            &[
                (
                    "ppt/presentation.xml",
                    r#"<presentation><sldId r:id="bad"/><sldId r:id="good"/></presentation>"#,
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    r#"<Relationships><Relationship Id="bad" Target="bad.xml"/><Relationship Id="good" Target="good.xml"/></Relationships>"#,
                ),
                ("ppt/bad.xml", "<slide><p>Discard</slide>"),
                ("ppt/good.xml", "<slide><p>Healthy part</p></slide>"),
            ],
        );
        let book = read(
            "xlsx",
            &[
                (
                    "xl/workbook.xml",
                    r#"<workbook><sheet name="Bad" r:id="bad"/><sheet name="Good" r:id="good"/><sheet name="Gone" r:id="gone"/></workbook>"#,
                ),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<Relationships><Relationship Id="bad" Target="bad.xml"/><Relationship Id="good" Target="good.xml"/></Relationships>"#,
                ),
                ("xl/bad.xml", "<worksheet><row>Discard</worksheet>"),
                (
                    "xl/good.xml",
                    r#"<worksheet><row><c t="inlineStr"><is><t>Healthy part</t></is></c></row></worksheet>"#,
                ),
            ],
        );
        for (text, part) in [(deck, "ppt/bad.xml"), (book, "xl/bad.xml")] {
            let note = format!("[Skipped document part: {part} (missing or invalid XML).]");
            assert_in_order(&text, &["Healthy part", &note]);
            assert!(!text.contains("Discard"), "{part}");
        }
        let book = read(
            "epub",
            &[
                (
                    "META-INF/container.xml",
                    r#"<container><rootfile full-path="book.opf"/></container>"#,
                ),
                (
                    "book.opf",
                    r#"<package><item id="bad" href="ch%ZZ.xhtml"/><item id="good" href="good.svg"/><itemref idref="bad"/><itemref idref="good"/></package>"#,
                ),
                (
                    "good.svg",
                    r#"<svg xmlns="http://www.w3.org/2000/svg"><text>Readable SVG chapter</text></svg>"#,
                ),
            ],
        );
        assert_eq!(
            book,
            "## Chapter 1\n\n## Chapter 2\nReadable SVG chapter\n[Skipped document part: chapter 1 (missing or invalid XML).]"
        );
        let package = read("epub", &[("META-INF/container.xml", "<container/>")]);
        assert_eq!(
            package,
            "[Skipped document part: EPUB package (missing or invalid XML).]"
        );
    }

    #[test]
    fn reads_epub_chapters_in_spine_order() {
        let parts = [
            ("META-INF/container.xml", r#"<container><rootfile full-path="OEBPS/book.opf"/></container>"#),
            ("OEBPS/book.opf", r#"<package><manifest><item id="a" href="a.xhtml"/><item id="b" href="b%20b.xhtml#start"/></manifest><spine><itemref idref="b"/><itemref idref="a"/></spine></package>"#),
            ("OEBPS/a.xhtml", r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd"><html><body><p>Last chapter</p><p>Price <s>10</s> 8</p></body></html>"#),
            ("OEBPS/b b.xhtml", "<!DOCTYPE html><html><body><h1>First chapter</h1><p>Larkspur</p><div>Direct div<section>Direct section<li>Direct item <em>emphasis</em> trailing</li></section></div><div>Account: A<br/>Balance: 731<line-break/>Currency: USD</div><table><thead><tr><th>Heading</th><th>Total</th></tr></thead><tbody><tr><td>Important value</td><td>731</td></tr><tr><td>Before<table><tr><td>Inner</td></tr></table>After</td><td>Neighbor</td></tr></tbody></table></body></html>"),
        ];
        assert_eq!(
            read("epub", &parts),
            "## Chapter 1\n# First chapter\nLarkspur\nDirect divDirect sectionDirect item emphasis trailing\n\nAccount: A\nBalance: 731\nCurrency: USD\nHeading\tTotal\nImportant value\t731\nBefore Inner After\tNeighbor\n\n## Chapter 2\nLast chapter\nPrice 10 8"
        );
    }

    #[test]
    fn reads_utf16_epub_parts() {
        for bom in [false, true] {
            for big_endian in [false, true] {
                let encode = |source: &str| -> Vec<u8> {
                    let source = format!("{}{source}", if bom { "\u{feff}" } else { "" });
                    source
                        .encode_utf16()
                        .flat_map(|unit| {
                            if big_endian {
                                unit.to_be_bytes()
                            } else {
                                unit.to_le_bytes()
                            }
                        })
                        .collect()
                };
                let declaration = r#"<?xml version="1.0" encoding="UTF-16"?>"#;
                let container = encode(&format!(
                    r#"{declaration}<container><rootfile full-path="book.opf"/></container>"#
                ));
                let package = encode(&format!(
                    r#"{declaration}<package><item id="a" href="a.xhtml"/><itemref idref="a"/></package>"#
                ));
                let chapter = encode(&format!("{declaration}<html><p>UTF-16 chapter: caf\u{e9} \u{65e5}\u{672c}\u{8a9e}</p></html>"));
                let parts: [(&str, &[u8]); 3] = [
                    ("META-INF/container.xml", &container),
                    ("book.opf", &package),
                    ("a.xhtml", &chapter),
                ];
                let text = read_parts("epub", &parts).expect("text");
                assert_eq!(
                    text,
                    "## Chapter 1\nUTF-16 chapter: caf\u{e9} \u{65e5}\u{672c}\u{8a9e}"
                );
            }
        }
    }

    #[test]
    fn reads_html_entities_in_epub_chapters() {
        let parts = [
            (
                "META-INF/container.xml",
                r#"<container><rootfile full-path="book.opf"/></container>"#,
            ),
            (
                "book.opf",
                r#"<package><item id="bad" href="bad.xhtml"/><item id="good" href="good.xhtml"/><itemref idref="bad"/><itemref idref="good"/></package>"#,
            ),
            ("bad.xhtml", "<html><p>Discard this broken chapter</html>"),
            (
                "good.xhtml",
                r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd"><html><p>North&nbsp;Harbor &NotEqualTilde; &CounterClockwiseContourIntegral; &amp; &lt;tag&gt; &quot;quoted&quot;</p><p><![CDATA[Literal &nbsp;]]></p><table><tr><td>&nbsp; </td></tr><tr><td>Kept&nbsp;cell</td></tr></table></html>"#,
            ),
        ];
        // Entities outside the short list stay as written.
        assert_eq!(
            read("epub", &parts),
            "## Chapter 1\n\n## Chapter 2\nNorth\u{a0}Harbor &NotEqualTilde; &CounterClockwiseContourIntegral; & <tag> \"quoted\"\nLiteral &nbsp;\nKept cell\n\n[Skipped document part: bad.xhtml (missing or invalid XML).]"
        );
    }

    #[test]
    fn reads_notebook_cells_outputs_and_tracebacks() {
        let notebook = r##"{"cells":[{"cell_type":"markdown","source":["# Survey\n","Vessel: Larkspur"]},{"cell_type":"code","source":["print(43 * 7)"],"outputs":[{"text":["301\n"]},{"data":{"text/plain":["42"]}},{"output_type":"error","ename":"ZeroDivisionError","evalue":"division by zero","traceback":["\u001b[32mCell In[1], line 1\u001b[0m","\u001b[31m----> 1 1 / 0\u001b[0m","ZeroDivisionError: division by zero"]}]}]}"##;
        assert_eq!(
            read_file("ipynb", notebook.as_bytes()).expect("text"),
            "## Cell 1 (markdown)\n# Survey\nVessel: Larkspur\n\n## Cell 2 (code)\nprint(43 * 7)\nOutput:\n301\n\n42\nZeroDivisionError: division by zero\nCell In[1], line 1\n----> 1 1 / 0\nZeroDivisionError: division by zero"
        );
        assert_eq!(
            rejection(read_file("ipynb", br#"{"cells":3}"#)),
            "Invalid notebook"
        );
        assert_eq!(
            rejection(read_file("ipynb", b"invalid document")),
            "Invalid notebook"
        );
    }

    #[test]
    fn stops_notebooks_at_their_limits() {
        let cells = vec![r#"{"source":"Kept"}"#; 10_001].join(",");
        let text =
            read_file("ipynb", format!(r#"{{"cells":[{cells}]}}"#).as_bytes()).expect("text");
        assert!(text.contains("## Cell 10000 (markdown)\nKept"));
        assert!(!text.contains("## Cell 10001"));
        assert_truncated(&text);
        let notebook = format!(
            r#"{{"cells":[{{"cell_type":"code","source":"1 / 0","outputs":[{{"output_type":"error","ename":"ZeroDivisionError","evalue":"division by zero","traceback":["\u001b[31m{}\u001b[0m","After limit"]}}]}}]}}"#,
            "trace".repeat(250_000)
        );
        let text = read_file("ipynb", notebook.as_bytes()).expect("text");
        assert!(text.contains("ZeroDivisionError: division by zero\ntrace"));
        assert!(!text.contains("After limit") && !text.contains('\u{1b}'));
        assert_truncated(&text);
    }

    #[test]
    fn strips_terminal_codes() {
        let cases = [
            ("\u{1b}[32mgreen\u{1b}[0m", "green"),
            ("\u{1b}[1;31mbold\u{1b}[m", "bold"),
            ("\u{1b}]0;title\u{7}after", "after"),
            (
                "\u{1b}]8;;https://example.com\u{1b}\\link\u{1b}]8;;\u{1b}\\",
                "link",
            ),
            ("\u{9b}2Jclear", "clear"),
            ("\u{1b}[12345rest", "rest"),
            ("\u{1b}[1;2;x", ";x"),
            ("keep \u{1b} alone", "keep \u{1b} alone"),
        ];
        for (line, stripped) in cases {
            assert_eq!(strip_vt(line), stripped, "{line:?}");
        }
    }

    #[test]
    fn reads_rtf_documents() {
        let read_rtf = |source: &[u8]| read_file("rtf", source);
        let text = read_rtf(
            br"{\rtf1\ansi{\fonttbl hidden}Hello\tab world\par Unicode \u945?{\*\secret hidden}}",
        );
        assert_eq!(text.expect("text"), "Hello\tworld\nUnicode \u{3b1}");
        // Code pages other than windows-1252 read their high bytes as U+FFFD.
        let cyrillic = br"{\rtf1\ansi\ansicpg1251 \'cf\'f0\'e8\'e2\'e5\'f2 {\ansicpg1252 \'e9} \'cc\'ee\'f1\'ea\'e2\'e0}";
        let unknown = "\u{fffd}".repeat(6);
        assert_eq!(
            read_rtf(cyrillic).expect("text"),
            format!("{unknown} \u{e9} {unknown}")
        );
        let raw = b"{\\rtf1\\ansi\\ansicpg1251 \xcf\xf0\xe8\xe2\xe5\xf2}";
        assert_eq!(read_rtf(raw).expect("text"), unknown);
        let deep = format!(r"{{\rtf1 Kept{}Lost{}", "{".repeat(128), "}".repeat(129));
        assert_eq!(
            read_rtf(deep.as_bytes()).expect("text"),
            format!("Kept{TRUNCATED}")
        );
        let code_page = rejection(read_rtf(br"{\rtf1\ansi\ansicpg932 \'82\'a0}"));
        assert_eq!(code_page, "Unsupported RTF code page");
        let long = format!(r"{{\rtf1 {}}}", "x".repeat(1_000_100));
        assert_truncated(&read_rtf(long.as_bytes()).expect("text"));
        let spaces = format!(r"{{\rtf1 First{}Last}}", " ".repeat(1_000_100));
        assert_eq!(
            read_rtf(spaces.as_bytes()).expect("text"),
            format!("First{TRUNCATED}")
        );
    }

    #[test]
    fn cuts_text_at_the_output_limit() {
        let long = "A".repeat(1_000_100);
        let paragraph = format!(
            "<w:document><w:p><w:t>{long}</w:t></w:p><w:p><w:t>Unneeded</w:t></w:p></w:document>"
        );
        let text = read("docx", &[("word/document.xml", &paragraph)]);
        assert_eq!(text, format!("{}{TRUNCATED}", &long[..MAX_OUTPUT]));
        let cell = format!(
            "<w:document><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Later cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>",
            &long[..MAX_OUTPUT]
        );
        let text = read("docx", &[("word/document.xml", &cell)]);
        assert_eq!(text, format!("{}{TRUNCATED}", &long[..MAX_OUTPUT]));
        let spaces = r#"<document><p>Start<s c="1000001"/>Unneeded</p></document>"#;
        assert_eq!(
            read("odt", &[("content.xml", spaces)]),
            format!("Start{TRUNCATED}")
        );
        let slide = format!("<p:sld><a:p><a:t>{long}</a:t></a:p></p:sld>");
        let deck = read(
            "pptx",
            &[
                (
                    "ppt/presentation.xml",
                    r#"<p:presentation><p:sldId r:id="a"/><p:sldId r:id="missing"/></p:presentation>"#,
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    r#"<Relationships><Relationship Id="a" Target="slides/slide.xml"/></Relationships>"#,
                ),
                ("ppt/slides/slide.xml", &slide),
            ],
        );
        assert_eq!(
            deck,
            format!("## Slide 1\n{}{TRUNCATED}", &long[..MAX_OUTPUT - 13])
        );
        let chapter = format!("<html><body><p>{long}</p></body></html>");
        let book = read(
            "epub",
            &[
                (
                    "META-INF/container.xml",
                    r#"<container><rootfile full-path="book.opf"/></container>"#,
                ),
                (
                    "book.opf",
                    r#"<package><item id="a" href="a.xhtml"/><itemref idref="a"/><itemref idref="missing"/></package>"#,
                ),
                ("a.xhtml", &chapter),
            ],
        );
        assert_eq!(
            book,
            format!("## Chapter 1\n{}{TRUNCATED}", &long[..MAX_OUTPUT - 15])
        );
        let strings = format!("<sst><si><t>{}</t></si></sst>", &long[..MAX_OUTPUT - 19]);
        let rows = r#"<row><c t="s"><v>0</v></c><c t="s"><v>0</v></c></row>"#.repeat(1_500);
        let sheet = read(
            "xlsx",
            &[
                (
                    "xl/workbook.xml",
                    r#"<workbook><sheets><sheet name="Repeated" r:id="a"/><sheet name="Unneeded" r:id="missing"/></sheets></workbook>"#,
                ),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<Relationships><Relationship Id="a" Target="worksheets/repeated.xml"/></Relationships>"#,
                ),
                ("xl/sharedStrings.xml", &strings),
                (
                    "xl/worksheets/repeated.xml",
                    &format!("<worksheet><sheetData>{rows}</sheetData></worksheet>"),
                ),
            ],
        );
        assert_eq!(
            sheet,
            format!(
                "## Sheet: Repeated\n{}{TRUNCATED}",
                &long[..MAX_OUTPUT - 20]
            )
        );
    }

    #[test]
    fn cuts_long_slides_notes_and_repeated_cells() {
        let notes = format!(
            "<notes><p>evacuation code ORCHID-81 {} Unneeded notes</p></notes>",
            "B".repeat(600_000)
        );
        let slide = format!("<slide><p>{}</p></slide>", "A".repeat(600_000));
        let deck = read(
            "pptx",
            &[
                (
                    "ppt/presentation.xml",
                    r#"<presentation><sldId r:id="a"/><sldId r:id="b"/></presentation>"#,
                ),
                (
                    "ppt/_rels/presentation.xml.rels",
                    r#"<Relationships><Relationship Id="a" Target="slides/a.xml"/><Relationship Id="b" Target="slides/b.xml"/></Relationships>"#,
                ),
                ("ppt/slides/a.xml", &slide),
                ("ppt/slides/b.xml", "<slide><p>Unneeded slide</p></slide>"),
                (
                    "ppt/slides/_rels/a.xml.rels",
                    r#"<Relationships><Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/a.xml"/></Relationships>"#,
                ),
                ("ppt/notesSlides/a.xml", &notes),
            ],
        );
        assert_in_order(
            &deck,
            &[
                "## Slide 1",
                &"A".repeat(32),
                "Notes:\nevacuation code ORCHID-81",
                &"B".repeat(32),
            ],
        );
        assert!(!deck.contains("Unneeded") && !deck.contains("## Slide 2"));
        assert_truncated(&deck);
        let cells = format!(
            r#"<office:document><table:table table:name="Repeated"><table:table-row><table:table-cell table:number-columns-repeated="512"><text:p>Lead {} Tail</text:p></table:table-cell></table:table-row></table:table></office:document>"#,
            "a".repeat(600_000)
        );
        let sheet = read("ods", &[("content.xml", &cells)]);
        assert_in_order(&sheet, &["## Sheet: Repeated", "Lead ", " Tail\tLead "]);
        assert_truncated(&sheet);
        let tables = (0..256)
            .map(|index| {
                let name = if index == 0 { "Shared" } else { "Unneeded" };
                format!(r#"<table:table table:name="{name}"><table:table-row><table:table-cell table:number-columns-repeated="512"><text:p>{}</text:p></table:table-cell></table:table-row></table:table>"#, "B".repeat(4096))
            })
            .collect::<String>();
        let sheets = read(
            "ods",
            &[(
                "content.xml",
                &format!("<office:document>{tables}</office:document>"),
            )],
        );
        assert_in_order(&sheets, &["## Sheet: Shared", &"B".repeat(32)]);
        assert!(!sheets.contains("## Sheet: Unneeded"));
        assert_truncated(&sheets);
        for extension in ["odt", "odp", "ods"] {
            for rows in [false, true] {
                let (repeat, words) = if rows {
                    (r#" table:number-rows-repeated="10000""#, 160)
                } else {
                    ("", 585)
                };
                let table = format!(
                    r#"<table:table><table:table-row{repeat}><table:table-cell table:number-columns-repeated="512"><text:p>{}</text:p></table:table-cell></table:table-row></table:table>"#,
                    "Budget ".repeat(words)
                );
                let content = format!(
                    "<office:document><draw:page>{}</draw:page><draw:page><text:p>Unneeded</text:p></draw:page></office:document>",
                    table.repeat(256)
                );
                let text = read(extension, &[("content.xml", &content)]);
                assert_in_order(&text, &["Budget "]);
                assert!(!text.contains("Unneeded"), "{extension}");
                assert_truncated(&text);
            }
        }
    }

    #[test]
    fn stops_at_deep_or_large_parts() {
        let deep = format!(
            "<document>{}Lost{}</document>",
            "<div>".repeat(129),
            "</div>".repeat(129)
        );
        let wide = format!("<document>{}</document>", "<p/>".repeat(200_001));
        for document in [deep, wide] {
            assert_eq!(read("docx", &[("word/document.xml", &document)]), TRUNCATED);
        }
    }

    #[test]
    fn stops_at_large_files_and_archives() {
        let document = b"<document><p>Readable</p></document>";
        let crowded = Archive::default()
            .stored("word/document.xml", document)
            .claim(2049)
            .finish();
        assert_eq!(read_file("docx", &crowded).expect("text"), TRUNCATED);
        let mut large = Archive::default();
        large.add(
            "word/document.xml",
            document,
            true,
            0,
            Some(33 * 1024 * 1024),
        );
        assert_eq!(read_file("docx", &large.finish()).expect("text"), TRUNCATED);
        let padded = format!(
            "<document>{}<p>Readable</p></document>",
            " ".repeat(300_000)
        );
        let bomb = Archive::default()
            .deflated("word/document.xml", padded.as_bytes())
            .finish();
        assert_eq!(read_file("docx", &bomb).expect("text"), TRUNCATED);
        let dir = TempDir::new();
        let path = dir.path().join("large.rtf");
        File::create(&path)
            .and_then(|file| file.set_len(17 * 1024 * 1024))
            .expect("a large file");
        assert_eq!(extract(&path).expect("text"), TRUNCATED);
    }

    #[test]
    fn rejects_documents_it_cannot_read() {
        let empty = read_parts(
            "docx",
            &[("word/document.xml", b"<document><p> </p></document>")],
        );
        assert_eq!(rejection(empty), "No text extracted");
        assert_eq!(
            rejection(extract(Path::new("notes.txt"))),
            "Unsupported document"
        );
        assert_eq!(
            rejection(extract(Path::new("notes"))),
            "Unsupported document"
        );
        assert!(matches!(
            read_file("docx", b"invalid document"),
            Err(Failure::Rejected(_))
        ));
        let mut encrypted = Archive::default();
        encrypted.add(
            "word/document.xml",
            b"<document><p>Secret</p></document>",
            false,
            1,
            None,
        );
        assert_eq!(
            rejection(read_file("docx", &encrypted.finish())),
            "Encrypted archive"
        );
        let dir = TempDir::new();
        let missing = extract(&dir.path().join("missing.docx")).expect_err("no file");
        assert!(matches!(&missing, Failure::Io(error) if error.kind() == io::ErrorKind::NotFound));
        assert!(missing
            .to_string()
            .starts_with("couldn't read the document: "));
    }

    #[test]
    fn tidies_blank_lines_and_nul_characters() {
        let mut out = Out::new();
        out.text("\n\0a\n\n \t\n\n b \n\n").expect("room");
        assert_eq!(out.finish(false).expect("text"), "a\n\n b");
        let mut out = Out::new();
        out.text(" \n\t\u{3000}").expect("room");
        assert!(
            matches!(out.finish(false), Err(Failure::Rejected(reason)) if reason == "No text extracted")
        );
        assert_eq!(Out::new().finish(true).expect("text"), TRUNCATED);
    }

    #[test]
    fn follows_javascript_number_and_uri_rules() {
        let numbers = [
            ("", 0.0),
            (" 12 ", 12.0),
            ("0x1f", 31.0),
            ("0b101", 5.0),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("-2", -2.0),
        ];
        for (text, number) in numbers {
            assert_eq!(js_number(text), number, "{text:?}");
        }
        assert_eq!(js_number("-Infinity"), f64::NEG_INFINITY);
        for text in ["1e", "abc", "0x", "--1", "1_0", "inf", "."] {
            assert!(js_number(text).is_nan(), "{text:?}");
        }
        assert_eq!(js_index("1.0"), Some(1));
        assert_eq!(js_index("-0"), Some(0));
        assert_eq!(js_index("0.5"), None);
        assert_eq!(js_index("x"), None);
        assert_eq!(
            decode_uri_component("ch%20a.xhtml").as_deref(),
            Some("ch a.xhtml")
        );
        assert_eq!(
            decode_uri_component("caf%C3%A9").as_deref(),
            Some("caf\u{e9}")
        );
        for text in ["%ZZ", "%e9", "%4", "%+1"] {
            assert_eq!(decode_uri_component(text), None, "{text:?}");
        }
    }

    #[test]
    fn handles_archive_paths_like_posix() {
        assert_eq!(
            normalize("xl/worksheets/../sharedStrings.xml"),
            "xl/sharedStrings.xml"
        );
        assert_eq!(normalize("ppt/../../x"), "../x");
        assert_eq!(normalize("/a/../../b"), "/b");
        assert_eq!(normalize("a/./b/"), "a/b/");
        assert_eq!(normalize(""), ".");
        assert_eq!(
            join(&["ppt/slides", "../notesSlides/n.xml"]),
            "ppt/notesSlides/n.xml"
        );
        assert_eq!(join(&[".", "a.xhtml"]), "a.xhtml");
        assert_eq!(join(&["", ""]), ".");
        assert_eq!(dirname("book.opf"), ".");
        assert_eq!(dirname("OEBPS/book.opf"), "OEBPS");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(basename("ppt/slides/slide1.xml"), "slide1.xml");
    }
}
