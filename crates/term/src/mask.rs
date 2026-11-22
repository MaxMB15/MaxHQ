// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Hides what a command line sets for a name that reads like a secret, as
//! `•••`: `export GH_TOKEN=…`, `FOO_SECRET=… cmd`, `--password=…`, fish's
//! `set -x API_KEY …`, and the word after `--token` or `--password`. A
//! name reads like a secret when one of its parts, split at `_` and `-`,
//! is one of the words a secret prompt asks with, or ends with one of the
//! longer ones, as `DBPASSWORD` does. It reads the line roughly as a shell
//! does, with quotes, escapes and `$(...)`, and hides too much rather than
//! too little.

use crate::look::SECRET_WORDS;

/// What a hidden value shows as.
pub const MASK: &str = "•••";

/// Words a name may end with and still read like a secret, as in
/// `DBPASSWORD` or `GITHUBTOKEN`. The short ones, such as `pin` or `key`,
/// end too many plain words.
const SUFFIXES: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "passcode",
    "secret",
    "token",
    "apikey",
];

/// `line` with each value set for a name that reads like a secret shown as
/// [`MASK`].
pub fn mask_secrets(line: &str) -> String {
    let words = words(line);
    let mut hidden: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let word = &words[at];
        let text = &line[word.start..word.end];
        if let Some(value) = assignment(text) {
            if value < text.len() {
                hidden.push((word.start + value, word.end));
            }
        } else if let Some(flag) = text.strip_prefix("--").or_else(|| text.strip_prefix('-'))
            && SECRET_WORDS.contains(&flag.to_ascii_lowercase().as_str())
            && !word.ends_command
            && let Some(next) = words.get(at + 1)
            && !line[next.start..next.end].starts_with('-')
        {
            // `--password hunter2`: the next word is the value.
            hidden.push((next.start, next.end));
            at += 1;
        } else if text == "set" && word.first {
            // fish: `set [-flags] NAME values…`.
            let mut name = at + 1;
            while (words.get(name)).is_some_and(|flag| {
                line[flag.start..flag.end].starts_with('-') && !words[name - 1].ends_command
            }) {
                name += 1;
            }
            if let Some(found) = words.get(name)
                && !words[name - 1].ends_command
                && secret_name(&line[found.start..found.end])
                && !found.ends_command
                && let Some(first) = words.get(name + 1)
            {
                let mut last = name + 1;
                while !words[last].ends_command && last + 1 < words.len() {
                    last += 1;
                }
                hidden.push((first.start, words[last].end));
                at = last;
            }
        }
        at += 1;
    }
    if hidden.is_empty() {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut from = 0;
    for (start, end) in hidden {
        out.push_str(&line[from..start]);
        out.push_str(MASK);
        from = end;
    }
    out.push_str(&line[from..]);
    out
}

/// Where the value starts in a `NAME=value` or `--name=value` word whose
/// name reads like a secret.
fn assignment(word: &str) -> Option<usize> {
    let equals = word.find('=')?;
    let name = &word[..equals];
    let bare = name.trim_start_matches('-');
    let plain = bare
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || (c == '-' && name.starts_with('-')));
    (plain && secret_name(bare)).then_some(equals + 1)
}

/// Whether a name reads like a secret.
fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.split(['_', '-']).any(|part| {
        SECRET_WORDS.contains(&part)
            || SUFFIXES
                .iter()
                .any(|suffix| part.len() > suffix.len() && part.ends_with(suffix))
    })
}

/// A word of a command line.
struct Word {
    start: usize,
    end: usize,
    /// Whether it's the first of its command.
    first: bool,
    /// Whether a `;`, `&`, `|`, `(`, `)` or line break ends its command
    /// after it, or the line ends.
    ends_command: bool,
}

/// The words of a command line, with quotes, escapes and `$(...)` inside
/// the word they're in.
fn words(line: &str) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut start: Option<usize> = None;
    let mut first = true;
    let (mut single, mut double, mut escaped, mut tick) = (false, false, false, false);
    let mut depth = 0u32;
    let mut previous = '\0';
    for (at, c) in line.char_indices() {
        let quoted = single || double || tick || depth > 0;
        if escaped {
            escaped = false;
        } else if single {
            single = c != '\'';
        } else if c == '\\' {
            escaped = true;
        } else if c == '`' {
            tick = !tick;
        } else if double && c == '"' && depth == 0 {
            double = false;
        } else if c == '(' && previous == '$' {
            depth += 1;
        } else if c == ')' && depth > 0 {
            depth -= 1;
        } else if quoted {
        } else if c == '\'' {
            single = true;
        } else if c == '"' {
            double = true;
        } else if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>') {
            let ends = matches!(c, '\n' | ';' | '&' | '|' | '(' | ')');
            if let Some(from) = start.take() {
                words.push(Word {
                    start: from,
                    end: at,
                    first,
                    ends_command: false,
                });
                first = false;
            }
            if ends {
                if let Some(last) = words.last_mut() {
                    last.ends_command = true;
                }
                first = true;
            }
            previous = c;
            continue;
        }
        if start.is_none() {
            start = Some(at);
        }
        previous = c;
    }
    if let Some(from) = start {
        words.push(Word {
            start: from,
            end: line.len(),
            first,
            ends_command: true,
        });
    } else if let Some(last) = words.last_mut() {
        last.ends_command = true;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_for_secret_names_hide() {
        for (line, shown) in [
            ("export GH_TOKEN=abc123", "export GH_TOKEN=•••"),
            (
                "FOO_SECRET=s3cr3t ./deploy.sh",
                "FOO_SECRET=••• ./deploy.sh",
            ),
            ("PASSWORD=hunter2", "PASSWORD=•••"),
            ("AWS_ACCESS_KEY=AKIA0000", "AWS_ACCESS_KEY=•••"),
            (
                "export DBPASSWORD='a b c' DEBUG=1",
                "export DBPASSWORD=••• DEBUG=1",
            ),
            (
                "env API_TOKEN=\"x y\" npm start",
                "env API_TOKEN=••• npm start",
            ),
            (
                "GITHUBTOKEN=$(pass show gh) gh pr list",
                "GITHUBTOKEN=••• gh pr list",
            ),
            (
                "mysql --password=hunter2 -u root",
                "mysql --password=••• -u root",
            ),
            ("psql --password hunter2 db", "psql --password ••• db"),
            (
                "login --token abc; echo done",
                "login --token •••; echo done",
            ),
            ("set -gx NPM_TOKEN abc def", "set -gx NPM_TOKEN •••"),
            (
                "set -x API_KEY abc; and echo ok",
                "set -x API_KEY •••; and echo ok",
            ),
            ("a && export OTP=123456 && b", "a && export OTP=••• && b"),
            (
                "export SECRET=one\nexport TOKEN=two",
                "export SECRET=•••\nexport TOKEN=•••",
            ),
        ] {
            assert_eq!(mask_secrets(line), shown, "{line}");
        }
    }

    #[test]
    fn plain_lines_stay_as_they_are() {
        for line in [
            "export DEBUG=cues",
            "ls -la",
            "export GH_TOKEN",
            "TOKEN= ./run",
            "echo 'PASSWORD=hunter2'",
            "git commit -m \"set the token\"",
            "MONKEY=banana make",
            "set -gx EDITOR vim",
            "echo set PASSWORD x",
            "--password",
            "",
        ] {
            assert_eq!(mask_secrets(line), line, "{line}");
        }
    }
}
