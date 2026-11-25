// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Text for PowerShell: a literal that holds any text as it is, and a
//! script as `-EncodedCommand` takes it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// `text` as a PowerShell string that holds it as it is: in single quotes,
/// with each single quote doubled, the curly ones too, since PowerShell
/// reads those as quotes as well. `$` and backticks mean nothing there.
pub fn literal(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

/// `script` as `-EncodedCommand` takes it: base64 of its UTF-16LE. That's
/// one plain word, so it reaches PowerShell the same through any shell or
/// command line that starts it.
pub fn encoded(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_doubles_every_single_quote_powershell_reads() {
        assert_eq!(literal("plain"), "'plain'");
        assert_eq!(literal(r"C:\it's"), r"'C:\it''s'");
        assert_eq!(
            literal("\u{2018}a\u{2019} \u{201a}b\u{201b}"),
            "'\u{2018}\u{2018}a\u{2019}\u{2019} \u{201a}\u{201a}b\u{201b}\u{201b}'"
        );
        assert_eq!(literal("$env:X `n \"x\""), "'$env:X `n \"x\"'");
        assert_eq!(literal(""), "''");
    }

    #[test]
    fn a_script_is_encoded_as_base64_of_its_utf16() {
        // What [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...))
        // gives, and `dir` as PowerShell's own help shows it.
        assert_eq!(encoded("dir"), "ZABpAHIA");
        assert_eq!(encoded("Write-Host é"), "VwByAGkAdABlAC0ASABvAHMAdAAgAOkA");
        assert_eq!(encoded(""), "");
    }
}
