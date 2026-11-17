// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Masks secrets in a command's output before it goes to an agent. The
//! output pill's entry passes its text through [`mask_text`] when the
//! message is encoded (see [`crate::context`]), so the agent and the
//! transcript only ever hold the masked text. The phone and the desktop
//! call the same function, and `maxd`'s diagnostics share the token starts.
//!
//! [`mask_text`] reads the whole text, not word by word, and puts [`MASK`]
//! in place of:
//!
//! - a token known by its start and shape: GitHub's (`ghp_`, `gho_`,
//!   `ghu_`, `ghs_`, `ghr_`, `github_pat_`), OpenAI's and Anthropic's
//!   (`sk-`, `sk-proj-`, `sk-ant-`), Slack's (`xoxb-` and the like),
//!   GitLab's (`glpat-`), npm's (`npm_`), an AWS access key ID (`AKIA` or
//!   `ASIA` and 16 more) and a JWT (two `eyJ` parts and a signature);
//! - the value after `Authorization:` with its scheme kept, a
//!   `Cookie:` or `Set-Cookie:` header's value, and a long value after a
//!   bare `Bearer`;
//! - a password in a URL, as in `postgres://app:[hidden]@db`;
//! - each line inside a private key block, so the text keeps its line
//!   count, or the whole body when the block sits on one line, as in JSON;
//! - the value of an assignment whose name ends in a secret's word:
//!   `password`, `passwd`, `passphrase`, `pass`, `token`, `secret`, `pwd`
//!   after another word, or `key` after `api`, `access`, `private`,
//!   `secret`, `signing`, `encryption` or `master`, with `=` or `: ` and
//!   any quotes. So `GITHUB_TOKEN=`, `"apiKey": "`, `client_secret: ` and
//!   `?token=` lose their value, and `token_type` and `input_tokens` keep
//!   theirs.
//!
//! Code should read as it was, so an assignment loses only a value that
//! looks like a literal secret. A quoted value goes unless it's empty, has
//! a space, starts with `$` or `<`, or is a word such as `string` or
//! `true`. A bare value goes when the `=` touches both the name and the
//! value, as in a shell or a URL, unless it's a call, a reference, a
//! number or such a word. Anywhere else, as in `token = x`, `token: x` or
//! a call's `f(token=x)`, a bare value goes only when it's at least six
//! characters with a letter and a digit, so `let token = read_token();`,
//! `token: String` and `self.token = token` stay. Hashes, UUIDs and base64
//! have no start a token has, so they stay too.

use std::borrow::Cow;

/// What stands in for a secret taken out.
pub const MASK: &str = "[hidden]";

/// How GitHub's tokens start.
pub const GITHUB_TOKEN_STARTS: [&str; 6] = ["gho_", "ghp_", "ghu_", "ghs_", "ghr_", "github_pat_"];

/// How other services' tokens start: OpenAI's and Anthropic's, Slack's,
/// GitLab's, AWS access key IDs, JWTs and npm's.
pub const OTHER_TOKEN_STARTS: [&str; 6] = ["sk-", "xox", "glpat-", "AKIA", "eyJ", "npm_"];

/// Whether `text` starts the way a token does. A rough test for a word
/// already known to be a value, as diagnostics use it; [`mask_text`] also
/// checks a token's shape.
pub fn starts_like_token(text: &str) -> bool {
    GITHUB_TOKEN_STARTS
        .iter()
        .chain(&OTHER_TOKEN_STARTS)
        .any(|start| text.starts_with(start))
}

/// `text` with the secrets it holds masked, such as tokens and keys a
/// command printed. It gives `text` back as it is when it finds none.
pub fn mask_text(text: &str) -> Cow<'_, str> {
    let passes: [fn(&str) -> Option<String>; 5] =
        [private_keys, url_passwords, headers, tokens, assignments];
    passes
        .iter()
        .fold(Cow::Borrowed(text), |text, pass| match pass(&text) {
            Some(masked) => Cow::Owned(masked),
            None => text,
        })
}

/// The replacements one pass makes, from the left.
struct Edits<'a> {
    text: &'a str,
    out: String,
    copied: usize,
    changed: bool,
}

impl<'a> Edits<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            out: String::new(),
            copied: 0,
            changed: false,
        }
    }

    /// Puts `with` in place of `start..end`, which starts at or after the
    /// end of the last one.
    fn replace(&mut self, start: usize, end: usize, with: &str) {
        if &self.text[start..end] == with {
            return;
        }
        self.out.push_str(&self.text[self.copied..start]);
        self.out.push_str(with);
        self.copied = end;
        self.changed = true;
    }

    /// The text with every replacement, or none when nothing changed.
    fn done(mut self) -> Option<String> {
        if !self.changed {
            return None;
        }
        self.out.push_str(&self.text[self.copied..]);
        Some(self.out)
    }
}

/// Whether a word goes on with `byte`.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether a word can start at `at`: nothing a word goes on with comes
/// before it, or a terminal's style escape such as `\x1b[1m` does.
fn starts_word(bytes: &[u8], at: usize) -> bool {
    at == 0 || !is_word_byte(bytes[at - 1]) || after_escape(&bytes[..at])
}

/// Whether `before` ends with a terminal escape such as `\x1b[0;32m`.
fn after_escape(before: &[u8]) -> bool {
    let Some((last, rest)) = before.split_last() else {
        return false;
    };
    if !last.is_ascii_alphabetic() {
        return false;
    }
    let params = rest
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit() || matches!(byte, b';' | b'?'))
        .count();
    rest[..rest.len() - params].ends_with(b"\x1b[")
}

/// Where the run of bytes `ok` takes from `from` ends.
fn run(bytes: &[u8], from: usize, ok: impl Fn(u8) -> bool) -> usize {
    from + bytes[from..].iter().take_while(|byte| ok(**byte)).count()
}

/// Where the spaces and tabs from `from` end.
fn skip_blanks(bytes: &[u8], from: usize) -> usize {
    run(bytes, from, |byte| matches!(byte, b' ' | b'\t'))
}

/// Where the line holding `at` ends, before its newline.
fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |found| at + found)
}

/// Where the line holding `at` starts.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |found| found + 1)
}

/// Whether `bytes` start with `start` in any case.
fn starts_with_any_case(bytes: &[u8], start: &[u8]) -> bool {
    bytes.len() >= start.len() && bytes[..start.len()].eq_ignore_ascii_case(start)
}

// Private key blocks.

/// Masks what's inside each private key block. A block on many lines keeps
/// its lines, each one masked, and anything before the BEGIN mark that
/// starts each line, such as a diff's `+`. One with no END mark, as in
/// output cut short, masks the lines after it that look like a key's.
fn private_keys(text: &str) -> Option<String> {
    let mut edits = Edits::new(text);
    let mut from = 0;
    while let Some(found) = text[from..].find("-----BEGIN ") {
        let begin = from + found;
        let label_at = begin + "-----BEGIN ".len();
        let Some(close) = text[label_at..line_end(text, label_at)].find("-----") else {
            from = label_at;
            continue;
        };
        let label = &text[label_at..label_at + close];
        let body = label_at + close + "-----".len();
        from = body;
        if !label.contains("PRIVATE KEY") {
            continue;
        }
        let prefix = &text[line_start(text, begin)..begin];
        let prefix = if prefix.bytes().all(|byte| b"+- \t>".contains(&byte)) {
            prefix
        } else {
            ""
        };
        let end_mark = format!("-----END {label}-----");
        match text[body..].find(&end_mark).map(|at| body + at) {
            Some(end) if !text[body..end].contains('\n') => {
                edits.replace(body, end, MASK);
                from = end + end_mark.len();
            }
            Some(end) => {
                mask_lines(&mut edits, text, body, end, prefix);
                from = end + end_mark.len();
            }
            None => {
                let mut last = line_end(text, body);
                let mut header = false;
                while last < text.len() {
                    let next = last + 1;
                    let line = &text[next..line_end(text, next)];
                    let line = line.strip_prefix(prefix).unwrap_or(line).trim_end();
                    let is_header = line.starts_with("Proc-Type:") || line.starts_with("DEK-Info:");
                    let base64 = !line.is_empty()
                        && line
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte));
                    if !(is_header || base64 || (line.is_empty() && header)) {
                        break;
                    }
                    header = is_header;
                    last = line_end(text, next);
                }
                mask_lines(&mut edits, text, body, last, prefix);
                from = last.max(body);
            }
        }
    }
    edits.done()
}

/// Masks each line in `from..to` that holds more than `prefix` and spaces,
/// keeping `prefix` and the line's end.
fn mask_lines(edits: &mut Edits<'_>, text: &str, from: usize, to: usize, prefix: &str) {
    let mut start = from;
    while start <= to {
        let end = text[start..to].find('\n').map_or(to, |found| start + found);
        let line = text[start..end].trim_end_matches('\r');
        let keep = if line.starts_with(prefix) {
            prefix.len()
        } else {
            0
        };
        if !line[keep..].trim().is_empty() {
            edits.replace(start + keep, start + line.len(), MASK);
        }
        start = end + 1;
    }
}

// Passwords in URLs.

/// Masks the password in each URL that has one before its host.
fn url_passwords(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut edits = Edits::new(text);
    let mut from = 0;
    while let Some(found) = text[from..].find("://") {
        let host = from + found + "://".len();
        from = host;
        // A scheme's letter comes before `://`.
        if host < 4 || !bytes[host - 4].is_ascii_alphabetic() {
            continue;
        }
        let end = run(bytes, host, |byte| {
            !(byte.is_ascii_whitespace() || b"/?#\"'`<>".contains(&byte))
        });
        from = end;
        let Some(at) = bytes[host..end].iter().rposition(|byte| *byte == b'@') else {
            continue;
        };
        if let Some(colon) = bytes[host..host + at].iter().position(|byte| *byte == b':')
            && colon + 1 < at
        {
            edits.replace(host + colon + 1, host + at, MASK);
        }
    }
    edits.done()
}

// Headers.

/// Masks the value of each `Authorization` header, keeping its scheme, and
/// each `Cookie` or `Set-Cookie` header's.
fn headers(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut edits = Edits::new(text);
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_alphabetic()
            && starts_word(bytes, at)
            && let Some((start, end)) = header_value(text, at)
        {
            edits.replace(start, end, MASK);
            at = end;
            continue;
        }
        at += 1;
    }
    edits.done()
}

/// The span of the header value whose name starts at `at`, if one does.
fn header_value(text: &str, at: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let (name, cookie) = if starts_with_any_case(&bytes[at..], b"authorization") {
        ("authorization", false)
    } else if starts_with_any_case(&bytes[at..], b"cookie") {
        ("cookie", true)
    } else {
        return None;
    };
    let mut i = at + name.len();
    // A longer name, such as `authorization_url` or `cookies`.
    if bytes
        .get(i)
        .is_some_and(|byte| is_word_byte(*byte) || *byte == b'-')
    {
        return None;
    }
    if matches!(bytes.get(i), Some(b'"' | b'\'')) {
        i += 1;
    }
    i = skip_blanks(bytes, i);
    match bytes.get(i) {
        Some(b':') => {}
        Some(b'=') if !cookie => {}
        _ => return None,
    }
    i = skip_blanks(bytes, i + 1);
    let quote = match bytes.get(i) {
        Some(&quote @ (b'"' | b'\'')) => {
            i += 1;
            Some(quote)
        }
        _ => None,
    };
    if cookie {
        let line = line_end(text, i);
        let end = quote
            .and_then(|quote| bytes[i..line].iter().position(|byte| *byte == quote))
            .map_or(line, |found| i + found);
        let end = i + text[i..end].trim_end().len();
        return (end > i).then_some((i, end));
    }
    let scheme = run(bytes, i, |byte| byte.is_ascii_alphabetic());
    if scheme > i && bytes.get(scheme) == Some(&b' ') && is_scheme(&bytes[i..scheme]) {
        i = skip_blanks(bytes, scheme);
    }
    let end = run(bytes, i, |byte| {
        !(byte.is_ascii_whitespace() || b"\"'`,".contains(&byte))
    });
    (end > i).then_some((i, end))
}

/// Whether `word` is what comes between `Authorization:` and the
/// credential.
fn is_scheme(word: &[u8]) -> bool {
    [
        b"bearer".as_slice(),
        b"basic",
        b"token",
        b"digest",
        b"bot",
        b"negotiate",
    ]
    .iter()
    .any(|scheme| word.eq_ignore_ascii_case(scheme))
}

// Tokens by their shape.

/// How a kind of token looks: how it starts, what follows the start, and
/// how many of those at least and at most.
struct Shape {
    start: &'static str,
    body: fn(u8) -> bool,
    least: usize,
    most: usize,
    /// Whether what follows must hold a letter and a digit, so a long
    /// word with dashes isn't taken for one.
    mixed: bool,
}

fn alphanumeric(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn dashed(byte: u8) -> bool {
    is_word_byte(byte) || byte == b'-'
}

fn upper(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

const fn shape(start: &'static str, body: fn(u8) -> bool, least: usize, mixed: bool) -> Shape {
    Shape {
        start,
        body,
        least,
        most: usize::MAX,
        mixed,
    }
}

/// The tokens [`tokens`] knows, longer starts before shorter ones.
const SHAPES: [Shape; 16] = [
    shape("github_pat_", is_word_byte, 20, false),
    shape("ghp_", alphanumeric, 20, false),
    shape("gho_", alphanumeric, 20, false),
    shape("ghu_", alphanumeric, 20, false),
    shape("ghs_", alphanumeric, 20, false),
    shape("ghr_", alphanumeric, 20, false),
    shape("sk-", dashed, 20, true),
    shape("xoxa-", dashed, 10, true),
    shape("xoxb-", dashed, 10, true),
    shape("xoxp-", dashed, 10, true),
    shape("xoxr-", dashed, 10, true),
    shape("xoxs-", dashed, 10, true),
    shape("glpat-", dashed, 20, false),
    shape("npm_", alphanumeric, 30, false),
    Shape {
        most: 16,
        ..shape("AKIA", upper, 16, false)
    },
    Shape {
        most: 16,
        ..shape("ASIA", upper, 16, false)
    },
];

/// Masks each token known by its shape, and a long value after a bare
/// `Bearer`.
fn tokens(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut edits = Edits::new(text);
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_alphabetic()
            && starts_word(bytes, at)
            && let Some((start, end)) = token_at(bytes, at)
        {
            edits.replace(start, end, MASK);
            at = end;
            continue;
        }
        at += 1;
    }
    edits.done()
}

/// The span to mask for a token that starts at `at`, if one does.
fn token_at(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let rest = &bytes[at..];
    for shape in &SHAPES {
        if !rest.starts_with(shape.start.as_bytes()) {
            continue;
        }
        let from = at + shape.start.len();
        let end = run(bytes, from, shape.body);
        let body = &bytes[from..end];
        let fits = (shape.least..=shape.most).contains(&body.len())
            && !bytes.get(end).is_some_and(|byte| is_word_byte(*byte))
            && (!shape.mixed || mixed(body));
        return fits.then_some((at, end));
    }
    if let Some(end) = jwt_end(bytes, at) {
        return Some((at, end));
    }
    for bearer in [b"Bearer ", b"bearer "] {
        if rest.starts_with(bearer) {
            let from = at + bearer.len();
            let end = run(bytes, from, |byte| {
                byte.is_ascii_alphanumeric() || b"._~+/=-".contains(&byte)
            });
            let body = &bytes[from..end];
            return (body.len() >= 16 && body.iter().any(u8::is_ascii_digit))
                .then_some((from, end));
        }
    }
    None
}

/// Whether `body` holds a letter and a digit.
fn mixed(body: &[u8]) -> bool {
    body.iter().any(u8::is_ascii_digit) && body.iter().any(u8::is_ascii_alphabetic)
}

/// Where a JWT that starts at `at` ends: a header and a payload, both JSON
/// so both start `eyJ`, and a signature, each in base64url with a dot
/// between.
fn jwt_end(bytes: &[u8], at: usize) -> Option<usize> {
    let part = |from: usize| run(bytes, from, dashed);
    let header = part(at);
    if !bytes[at..].starts_with(b"eyJ") || header - at < 10 || bytes.get(header) != Some(&b'.') {
        return None;
    }
    let payload = part(header + 1);
    if !bytes[header + 1..].starts_with(b"eyJ")
        || payload - header < 11
        || bytes.get(payload) != Some(&b'.')
    {
        return None;
    }
    Some(part(payload + 1))
}

// Assignments.

/// Masks the value of each assignment whose name ends in a secret's word.
fn assignments(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut edits = Edits::new(text);
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'=' | b':')
            && let Some((start, end)) = assigned_secret(bytes, at)
        {
            edits.replace(start, end, MASK);
            at = end;
            continue;
        }
        at += 1;
    }
    edits.done()
}

/// The span of the secret assigned with the `=` or `:` at `mark`, if its
/// name ends in a secret's word and the value looks like a literal.
fn assigned_secret(bytes: &[u8], mark: usize) -> Option<(usize, usize)> {
    let before = mark.checked_sub(1).map(|at| bytes[at]);
    let after = bytes.get(mark + 1).copied();
    if bytes[mark] == b'=' {
        // `==`, `=>`, `!=`, `<=`, `+=`, `:=` and the like.
        if after.is_some_and(|byte| b"=>".contains(&byte))
            || before.is_some_and(|byte| b"=!<>+-*/|&^%:".contains(&byte))
        {
            return None;
        }
    } else if !after.is_some_and(|byte| b" \t\"'".contains(&byte)) || before == Some(b':') {
        // `a::b`, a host and its port, a time.
        return None;
    }

    let mut name_end = skip_blanks_back(bytes, mark);
    let spaced = name_end < mark || bytes[mark] == b':';
    let quote = name_end
        .checked_sub(1)
        .map(|at| bytes[at])
        .filter(|byte| matches!(byte, b'"' | b'\''));
    if quote.is_some() {
        name_end -= 1;
    }
    let name_start = name_end
        - bytes[..name_end]
            .iter()
            .rev()
            .take_while(|byte| is_word_byte(**byte) || matches!(byte, b'-' | b'.'))
            .count();
    let name = &bytes[name_start..name_end];
    if name.is_empty()
        || quote.is_some_and(|quote| name_start == 0 || bytes[name_start - 1] != quote)
        || !names_secret(name)
    {
        return None;
    }
    // `f(token=x)` and `config.token=x` are code.
    let in_code = name.contains(&b'.')
        || skip_blanks_back(bytes, name_start)
            .checked_sub(1)
            .is_some_and(|at| matches!(bytes[at], b'(' | b','));

    let start = skip_blanks(bytes, mark + 1);
    let spaced = spaced || start > mark + 1;
    match bytes.get(start) {
        Some(&quote @ (b'"' | b'\'' | b'`')) => {
            let open = start + 1;
            let close = open
                + bytes[open..]
                    .iter()
                    .position(|byte| *byte == quote || *byte == b'\n')?;
            (bytes[close] == quote && quoted_literal(&bytes[open..close])).then_some((open, close))
        }
        _ => {
            let end = run(bytes, start, |byte| {
                !(byte.is_ascii_whitespace() || b"\"'`&;,)]}<>|".contains(&byte))
            });
            bare_literal(&bytes[start..end], spaced || in_code).then_some((start, end))
        }
    }
}

/// Where the spaces and tabs before `to` start.
fn skip_blanks_back(bytes: &[u8], to: usize) -> usize {
    to - bytes[..to]
        .iter()
        .rev()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

/// Whether a name's last word, or its last two, name a secret.
fn names_secret(name: &[u8]) -> bool {
    let words = words(name);
    let Some((last, rest)) = words.split_last() else {
        return false;
    };
    let before = rest.last().map(String::as_str);
    match last.as_str() {
        "password" | "passwd" | "passphrase" | "pass" | "token" | "secret" | "apikey"
        | "accesskey" | "privatekey" | "secretkey" | "sshpass" => true,
        // `PWD` and `OLDPWD` are a folder, `MYSQL_PWD` a password.
        "pwd" => before.is_some(),
        "key" => matches!(
            before,
            Some("api" | "access" | "private" | "secret" | "signing" | "encryption" | "master")
        ),
        _ => false,
    }
}

/// The words in a name, in lower case: split at anything not a letter or
/// a digit, and where a lower case letter meets an upper case one.
fn words(name: &[u8]) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut lower = false;
    for &byte in name {
        if (!byte.is_ascii_alphanumeric() || (lower && byte.is_ascii_uppercase()))
            && !word.is_empty()
        {
            words.push(std::mem::take(&mut word));
        }
        if byte.is_ascii_alphanumeric() {
            word.push(byte.to_ascii_lowercase() as char);
        }
        lower = byte.is_ascii_lowercase() || byte.is_ascii_digit();
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Words that stand for a type, a setting or a name, not a secret.
const NOT_SECRETS: [&str; 26] = [
    "null",
    "nil",
    "none",
    "true",
    "false",
    "undefined",
    "yes",
    "no",
    "on",
    "off",
    "required",
    "optional",
    "string",
    "str",
    "bool",
    "boolean",
    "number",
    "int",
    "integer",
    "any",
    "unknown",
    "object",
    "password",
    "passphrase",
    "token",
    "secret",
];

/// Whether `value` is a word that's no secret, a number or one character
/// over and over, as in `****`.
fn plainly_not_secret(value: &[u8]) -> bool {
    NOT_SECRETS
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word.as_bytes()))
        || value
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b'.')
        || value.iter().all(|byte| *byte == value[0])
        || value == MASK.as_bytes()
}

/// Whether a quoted value looks like a literal secret.
fn quoted_literal(value: &[u8]) -> bool {
    !value.is_empty()
        && !value.iter().any(u8::is_ascii_whitespace)
        && !value.starts_with(b"$")
        && !value.starts_with(b"<")
        && !value.windows(2).any(|pair| pair == b"{{" || pair == b"${")
        && !plainly_not_secret(value)
}

/// Whether a bare value looks like a literal secret. A `strict` one, after
/// a space or in code, also needs six characters with a letter and a
/// digit.
fn bare_literal(value: &[u8], strict: bool) -> bool {
    let Some(first) = value.first() else {
        return false;
    };
    if b"$[{(<*%\\#".contains(first)
        || value.contains(&b'(')
        || value.windows(2).any(|pair| pair == b"::" || pair == b"->")
        || !value.iter().any(u8::is_ascii_alphanumeric)
        || plainly_not_secret(value)
    {
        return false;
    }
    !strict || (value.len() >= 6 && mixed(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake token of `len` characters after `start`, with letters and
    /// digits, built here so no whole token sits in the source.
    fn fake(start: &str, len: usize) -> String {
        let body: String = "Zx7Qm2Lp9Vr4Tn8Ks3Wd6Hb1"
            .chars()
            .cycle()
            .take(len)
            .collect();
        format!("{start}{body}")
    }

    fn jwt() -> String {
        [
            format!("eyJ{}", "hbGciOiJIUzI1NiJ9"),
            format!("eyJ{}", "zdWIiOiIxMjM0NTY3ODkwIn0"),
            fake("", 43),
        ]
        .join(".")
    }

    fn masks(cases: &[(String, String)]) {
        let wrong: Vec<_> = cases
            .iter()
            .filter(|(text, kept)| mask_text(text) != kept.as_str())
            .map(|(text, kept)| (text, kept, mask_text(text)))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn known_tokens_are_hidden_where_they_stand() {
        let tokens = [
            fake("ghp_", 36),
            fake("gho_", 36),
            fake("ghu_", 36),
            fake("ghs_", 36),
            fake("ghr_", 36),
            fake("github_pat_", 22) + "_" + &fake("", 59),
            fake("sk-", 48),
            fake("sk-proj-", 64),
            fake("sk-ant-api03-", 93),
            fake("xoxb-", 12) + "-" + &fake("", 24),
            fake("xoxp-", 40),
            fake("glpat-", 20),
            fake("npm_", 36),
            format!("AKIA{}", "IOSFODNN7EXAMPLE"),
            format!("ASIA{}", "Y34FZKBOKMUTVV7A"),
            jwt(),
        ];
        let mut cases = Vec::new();
        for token in tokens {
            cases.extend([
                (token.clone(), MASK.to_owned()),
                (
                    format!("Using {token} to push"),
                    "Using [hidden] to push".to_owned(),
                ),
                (
                    format!("\x1b[32m{token}\x1b[0m\n"),
                    "\x1b[32m[hidden]\x1b[0m\n".to_owned(),
                ),
                (
                    format!("+    \"value\": \"{token}\","),
                    "+    \"value\": \"[hidden]\",".to_owned(),
                ),
                (format!("--flag={token}"), "--flag=[hidden]".to_owned()),
            ]);
        }
        masks(&cases);
    }

    #[test]
    fn a_secret_assignment_keeps_its_name() {
        let aws = fake("wJalrXUtnFEMI/K7MDENG/", 18);
        masks(&[
            (
                "GITHUB_TOKEN=abc123def".into(),
                "GITHUB_TOKEN=[hidden]".into(),
            ),
            (
                "export DB_PASSWORD='hunter2'".into(),
                "export DB_PASSWORD='[hidden]'".into(),
            ),
            (
                r#"{"password": "hunter2", "user": "me"}"#.into(),
                r#"{"password": "[hidden]", "user": "me"}"#.into(),
            ),
            (
                r#"{"apiKey":"abc123","name":"x"}"#.into(),
                r#"{"apiKey":"[hidden]","name":"x"}"#.into(),
            ),
            ("password: hunter22".into(), "password: [hidden]".into()),
            (
                "  client_secret: s3cr3t-value".into(),
                "  client_secret: [hidden]".into(),
            ),
            (
                format!("aws_secret_access_key = {aws}"),
                "aws_secret_access_key = [hidden]".into(),
            ),
            (
                format!("AWS_SECRET_ACCESS_KEY={aws}"),
                "AWS_SECRET_ACCESS_KEY=[hidden]".into(),
            ),
            (
                "curl https://api.example.com/v1?token=abc123&page=2".into(),
                "curl https://api.example.com/v1?token=[hidden]&page=2".into(),
            ),
            (
                "tool --password=hunter2 --verbose".into(),
                "tool --password=[hidden] --verbose".into(),
            ),
            (
                "MYSQL_PWD=hunter2 mysql".into(),
                "MYSQL_PWD=[hidden] mysql".into(),
            ),
            (
                "x-api-key: 0123abcd4567".into(),
                "x-api-key: [hidden]".into(),
            ),
            (
                "const accessToken = \"abc\";".into(),
                "const accessToken = \"[hidden]\";".into(),
            ),
            (
                "spring.datasource.password=hunter22".into(),
                "spring.datasource.password=[hidden]".into(),
            ),
            (
                "SECRET=changeme\nPORT=3000".into(),
                "SECRET=[hidden]\nPORT=3000".into(),
            ),
        ]);
    }

    #[test]
    fn a_header_keeps_its_scheme() {
        let long = fake("", 40);
        masks(&[
            (
                "Authorization: Bearer abc.def-123".into(),
                "Authorization: Bearer [hidden]".into(),
            ),
            (
                r#"curl -H "Authorization: Basic dXNlcjpwYXNz" https://example.com"#.into(),
                r#"curl -H "Authorization: Basic [hidden]" https://example.com"#.into(),
            ),
            (
                r#"{"authorization": "Bearer abc123"}"#.into(),
                r#"{"authorization": "Bearer [hidden]"}"#.into(),
            ),
            (
                "proxy-authorization:token abc".into(),
                "proxy-authorization:token [hidden]".into(),
            ),
            (
                "authorization: abc".into(),
                "authorization: [hidden]".into(),
            ),
            (
                "Set-Cookie: session=abc; Path=/; HttpOnly\nnext".into(),
                "Set-Cookie: [hidden]\nnext".into(),
            ),
            (
                format!("-H 'Bearer {long}' https://x"),
                "-H 'Bearer [hidden]' https://x".into(),
            ),
        ]);
    }

    #[test]
    fn a_url_loses_its_password_only() {
        let token = fake("ghs_", 36);
        masks(&[
            (
                "postgres://app:hunter2@db.example.com:5432/app".into(),
                "postgres://app:[hidden]@db.example.com:5432/app".into(),
            ),
            (
                format!("remote: https://x-access-token:{token}@github.com/owner/repo.git"),
                "remote: https://x-access-token:[hidden]@github.com/owner/repo.git".into(),
            ),
            (
                "redis://:p%40ss@cache:6379".into(),
                "redis://:[hidden]@cache:6379".into(),
            ),
        ]);
    }

    #[test]
    fn a_private_key_keeps_its_marks_and_its_lines() {
        let body = [fake("MIIEow", 58), fake("", 64), fake("", 20) + "=="];
        let block = |prefix: &str, label: &str| {
            let mut lines = vec![format!("{prefix}-----BEGIN {label}-----")];
            lines.extend(body.iter().map(|line| format!("{prefix}{line}")));
            lines.push(format!("{prefix}-----END {label}-----"));
            lines.join("\n")
        };
        let masked = |prefix: &str, label: &str| {
            let mut lines = vec![format!("{prefix}-----BEGIN {label}-----")];
            lines.extend(body.iter().map(|_| format!("{prefix}{MASK}")));
            lines.push(format!("{prefix}-----END {label}-----"));
            lines.join("\n")
        };
        let cut: Vec<_> = block("", "OPENSSH PRIVATE KEY")
            .lines()
            .take(3)
            .map(str::to_owned)
            .collect();
        masks(&[
            (
                format!("key:\n{}\ndone", block("", "RSA PRIVATE KEY")),
                format!("key:\n{}\ndone", masked("", "RSA PRIVATE KEY")),
            ),
            (block("+", "PRIVATE KEY"), masked("+", "PRIVATE KEY")),
            (
                block("    ", "EC PRIVATE KEY"),
                masked("    ", "EC PRIVATE KEY"),
            ),
            (
                format!(
                    r#""private_key": "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n","#,
                    body[0]
                ),
                r#""private_key": "-----BEGIN PRIVATE KEY-----[hidden]-----END PRIVATE KEY-----\n","#
                    .into(),
            ),
            (
                format!("{}\n$ next", cut.join("\n")),
                format!("{}\n{MASK}\n{MASK}\n$ next", cut[0]),
            ),
        ]);
    }

    #[test]
    fn ordinary_output_stays_as_it_is() {
        let hash = "9fceb02d0ae598e95dc970b74767f19372d61af8";
        let sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        for text in [
            "",
            "FAIL src/cache.test.ts\n1 failed",
            "```\n`x`",
            hash,
            &format!("commit {hash}\nMerge: 1a2b3c4 5d6e7f8"),
            &format!("sha256:{sha256}"),
            "0199a7c2-0000-7000-8000-00000000000a",
            "diff --git a/icon.png b/icon.png\nGIT binary patch\nliteral 68\nzcmeAS@N?(olHy`uVBq!ia0vp^j3CUx1|;Q0k8}blv7Rn&Ar*6y5*",
            "+  \"icon\": \"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\"",
            "  integrity sha512-dyl4Yq0cUBkXHAJZmT6jBvhRCwcXp3Nl0kuNCPqpzPzDwbAtFhb2DVLqcAPwmUZt4P5DQ0CGIpnuF2X+qB0wUg==",
            "task-list disk-usage sk-learn ask-for-help",
            "Enter your password:",
            "Password: ",
            "Password:  \u{1f512} sent",
            "let token = read_token();",
            "token.trim()",
            "if token == expected {",
            "fn check(token: &str) -> bool",
            "pub token: String,",
            "pub token: u64,",
            "password?: string;",
            "self.token = token;",
            "client(token=token, timeout=30)",
            "client(token=self.token)",
            "input_tokens: 1234, output_tokens: 56",
            r#""token_type": "bearer""#,
            r#""password": "Password""#,
            r#""token": """#,
            "PWD=/Users/me/code/example-app\nOLDPWD=/tmp",
            "pass=12 fail=0",
            "git@github.com:owner/repo.git",
            "http://localhost:5173/ and https://example.com:8443/a@b",
            "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUQ\n-----END CERTIFICATE-----",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@devbox",
            "GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
            "token: \"${{ secrets.NPM_TOKEN }}\"",
            "Authorization failed for owner/repo",
            "secret: true\npassword: null",
            "DB_PASSWORD=$DB_PASSWORD",
            "password: [hidden]",
            "Bearer authentication needed",
            "at 12:30:45 the build took 3:20",
            "The key is AKIAIOSFODNN7EXAMPLEXYZ",
        ] {
            assert!(
                matches!(mask_text(text), Cow::Borrowed(_)),
                "{text:?} became {:?}",
                mask_text(text)
            );
        }
    }

    #[test]
    fn masking_twice_changes_nothing_more() {
        let text = format!(
            "GITHUB_TOKEN={}\nAuthorization: Bearer abc123\npostgres://a:b@c/d",
            fake("ghp_", 36)
        );
        let once = mask_text(&text).into_owned();
        assert!(matches!(mask_text(&once), Cow::Borrowed(_)), "{once}");
        assert_eq!(
            once,
            "GITHUB_TOKEN=[hidden]\nAuthorization: Bearer [hidden]\npostgres://a:[hidden]@c/d"
        );
    }

    #[test]
    fn text_past_ascii_keeps_its_characters() {
        let token = fake("ghp_", 36);
        assert_eq!(
            mask_text(&format!("ключ {token} → готово ✓")),
            "ключ [hidden] → готово ✓"
        );
        assert_eq!(mask_text("пароль: ✓✓✓"), "пароль: ✓✓✓");
    }

    #[test]
    fn a_word_is_split_where_its_case_changes() {
        assert_eq!(words(b"accessToken"), ["access", "token"]);
        assert_eq!(words(b"--x-api-key"), ["x", "api", "key"]);
        assert_eq!(
            words(b"AWS_SECRET_ACCESS_KEY"),
            ["aws", "secret", "access", "key"]
        );
        assert_eq!(words(b"APIKey"), ["apikey"]);
        assert!(names_secret(b"client_secret"));
        assert!(!names_secret(b"token_type"));
        assert!(!names_secret(b"input_tokens"));
        assert!(!names_secret(b"PWD"));
        assert!(!names_secret(b"primary_key"));
    }

    #[test]
    fn a_token_start_is_known() {
        assert!(starts_like_token("ghp_x"));
        assert!(starts_like_token("sk-x"));
        assert!(!starts_like_token("main"));
    }
}
