// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

//! Reads text out of RTF, ported from the `rtf` reader in MIT-licensed
//! code, see THIRD_PARTY_NOTICES. It follows groups, skips destinations
//! that don't hold body text, and decodes `\u`, `\'hh` and raw bytes.
//! Windows-1252 is the only code page it decodes beyond ASCII: under the
//! other single byte Windows code pages that code accepts, bytes from 0x80
//! up read as U+FFFD.

use super::extract::{Out, Stop};

/// How deep groups can nest.
const MAX_DEPTH: usize = 128;

/// How far past a backslash a control word can reach.
const WINDOW: usize = 79;

/// Destinations whose text isn't part of the document's body.
const SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "fldinst",
    "header",
    "footer",
];

/// What windows-1252 maps bytes 0x80 to 0x9F to. Bytes from 0xA0 up are
/// the same as in Latin-1.
const WINDOWS_1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// What a group knows about the text inside it.
#[derive(Clone, Copy)]
struct State {
    /// Whether the group's text is left out.
    skip: bool,
    /// How many fallback characters follow each `\u`.
    uc: u64,
    /// Whether bytes decode as windows-1252, the default.
    windows_1252: bool,
}

/// Collects the RTF's text, pairing up UTF-16 halves from `\u` words.
struct Writer<'a> {
    /// Where the text goes.
    out: &'a mut Out,
    /// A high surrogate waiting for its low half.
    high: Option<u16>,
}

impl Writer<'_> {
    /// Writes text, after any unpaired half.
    fn text(&mut self, text: &str) -> Result<(), Stop> {
        if text.is_empty() {
            return Ok(());
        }
        self.flush()?;
        self.out.text(text)
    }

    /// Writes a character.
    fn char(&mut self, c: char) -> Result<(), Stop> {
        self.text(c.encode_utf8(&mut [0; 4]))
    }

    /// Writes one UTF-16 unit, holding a high half until its low half
    /// comes.
    fn unit(&mut self, unit: u16) -> Result<(), Stop> {
        match unit {
            0xd800..=0xdbff => {
                self.flush()?;
                self.high = Some(unit);
                Ok(())
            }
            0xdc00..=0xdfff => match self.high.take() {
                Some(high) => {
                    let pair = char::decode_utf16([high, unit]).next();
                    let c = pair
                        .and_then(Result::ok)
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    self.char(c)
                }
                None => self.char(char::REPLACEMENT_CHARACTER),
            },
            _ => self.char(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)),
        }
    }

    /// Writes a high half that never got its low half.
    fn flush(&mut self) -> Result<(), Stop> {
        if self.high.take().is_some() {
            self.out.text("\u{fffd}")?;
        }
        Ok(())
    }
}

/// Reads the text of the RTF in `data` into `out`.
pub(super) fn read(data: &[u8], out: &mut Out) -> Result<(), Stop> {
    if !data.starts_with(b"{\\rtf") {
        return Err(rejected("Invalid RTF document"));
    }
    let mut out = Writer { out, high: None };
    let mut state = State {
        skip: false,
        uc: 1,
        windows_1252: true,
    };
    let mut stack = Vec::new();
    let mut fallback = 0u64;
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        if byte == b'{' || byte == b'}' {
            fallback = 0;
        }
        if byte == b'{' {
            stack.push(state);
            if stack.len() > MAX_DEPTH {
                return Err(Stop::Truncated);
            }
        } else if byte == b'}' {
            state = stack.pop().unwrap_or(state);
        } else if byte == b'\\' {
            let rest = &data[(i + 1).min(data.len())..(i + 1 + WINDOW).min(data.len())];
            if let Some(hex) = hex(rest) {
                if fallback > 0 {
                    fallback -= 1;
                } else if !state.skip {
                    out.char(decode(hex, state))?;
                }
                i += 3;
            } else if let Some((control, value, len)) = word(rest) {
                i += len;
                if control == "bin" {
                    let count =
                        safe_count(value).ok_or_else(|| rejected("Invalid RTF binary length"))?;
                    i = i.saturating_add(usize::try_from(count).unwrap_or(usize::MAX));
                }
                if fallback > 0 {
                    fallback -= 1;
                    i += 1;
                    continue;
                }
                if SKIPPED.contains(&control) {
                    state.skip = true;
                }
                if control == "ansicpg" {
                    // Per-byte decoding only suits single byte Windows code pages.
                    if value != 874.0 && !(1250.0..=1258.0).contains(&value) {
                        return Err(rejected("Unsupported RTF code page"));
                    }
                    state.windows_1252 = value == 1252.0;
                } else if control == "uc" {
                    state.uc =
                        safe_count(value).ok_or_else(|| rejected("Invalid RTF fallback count"))?;
                } else if control == "u" && !state.skip {
                    out.unit(utf16_unit(value))?;
                    fallback = state.uc;
                } else if !state.skip {
                    match control {
                        "par" | "line" | "row" => out.text("\n")?,
                        "tab" | "cell" => out.text("\t")?,
                        _ => out.text(symbol(control).unwrap_or(""))?,
                    }
                }
            } else {
                i += 1;
                let next = rest.first().copied();
                if fallback > 0 {
                    fallback -= 1;
                } else if next == Some(b'*') {
                    state.skip = true;
                } else if !state.skip {
                    match next {
                        Some(c @ (b'\\' | b'{' | b'}')) => out.char(char::from(c))?,
                        Some(c) => out.text(symbol(&char::from(c).to_string()).unwrap_or(""))?,
                        None => {}
                    }
                }
            }
        } else if !state.skip && byte != b'\n' && byte != b'\r' {
            if fallback > 0 {
                fallback -= 1;
            } else {
                out.char(decode(byte, state))?;
            }
        }
        i += 1;
    }
    out.flush()
}

/// The byte a `'hh` escape after a backslash stands for.
fn hex(rest: &[u8]) -> Option<u8> {
    match rest {
        [b'\'', high, low, ..] if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
            let digits = std::str::from_utf8(&rest[1..3]).ok()?;
            u8::from_str_radix(digits, 16).ok()
        }
        _ => None,
    }
}

/// The control word after a backslash: its letters, its number (NaN when
/// it has none) and how many bytes it takes, with the space that may end
/// it.
fn word(rest: &[u8]) -> Option<(&str, f64, usize)> {
    let letters = rest.iter().take_while(|b| b.is_ascii_alphabetic()).count();
    if letters == 0 {
        return None;
    }
    let mut len = letters;
    let sign = usize::from(rest.get(len) == Some(&b'-'));
    let digits = rest[len + sign..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let mut value = f64::NAN;
    if digits > 0 {
        let number = std::str::from_utf8(&rest[len..len + sign + digits]).ok()?;
        value = number.parse().unwrap_or(f64::NAN);
        len += sign + digits;
    }
    if rest.get(len) == Some(&b' ') {
        len += 1;
    }
    let control = std::str::from_utf8(&rest[..letters]).ok()?;
    Some((control, value, len))
}

/// A word's number as a count, when it's a whole number JavaScript holds
/// exactly and isn't negative.
fn safe_count(value: f64) -> Option<u64> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    (value.fract() == 0.0 && (0.0..=MAX_SAFE).contains(&value)).then_some(value as u64)
}

/// The UTF-16 unit a `\u` word's number stands for. Negative numbers count
/// back from 65536, and anything else wraps the way JavaScript's
/// `String.fromCharCode` does.
fn utf16_unit(value: f64) -> u16 {
    let value = if value < 0.0 { value + 65536.0 } else { value };
    if !value.is_finite() {
        return 0;
    }
    value.trunc().rem_euclid(65536.0) as u16
}

/// The text a symbol word such as `\endash` or `\~` stands for.
fn symbol(control: &str) -> Option<&'static str> {
    Some(match control {
        "endash" => "\u{2013}",
        "emdash" => "\u{2014}",
        "enspace" => "\u{2002}",
        "emspace" => "\u{2003}",
        "qmspace" => "\u{2005}",
        "lquote" => "\u{2018}",
        "rquote" => "\u{2019}",
        "ldblquote" => "\u{201c}",
        "rdblquote" => "\u{201d}",
        "bullet" => "\u{2022}",
        "~" => "\u{a0}",
        "_" => "\u{2011}",
        "-" => "",
        _ => return None,
    })
}

/// The character a byte stands for under the group's code page.
fn decode(byte: u8, state: State) -> char {
    match byte {
        0..=0x7f => char::from(byte),
        _ if !state.windows_1252 => char::REPLACEMENT_CHARACTER,
        0x80..=0x9f => WINDOWS_1252[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// A rejection that names what's wrong with the RTF.
fn rejected(reason: &str) -> Stop {
    Stop::Rejected(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of an RTF document, and whether it was cut short.
    fn read_text(source: &[u8]) -> Result<(String, bool), String> {
        let mut out = Out::new();
        match read(source, &mut out) {
            Ok(()) => Ok((out.into_raw(), false)),
            Err(Stop::Truncated) => Ok((out.into_raw(), true)),
            Err(Stop::Rejected(reason)) => Err(reason),
            Err(Stop::Io(error)) => Err(error.to_string()),
        }
    }

    /// The text of an RTF document that reads in full.
    fn text(source: &str) -> String {
        let (text, truncated) = read_text(source.as_bytes()).expect("RTF");
        assert!(!truncated, "{source}");
        text
    }

    #[test]
    fn reads_symbols() {
        let source = r"{\rtf1\ansi Pages 1\endash 5\emdash\lquote left\rquote\ldblquote right\rdblquote\bullet\b End\b0\unknown{\*\secret\bullet hidden}}";
        assert_eq!(
            text(source),
            "Pages 1\u{2013}5\u{2014}\u{2018}left\u{2019}\u{201c}right\u{201d}\u{2022}End"
        );
        assert_eq!(
            text(r"{\rtf1\ansi A\enspace B\emspace C\qmspace D\~E\_F\-G}"),
            "A\u{2002}B\u{2003}C\u{2005}D\u{a0}E\u{2011}FG"
        );
    }

    #[test]
    fn skips_unicode_fallbacks() {
        let cases = [
            r"{\rtf1\ansi\uc10\u945\endash\emdash\enspace\emspace\qmspace\lquote\rquote\ldblquote\rdblquote\bullet Z}",
            r"{\rtf1\ansi\uc3\u945\~\_\-Z}",
            r"{\rtf1\ansi{\u945}Z}",
            r"{\rtf1\ansi\u945{Z}}",
            r"{\rtf1\ansi\u945\'3fZ}",
            r"{\rtf1\ansi\u945\{Z}",
            r"{\rtf1\ansi\u945\*Z}",
            r"{\rtf1\ansi\uc4\u945?\tab\'3f\~Z}",
            r"{\rtf1\ansi\u945\bin3 abcZ}",
            r"{\rtf1\ansi\uc0\u945Z}",
        ];
        for source in cases {
            assert_eq!(text(source), "\u{3b1}Z", "{source}");
        }
        let large = format!(r"{{\rtf1\ansi\uc20\u945{}Z}}", "?".repeat(20));
        assert_eq!(text(&large), "\u{3b1}Z");
        assert_eq!(text(r"{\rtf1\ansi{\uc2\u945??}\u946?Z}"), "\u{3b1}\u{3b2}Z");
        assert_eq!(text(r"{\rtf1\ansi A\u9\tab X}"), "A\tX");
    }

    #[test]
    fn pairs_utf16_halves_and_wraps_negative_numbers() {
        assert_eq!(text(r"{\rtf1\ansi\u-10179?\u-8704?}"), "\u{1f600}");
        assert_eq!(text(r"{\rtf1\ansi\u-10179?A}"), "\u{fffd}A");
        assert_eq!(text(r"{\rtf1\ansi\u-4064?}"), "\u{f020}");
    }

    #[test]
    fn skips_destinations_and_decodes_windows_1252() {
        let source =
            r"{\rtf1\ansi{\fonttbl hidden}Hello\tab world\par Unicode \u945?{\*\secret hidden}}";
        assert_eq!(text(source), "Hello\tworld\nUnicode \u{3b1}");
        let bytes = b"{\\rtf1\\ansi\\ansicpg1252 caf\\'e9 \x93q\x94 \\'80}";
        assert_eq!(
            read_text(bytes).expect("RTF").0,
            "caf\u{e9} \u{201c}q\u{201d} \u{20ac}"
        );
    }

    #[test]
    fn reads_other_code_pages_as_replacement_characters() {
        let bytes = b"{\\rtf1\\ansi\\ansicpg1251 A\\'c0\xc1}";
        assert_eq!(read_text(bytes).expect("RTF").0, "A\u{fffd}\u{fffd}");
        let error = read_text(br"{\rtf1\ansi\ansicpg932 A}").expect_err("a rejection");
        assert_eq!(error, "Unsupported RTF code page");
    }

    #[test]
    fn stops_at_deep_groups() {
        let source = format!(r"{{\rtf1 Kept{}Lost{}", "{".repeat(128), "}".repeat(129));
        assert_eq!(
            read_text(source.as_bytes()).expect("RTF"),
            ("Kept".to_owned(), true)
        );
    }

    #[test]
    fn rejects_what_isnt_rtf() {
        assert_eq!(
            read_text(b"plain text").expect_err("a rejection"),
            "Invalid RTF document"
        );
        assert!(read_text(br"{\rtf1\uc-1 A}").is_err());
        assert!(read_text(br"{\rtf1\bin A}").is_err());
    }
}
