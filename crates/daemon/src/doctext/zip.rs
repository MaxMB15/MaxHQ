// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

//! A small ZIP reader for the extractor, ported from the archive step in
//! MIT-licensed code, see THIRD_PARTY_NOTICES, which used yauzl. It reads
//! the central directory, then inflates only the parts that hold a
//! document's text, within the worker's limits. As with yauzl, CRCs aren't
//! checked. Names that aren't marked UTF-8 are read as UTF-8 anyway, where
//! yauzl reads them as code page 437, since documents only name their parts
//! in ASCII.

use std::collections::HashMap;
use std::io::Read;

use flate2::read::DeflateDecoder;

use super::extract::{Stop, MAX_EXPANDED};

/// The most entries an archive can have.
const MAX_ENTRIES: u64 = 2048;

/// How many times its packed size a part can grow to.
const MAX_RATIO: u64 = 200;

/// The size any part can grow to, however small it's packed.
const RATIO_FLOOR: u64 = 1024;

/// The endings of the parts that hold text. Pictures, fonts and embedded
/// files are never inflated.
const TEXT_PARTS: &[&str] = &[".xml", ".rels", ".xhtml", ".html", ".htm", ".opf", ".svg"];

/// Marks the end of the central directory.
const END_SIGNATURE: u32 = 0x0605_4b50;

/// The size of the end record without its comment.
const END_LEN: usize = 22;

/// Marks the locator a ZIP64 archive puts just before its end record.
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;

/// The size of the ZIP64 locator.
const ZIP64_LOCATOR_LEN: usize = 20;

/// Marks the ZIP64 end record.
const ZIP64_END_SIGNATURE: u32 = 0x0606_4b50;

/// The extra field that holds an entry's ZIP64 sizes and offset.
const ZIP64_EXTRA: u16 = 0x0001;

/// Marks an entry in the central directory.
const ENTRY_SIGNATURE: u32 = 0x0201_4b50;

/// The size of a central directory entry before its name.
const ENTRY_LEN: usize = 46;

/// Marks an entry's local header.
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;

/// The size of a local header before its name.
const LOCAL_LEN: usize = 30;

/// The flag an encrypted entry has.
const ENCRYPTED: u16 = 1;

/// Stored without compression.
const STORED: u16 = 0;

/// Packed with deflate.
const DEFLATED: u16 = 8;

/// One entry from the central directory.
struct Entry {
    /// The path inside the archive.
    name: String,
    /// The general purpose flags.
    flags: u16,
    /// How the data is packed.
    method: u16,
    /// The size of the packed data.
    packed: u64,
    /// The size once unpacked.
    size: u64,
    /// Where the local header starts.
    offset: u64,
}

/// Reads the text parts of the archive in `data`, by name. A part that
/// starts with a UTF-16 byte order mark, or with `<` in UTF-16, is read as
/// UTF-16, and any other as UTF-8.
pub(super) fn parts(data: &[u8]) -> Result<HashMap<String, String>, Stop> {
    let (count, mut at) = directory(data)?;
    if count > MAX_ENTRIES {
        return Err(Stop::Truncated);
    }
    let mut files = HashMap::new();
    let mut expanded = 0u64;
    for _ in 0..count {
        let (entry, next) = entry(data, at)?;
        at = next;
        if entry.flags & ENCRYPTED != 0 {
            return Err(rejected("Encrypted archive"));
        }
        if !wanted(&entry.name) {
            continue;
        }
        if entry.size > MAX_EXPANDED || entry.size > RATIO_FLOOR.max(entry.packed * MAX_RATIO) {
            return Err(Stop::Truncated);
        }
        expanded += entry.size;
        if expanded > MAX_EXPANDED {
            return Err(Stop::Truncated);
        }
        let bytes = contents(data, &entry)?;
        files.insert(entry.name, decode(&bytes));
    }
    Ok(files)
}

/// Whether a part might hold text, by its name.
fn wanted(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    TEXT_PARTS.iter().any(|ending| name.ends_with(ending))
}

/// Finds the central directory: how many entries it has and where it
/// starts.
fn directory(data: &[u8]) -> Result<(u64, usize), Stop> {
    if data.len() < END_LEN {
        return Err(rejected("Not a ZIP archive"));
    }
    let lowest = data.len().saturating_sub(END_LEN + usize::from(u16::MAX));
    let end = (lowest..=data.len() - END_LEN)
        .rev()
        .find(|at| u32_at(data, *at) == Some(END_SIGNATURE))
        .ok_or_else(|| rejected("Not a ZIP archive"))?;
    let disk = u16_at(data, end + 4).unwrap_or(0);
    let start_disk = u16_at(data, end + 6).unwrap_or(0);
    if disk != 0 || start_disk != 0 {
        return Err(rejected("Multi-disk archives aren't supported"));
    }
    let mut count = u64::from(u16_at(data, end + 10).unwrap_or(0));
    let mut offset = u64::from(u32_at(data, end + 16).unwrap_or(0));
    if count == u64::from(u16::MAX) || offset == u64::from(u32::MAX) {
        let locator = end
            .checked_sub(ZIP64_LOCATOR_LEN)
            .filter(|at| u32_at(data, *at) == Some(ZIP64_LOCATOR_SIGNATURE))
            .ok_or_else(|| rejected("Missing ZIP64 locator"))?;
        let record = u64_at(data, locator + 8).ok_or_else(|| rejected("Missing ZIP64 record"))?;
        let record = usize::try_from(record).map_err(|_| rejected("Missing ZIP64 record"))?;
        if u32_at(data, record) != Some(ZIP64_END_SIGNATURE) {
            return Err(rejected("Missing ZIP64 record"));
        }
        count = u64_at(data, record + 32).ok_or_else(|| rejected("Missing ZIP64 record"))?;
        offset = u64_at(data, record + 48).ok_or_else(|| rejected("Missing ZIP64 record"))?;
    }
    let offset = usize::try_from(offset).map_err(|_| rejected("Invalid central directory"))?;
    Ok((count, offset))
}

/// Reads the central directory entry at `at`, and where the next starts.
fn entry(data: &[u8], at: usize) -> Result<(Entry, usize), Stop> {
    let bad = || rejected("Invalid central directory");
    if u32_at(data, at) != Some(ENTRY_SIGNATURE) {
        return Err(bad());
    }
    let flags = u16_at(data, at + 8).ok_or_else(bad)?;
    let method = u16_at(data, at + 10).ok_or_else(bad)?;
    let mut packed = u64::from(u32_at(data, at + 20).ok_or_else(bad)?);
    let mut size = u64::from(u32_at(data, at + 24).ok_or_else(bad)?);
    let name_len = usize::from(u16_at(data, at + 28).ok_or_else(bad)?);
    let extra_len = usize::from(u16_at(data, at + 30).ok_or_else(bad)?);
    let comment_len = usize::from(u16_at(data, at + 32).ok_or_else(bad)?);
    let mut offset = u64::from(u32_at(data, at + 42).ok_or_else(bad)?);
    let name_at = at + ENTRY_LEN;
    let extra_at = name_at + name_len;
    let next = extra_at + extra_len + comment_len;
    let name = data.get(name_at..extra_at).ok_or_else(bad)?;
    let extra = data.get(extra_at..extra_at + extra_len).ok_or_else(bad)?;
    if next > data.len() {
        return Err(bad());
    }
    if packed == u64::from(u32::MAX) || size == u64::from(u32::MAX) || offset == u64::from(u32::MAX)
    {
        let mut values = zip64(extra)
            .ok_or_else(bad)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| u64::from_le_bytes(*chunk));
        if size == u64::from(u32::MAX) {
            size = values.next().ok_or_else(bad)?;
        }
        if packed == u64::from(u32::MAX) {
            packed = values.next().ok_or_else(bad)?;
        }
        if offset == u64::from(u32::MAX) {
            offset = values.next().ok_or_else(bad)?;
        }
    }
    let name = String::from_utf8_lossy(name).into_owned();
    check_name(&name)?;
    let entry = Entry {
        name,
        flags,
        method,
        packed,
        size,
        offset,
    };
    Ok((entry, next))
}

/// The ZIP64 field among an entry's extra fields.
fn zip64(mut extra: &[u8]) -> Option<&[u8]> {
    while extra.len() >= 4 {
        let id = u16_at(extra, 0)?;
        let len = usize::from(u16_at(extra, 2)?);
        let field = extra.get(4..4 + len)?;
        if id == ZIP64_EXTRA {
            return Some(field);
        }
        extra = &extra[4 + len..];
    }
    None
}

/// Rejects a name that leaves the archive or uses backslashes, as yauzl
/// does with its strict names.
fn check_name(name: &str) -> Result<(), Stop> {
    if name.contains('\\') {
        return Err(rejected("Invalid characters in a part's name"));
    }
    let drive = name.as_bytes().get(1) == Some(&b':')
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    if name.starts_with('/') || drive {
        return Err(rejected("A part has an absolute path"));
    }
    if name.split('/').any(|segment| segment == "..") {
        return Err(rejected("A part has an invalid relative path"));
    }
    Ok(())
}

/// An entry's bytes, inflated when they were packed.
fn contents(data: &[u8], entry: &Entry) -> Result<Vec<u8>, Stop> {
    let bad = || rejected("Invalid local file header");
    let at = usize::try_from(entry.offset).map_err(|_| bad())?;
    if u32_at(data, at) != Some(LOCAL_SIGNATURE) {
        return Err(bad());
    }
    let name_len = usize::from(u16_at(data, at + 26).ok_or_else(bad)?);
    let extra_len = usize::from(u16_at(data, at + 28).ok_or_else(bad)?);
    let start = at + LOCAL_LEN + name_len + extra_len;
    let packed = usize::try_from(entry.packed).map_err(|_| bad())?;
    let packed = data
        .get(start..start.saturating_add(packed))
        .ok_or_else(|| rejected("A part runs past the end of the archive"))?;
    let bytes = match entry.method {
        STORED => {
            if entry.packed != entry.size {
                return Err(rejected("A stored part's sizes don't match"));
            }
            packed.to_vec()
        }
        DEFLATED => {
            let mut bytes = Vec::new();
            DeflateDecoder::new(packed)
                .take(entry.size + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| rejected("A part doesn't inflate"))?;
            bytes
        }
        _ => return Err(rejected("Unsupported compression method")),
    };
    if bytes.len() as u64 != entry.size {
        return Err(rejected("A part's size doesn't match the archive's"));
    }
    Ok(bytes)
}

/// A part's text, read as UTF-16 when its first bytes say so and as UTF-8
/// otherwise, without its byte order mark.
fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xff, 0xfe, ..] | [0x3c, 0, ..] => utf16(bytes, u16::from_le_bytes),
        [0xfe, 0xff, ..] | [0, 0x3c, ..] => utf16(bytes, u16::from_be_bytes),
        _ => {
            let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
            String::from_utf8_lossy(bytes).into_owned()
        }
    }
}

/// UTF-16 text in one byte order, with a stray half or odd byte read as
/// U+FFFD.
fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let mut units = bytes
        .chunks(2)
        .map(|pair| match pair {
            [a, b] => unit([*a, *b]),
            _ => 0xfffd,
        })
        .peekable();
    if units.peek() == Some(&0xfeff) {
        units.next();
    }
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// A rejection that names what's wrong with the archive.
fn rejected(reason: &str) -> Stop {
    Stop::Rejected(reason.to_owned())
}

/// The little-endian `u16` at `at`, if the data reaches it.
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// The little-endian `u32` at `at`, if the data reaches it.
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The little-endian `u64` at `at`, if the data reaches it.
fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

/// Builds archives for tests.
#[cfg(test)]
pub(super) mod build {
    use std::io::Write;

    use flate2::write::DeflateEncoder;
    use flate2::Compression;

    /// An archive being built, one part at a time.
    #[derive(Default)]
    pub struct Archive {
        local: Vec<u8>,
        central: Vec<u8>,
        count: u16,
        claimed: Option<u16>,
    }

    impl Archive {
        /// Adds a part stored as it is.
        pub fn stored(mut self, name: &str, data: &[u8]) -> Self {
            self.add(name, data, false, 0, None);
            self
        }

        /// Adds a part packed with deflate.
        pub fn deflated(mut self, name: &str, data: &[u8]) -> Self {
            self.add(name, data, true, 0, None);
            self
        }

        /// Adds a part with the given flags, and with `size` in place of
        /// its real size when there is one.
        pub fn add(
            &mut self,
            name: &str,
            data: &[u8],
            deflate: bool,
            flags: u16,
            size: Option<u32>,
        ) {
            let packed = if deflate {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(data).expect("deflate");
                encoder.finish().expect("deflate")
            } else {
                data.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = self.local.len() as u32;
            let name_len = name.len() as u16;
            let size = size.unwrap_or(data.len() as u32);

            self.local.extend(0x0403_4b50u32.to_le_bytes());
            self.local.extend(20u16.to_le_bytes());
            self.local.extend(flags.to_le_bytes());
            self.local.extend(method.to_le_bytes());
            self.local.extend([0; 8]);
            self.local.extend((packed.len() as u32).to_le_bytes());
            self.local.extend(size.to_le_bytes());
            self.local.extend(name_len.to_le_bytes());
            self.local.extend(0u16.to_le_bytes());
            self.local.extend(name.as_bytes());
            self.local.extend(&packed);

            self.central.extend(0x0201_4b50u32.to_le_bytes());
            self.central.extend(20u16.to_le_bytes());
            self.central.extend(20u16.to_le_bytes());
            self.central.extend(flags.to_le_bytes());
            self.central.extend(method.to_le_bytes());
            self.central.extend([0; 8]);
            self.central.extend((packed.len() as u32).to_le_bytes());
            self.central.extend(size.to_le_bytes());
            self.central.extend(name_len.to_le_bytes());
            self.central.extend([0; 12]);
            self.central.extend(offset.to_le_bytes());
            self.central.extend(name.as_bytes());
            self.count += 1;
        }

        /// Makes the end record claim this many entries.
        pub fn claim(mut self, count: u16) -> Self {
            self.claimed = Some(count);
            self
        }

        /// The archive's bytes.
        pub fn finish(self) -> Vec<u8> {
            let count = self.claimed.unwrap_or(self.count);
            let mut data = self.local;
            let offset = data.len() as u32;
            data.extend(&self.central);
            data.extend(0x0605_4b50u32.to_le_bytes());
            data.extend([0; 4]);
            data.extend(count.to_le_bytes());
            data.extend(count.to_le_bytes());
            data.extend((self.central.len() as u32).to_le_bytes());
            data.extend(offset.to_le_bytes());
            data.extend(0u16.to_le_bytes());
            data
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build::Archive;
    use super::*;

    #[test]
    fn reads_stored_and_deflated_parts() {
        let data = Archive::default()
            .stored("word/document.xml", b"<document>Stored</document>")
            .deflated(
                "word/styles.xml",
                "<styles>Packed</styles>".repeat(50).as_bytes(),
            )
            .finish();
        let parts = parts(&data).expect("an archive");
        assert_eq!(parts["word/document.xml"], "<document>Stored</document>");
        assert_eq!(
            parts["word/styles.xml"],
            "<styles>Packed</styles>".repeat(50)
        );
    }

    #[test]
    fn leaves_parts_without_text_packed() {
        // A huge picture would fail the size checks, so this shows it's
        // never looked at.
        let mut archive = Archive::default().stored("content.xml", b"<a/>");
        archive.add("media/huge.png", b"x", false, 0, Some(u32::MAX - 1));
        let parts = parts(&archive.finish()).expect("an archive");
        assert_eq!(parts.keys().collect::<Vec<_>>(), ["content.xml"]);
    }

    #[test]
    fn rejects_encrypted_parts() {
        let mut archive = Archive::default();
        archive.add("content.xml", b"<a/>", false, ENCRYPTED, None);
        assert!(
            matches!(parts(&archive.finish()), Err(Stop::Rejected(reason)) if reason == "Encrypted archive")
        );
    }

    #[test]
    fn stops_at_a_part_that_would_grow_too_far() {
        let mut archive = Archive::default();
        archive.add("content.xml", &[b' '; 10], true, 0, Some(4096));
        assert!(matches!(parts(&archive.finish()), Err(Stop::Truncated)));
    }

    #[test]
    fn stops_at_too_many_entries() {
        let data = Archive::default()
            .stored("content.xml", b"<a/>")
            .claim(2049)
            .finish();
        assert!(matches!(parts(&data), Err(Stop::Truncated)));
    }

    #[test]
    fn rejects_names_that_leave_the_archive() {
        for name in [
            "../content.xml",
            "/content.xml",
            "C:/content.xml",
            "a\\content.xml",
        ] {
            let data = Archive::default().stored(name, b"<a/>").finish();
            assert!(matches!(parts(&data), Err(Stop::Rejected(_))), "{name}");
        }
    }

    #[test]
    fn rejects_a_part_whose_size_is_wrong() {
        let mut archive = Archive::default();
        archive.add("content.xml", b"<a>more than claimed</a>", true, 0, Some(4));
        assert!(matches!(parts(&archive.finish()), Err(Stop::Rejected(_))));
    }

    #[test]
    fn rejects_what_isnt_an_archive() {
        assert!(matches!(parts(b"invalid document"), Err(Stop::Rejected(_))));
        assert!(matches!(parts(b""), Err(Stop::Rejected(_))));
    }

    #[test]
    fn reads_utf16_and_drops_byte_order_marks() {
        let utf16le: Vec<u8> = "\u{feff}<a>caf\u{e9}</a>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let utf16be: Vec<u8> = "<a>\u{65e5}\u{672c}</a>"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert_eq!(decode(&utf16le), "<a>caf\u{e9}</a>");
        assert_eq!(decode(&utf16be), "<a>\u{65e5}\u{672c}</a>");
        assert_eq!(decode(b"\xef\xbb\xbf<a/>"), "<a/>");
    }
}
