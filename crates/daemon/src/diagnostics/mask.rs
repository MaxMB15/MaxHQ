// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Takes credentials out of a command line before a sample keeps it. Any
//! program under `maxd` may take a token in its arguments, and diagnostics
//! go to every client with App rights, a paired phone among them.

use maxhq_core::hooks::template::is_credential;

use crate::clone::redact;
use crate::secrets::TOKEN_STARTS;

/// What stands in for a value taken out.
const MASK: &str = "[hidden]";

/// How other services' tokens start.
const OTHER_TOKEN_STARTS: [&str; 6] = ["sk-", "xox", "glpat-", "AKIA", "eyJ", "npm_"];

/// What can come between `Authorization:` and the credential.
const AUTH_SCHEMES: [&str; 4] = ["bearer", "basic", "token", "digest"];

/// Short words for a password. The header rule leaves them out because
/// `compass` and `bypass` hold them, so these only count as a whole word.
const PASSWORD_WORDS: [&str; 5] = ["pass", "passwd", "pwd", "passphrase", "sshpass"];

/// Flags whose value holds a credential only with this mark in it, so
/// `curl -u a:b` loses its value and `git push -u origin` keeps its.
const LOGIN_FLAGS: [(&str, char); 5] = [
    ("-u", ':'),
    ("-U", ':'),
    ("--user", ':'),
    ("--proxy-user", ':'),
    ("-b", '='),
];

/// What the word before says about this one.
#[derive(Default)]
enum Next {
    #[default]
    Plain,
    /// It's a value, unless it's a long flag.
    Value,
    /// It's a value if it holds this mark.
    Login(char),
}

/// `command` with each word that looks like a credential taken out, and any
/// user, password or query in a URL. It goes word by word, so a value inside
/// a word it can't name, such as a field in JSON, stays.
pub fn masked(command: &str) -> String {
    let command = redact(command);
    let mut next = Next::Plain;
    command
        .split_inclusive(char::is_whitespace)
        .map(|piece| {
            let word = piece.trim_end_matches(char::is_whitespace);
            let gap = &piece[word.len()..];
            masked_word(word, &mut next) + gap
        })
        .collect()
}

/// Whether a variable named `name` with `value` looks like it holds a
/// credential, so a card hides the value until you ask to see it.
pub fn looks_secret(name: &str, value: &str) -> bool {
    names_credential(name) || masked(value) != value
}

/// `word`, or what replaces it. `next` says what the word before named, and
/// this sets it for the word after.
fn masked_word(word: &str, next: &mut Next) -> String {
    if word.is_empty() {
        return String::new();
    }
    let unquoted = word.trim_start_matches(['\'', '"']);
    let bare = word.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
    let is_value = match std::mem::take(next) {
        // `--no-auth --port 5173` names no value, as a long flag follows it.
        Next::Value => !unquoted.starts_with("--"),
        Next::Login(mark) => word.contains(mark),
        Next::Plain => false,
    };
    if is_value {
        if is_scheme(bare) {
            *next = Next::Value;
        }
        return MASK.to_owned();
    }
    if looks_like_token(bare) {
        return MASK.to_owned();
    }
    // A header, with its value in this word or the next.
    if let Some((name, value)) = word.split_once(':')
        && is_header_name(name)
        && names_credential(name)
    {
        if value.is_empty() || is_scheme(value) {
            *next = Next::Value;
            return word.to_owned();
        }
        return format!("{name}:{MASK}");
    }
    if let Some((name, value)) = word.split_once('=')
        && (names_credential(name)
            || looks_like_token(value.trim_start_matches(['\'', '"']))
            || login_mark(name).is_some_and(|mark| value.contains(mark)))
    {
        return format!("{name}={MASK}");
    }
    // `-ualice:s3cret`. A `=` makes it some other flag's value.
    if let Some(value) = unquoted
        .strip_prefix("-u")
        .or_else(|| unquoted.strip_prefix("-U"))
        && value.contains(':')
        && !value.contains('=')
    {
        return format!("{}{MASK}", &unquoted[..2]);
    }
    if let Some(mark) = login_mark(unquoted) {
        *next = Next::Login(mark);
        return word.to_owned();
    }
    let flag = unquoted.starts_with('-') && names_credential(bare);
    let bearer = bare
        .rsplit(':')
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case("bearer"));
    if flag || bearer {
        *next = Next::Value;
    }
    word.to_owned()
}

/// Whether `name` looks like it holds a credential, whatever marks a flag or
/// a quote put around it.
fn names_credential(name: &str) -> bool {
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    is_credential(name)
        || name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| {
                PASSWORD_WORDS
                    .iter()
                    .any(|known| word.eq_ignore_ascii_case(known))
            })
}

/// Whether `name` could be a header's, which a host or a path can't.
fn is_header_name(name: &str) -> bool {
    let name = name.trim_start_matches(['\'', '"']);
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `text` is what comes between `Authorization:` and the credential.
fn is_scheme(text: &str) -> bool {
    AUTH_SCHEMES
        .iter()
        .any(|scheme| text.eq_ignore_ascii_case(scheme))
}

/// The mark that makes `flag`'s value a credential, if `flag` can hold one.
fn login_mark(flag: &str) -> Option<char> {
    LOGIN_FLAGS
        .iter()
        .find(|(name, _)| *name == flag)
        .map(|(_, mark)| *mark)
}

/// Whether `text` starts the way a token does.
fn looks_like_token(text: &str) -> bool {
    TOKEN_STARTS
        .iter()
        .chain(&OTHER_TOKEN_STARTS)
        .any(|start| text.starts_with(start))
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;
    use crate::diagnostics::collect::{Collector, Root};

    #[test]
    fn a_sample_never_holds_the_credential() {
        // `read` keeps the shell waiting on its stdin, so it has no child to
        // leave behind when it's killed.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read line", "sh", "--token", "abc123"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();

        let mut collector = Collector::new();
        collector.prime_cpu_usage();
        let sample = collector.sample(&[Root {
            pid,
            start_time_ms: None,
        }]);
        child.kill().unwrap();
        child.wait().unwrap();

        let process = sample.processes.iter().find(|process| process.pid == pid);
        let command = &process.expect("the child is in the sample").command;
        assert!(command.contains("--token [hidden]"), "{command}");
        assert!(!command.contains("abc123"), "{command}");
    }

    #[test]
    fn a_credential_in_a_command_line_is_taken_out() {
        for (command, kept) in [
            ("tool --token abc123 go", "tool --token [hidden] go"),
            ("tool --token=abc123 go", "tool --token=[hidden] go"),
            ("tool --password abc123", "tool --password [hidden]"),
            ("tool --api-key abc123", "tool --api-key [hidden]"),
            ("tool -token abc123", "tool -token [hidden]"),
            ("API_KEY=abc123 tool", "API_KEY=[hidden] tool"),
            (
                "curl -H Authorization: Bearer abc123 https://example.com/a",
                "curl -H Authorization: [hidden] [hidden] https://example.com/a",
            ),
            (
                "curl -H Authorization:Bearer abc123 https://example.com/a",
                "curl -H Authorization:Bearer [hidden] https://example.com/a",
            ),
            ("curl -H x-api-key: abc123", "curl -H x-api-key: [hidden]"),
            ("tool Bearer abc123", "tool Bearer [hidden]"),
            (
                "git clone https://user:abc123@example.com/a.git",
                "git clone https://example.com/a.git",
            ),
            (
                "curl https://example.com/a?token=abc123 -o b",
                "curl https://example.com/a -o b",
            ),
            ("tool ghp_abc123 go", "tool [hidden] go"),
            ("tool github_pat_abc123", "tool [hidden]"),
            ("tool sk-ant-abc123", "tool [hidden]"),
            ("tool --flag=ghs_abc123", "tool --flag=[hidden]"),
            (
                "bash -c 'tool --token abc123; TOKEN=abc123 tool'",
                "bash -c 'tool --token [hidden] TOKEN=[hidden] tool'",
            ),
        ] {
            assert_eq!(masked(command), kept, "{command}");
        }
    }

    #[test]
    fn other_ways_to_write_a_credential_are_taken_out() {
        let kept: Vec<_> = [
            (
                "curl -u alice:abc123 https://x",
                "curl -u [hidden] https://x",
            ),
            ("curl -ualice:abc123 https://x", "curl -u[hidden] https://x"),
            (
                "curl --user alice:abc123 https://x",
                "curl --user [hidden] https://x",
            ),
            (
                "curl --user=alice:abc123 https://x",
                "curl --user=[hidden] https://x",
            ),
            (
                "curl --proxy-user alice:abc123 https://x",
                "curl --proxy-user [hidden] https://x",
            ),
            ("tool --pass abc123", "tool --pass [hidden]"),
            ("tool --pass=abc123", "tool --pass=[hidden]"),
            ("tool --passwd abc123", "tool --passwd [hidden]"),
            ("tool --pwd abc123", "tool --pwd [hidden]"),
            ("tool --passphrase abc123", "tool --passphrase [hidden]"),
            ("MYSQL_PWD=abc123 tool", "MYSQL_PWD=[hidden] tool"),
            ("DB_PASS=abc123 tool", "DB_PASS=[hidden] tool"),
            ("SSHPASS=abc123 tool", "SSHPASS=[hidden] tool"),
            (
                "curl -H Authorization:Basic abc123",
                "curl -H Authorization:Basic [hidden]",
            ),
            (
                "curl -H Authorization:abc123",
                "curl -H Authorization:[hidden]",
            ),
            ("curl -H x-api-key:abc123", "curl -H x-api-key:[hidden]"),
            ("curl -H x-api-key:a=abc123", "curl -H x-api-key:[hidden]"),
            (
                "curl -H Cookie: session=abc123 https://x",
                "curl -H Cookie: [hidden] https://x",
            ),
            (
                "sh -c 'curl -H \"Cookie: session=abc123\"'",
                "sh -c 'curl -H \"Cookie: [hidden]",
            ),
            ("curl -H Cookie:session=abc123", "curl -H Cookie:[hidden]"),
            ("curl --cookie session=abc123", "curl --cookie [hidden]"),
            ("curl --cookie=session=abc123", "curl --cookie=[hidden]"),
            (
                "curl -b session=abc123 https://x",
                "curl -b [hidden] https://x",
            ),
            (
                "sh -c 'tool --token\tabc123 go'",
                "sh -c 'tool --token\t[hidden] go'",
            ),
            (
                "sh -c 'tool\n--token\nabc123 go'",
                "sh -c 'tool\n--token\n[hidden] go'",
            ),
        ]
        .into_iter()
        .filter(|(command, kept)| masked(command) != *kept)
        .map(|(command, _)| (command, masked(command)))
        .collect();
        assert!(kept.is_empty(), "{kept:#?}");
    }

    #[test]
    fn a_flag_that_can_mean_a_login_keeps_any_other_value() {
        for command in [
            "git push -u origin main",
            "python3 -u run.py",
            "sudo -u root tool",
            "pip install -U ruff",
            "git checkout -b feature",
            "curl -b cookies.txt https://example.com",
            "tool --user alice",
            "tool --compass north --bypass all",
            "tool\twith\ttabs\nand a line",
            "curl https://keys.example.com:443/a",
            "tool keys.example.com:443",
            "docker run -v /a/key:/b/key ghcr.io/a/key:latest",
            "git clone git@example.com:a/key.git",
        ] {
            assert_eq!(masked(command), command);
        }
    }

    #[test]
    fn a_variable_looks_secret_by_its_name_or_its_value() {
        for (name, value) in [
            ("EXAMPLE_TOKEN", "abc"),
            ("EXAMPLE_API_KEY", "abc"),
            ("EXAMPLE_PASSWORD", "abc"),
            ("EXAMPLE_PASS", "abc"),
            ("EXAMPLE_URL", "https://me:hunter2@example.com/"),
            ("EXAMPLE_FLAGS", "--token abc"),
            ("EXAMPLE_VALUE", "sk-abc123"),
        ] {
            assert!(looks_secret(name, value), "{name}={value}");
        }
        for (name, value) in [
            ("EXAMPLE_MODE", "on"),
            ("EXAMPLE_HOME", "/work/app/.example"),
            ("EXAMPLE_URL", "http://127.0.0.1:4000/"),
            ("EXAMPLE_COMPASS", "north"),
        ] {
            assert!(!looks_secret(name, value), "{name}={value}");
        }
    }

    #[test]
    fn an_ordinary_command_line_stays_as_it_is() {
        for command in [
            "node vite --port 5173",
            "cargo test -p maxd -- --nocapture",
            "/bin/sleep 30",
            "tool  two  spaces",
            "git clone https://example.com/a.git",
            "ssh git@example.com",
            "tool --no-auth --port 5173",
            "FOO=bar tool --mode=fast",
        ] {
            assert_eq!(masked(command), command);
        }
    }

    #[test]
    fn a_flag_that_names_no_value_keeps_the_long_flag_after_it() {
        assert_eq!(
            masked("tool --no-auth --token abc"),
            "tool --no-auth --token [hidden]"
        );
    }
}
