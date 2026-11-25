// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Text for XML and the markup built on it, such as a toast, a desktop
//! notification's body, a logon task or a small HTML page: what reads as
//! markup written as entities, and read back.

/// `text` for an XML text node or a double-quoted attribute: `&`, `<`, `>`
/// and `"` as entities, and the characters XML can't hold at all left
/// out, so outside text can't break what it goes in. A single quote stays
/// as it is: nothing puts text in single-quoted attributes, and GNOME's
/// notification markup shows a numeric entity such as `&#39;` as written.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\t' | '\n' | '\r' => escaped.push(c),
            c if c < ' ' || matches!(c, '\u{fffe}' | '\u{ffff}') => {}
            c => escaped.push(c),
        }
    }
    escaped
}

/// The entities [`escape`] writes read back, and `&apos;`, which other
/// writers such as Task Scheduler use.
pub fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_reads_as_markup_is_written_as_entities() {
        assert_eq!(
            escape("<b>\"Tom & Jerry's\"</b>"),
            "&lt;b&gt;&quot;Tom &amp; Jerry's&quot;&lt;/b&gt;"
        );
        assert_eq!(escape("&amp;"), "&amp;amp;");
    }

    #[test]
    fn what_xml_cant_hold_is_left_out() {
        assert_eq!(escape("a\u{0}b\u{1b}[0m\tc\nd\re"), "ab[0m\tc\nd\re");
        assert_eq!(escape("\u{fffe}é😀\u{ffff}"), "é😀");
    }

    #[test]
    fn escaped_text_reads_back_as_it_was() {
        let text = r#"C:\Program Files\<x> & "y" 'z'"#;
        assert_eq!(unescape(&escape(text)), text);
        assert_eq!(unescape("it&apos;s &amp;lt;"), "it's &lt;");
    }
}
