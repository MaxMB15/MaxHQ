// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Takes credentials out of a command line before a sample keeps it. Any
//! program under `maxd` may take a token in its arguments, and diagnostics
//! go to every client with App rights, a paired phone among them.
//!
//! It goes word by word, as a command line splits. What a command prints
//! goes through [`maxhq_core::mask::mask_text`] instead, which shares the
//! mark and the token starts with this.

use maxhq_core::hooks::template::is_credential;
use maxhq_core::mask::{starts_like_token, MASK};

use crate::clone::redact;

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
    /// It's inside a value that a quote opened, up to the word that closes
    /// it. `shown` says whether the mark already stands for that value.
    Quoted { quote: char, shown: bool },
}

impl Next {
    /// Whether `word` is the value this says comes next.
    fn takes(&self, word: &str) -> bool {
        match *self {
            // `--no-auth --port 5173` names no value, as a long flag follows it.
            Next::Value => !word.trim_start_matches(['\'', '"']).starts_with("--"),
            Next::Login(mark) => word.contains(mark),
            Next::Plain | Next::Quoted { .. } => false,
        }
    }
}

/// The program the words belong to, for the ones that take a password as
/// `-p`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Program {
    #[default]
    Other,
    /// `sshpass -p secret ssh host`, up to the command it runs.
    Sshpass,
    /// `mysql -psecret db`, where a bare `-p` asks for it instead.
    Mysql,
}

/// Where the masking stands from one word to the next.
#[derive(Default)]
struct Words {
    next: Next,
    program: Program,
}

/// `command` with each word that looks like a credential taken out, and any
/// user, password or query in a URL. It goes word by word, so an unquoted
/// value of several words loses only its first; [`masked_args`] knows
/// where each argument ends.
pub fn masked(command: &str) -> String {
    Words::default().line(&redact(command))
}

/// A process's arguments as one command line, masked as [`masked`] does.
/// An argument with spaces in it that a flag before it gives a value to
/// is hidden whole.
pub fn masked_args<S: AsRef<str>>(args: impl IntoIterator<Item = S>) -> String {
    let mut words = Words::default();
    let mut line = String::new();
    for arg in args {
        let arg = redact(arg.as_ref());
        if !line.is_empty() {
            line.push(' ');
        }
        if arg.contains(char::is_whitespace) && words.next.takes(&arg) {
            line.push_str(MASK);
            words.next = Next::Plain;
        } else {
            line.push_str(&words.line(&arg));
            // A quote inside one argument ends with it.
            if matches!(words.next, Next::Quoted { .. }) {
                words.next = Next::Plain;
            }
        }
    }
    line
}

/// Whether a variable named `name` with `value` looks like it holds a
/// credential, so a card hides the value until you ask to see it.
pub fn looks_secret(name: &str, value: &str) -> bool {
    names_credential(name) || masked(value) != value
}

impl Words {
    /// `text` with its credentials taken out, word by word.
    fn line(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for piece in text.split_inclusive(char::is_whitespace) {
            let word = piece.trim_end_matches(char::is_whitespace);
            let gap = &piece[word.len()..];
            if let Next::Quoted { quote, shown } = self.next {
                self.next = if word.contains(quote) {
                    Next::Plain
                } else {
                    Next::Quoted { quote, shown: true }
                };
                // One mark stands for the whole value.
                if !shown {
                    out.push_str(MASK);
                    out.push_str(gap);
                }
                continue;
            }
            out.push_str(&self.word(word));
            out.push_str(gap);
            if word.ends_with([';', '|', '&']) {
                self.program = Program::Other;
            }
        }
        out
    }

    /// `word`, or what replaces it. What the words before named says what
    /// it is, and this sets that for the word after.
    fn word(&mut self, word: &str) -> String {
        if word.is_empty() {
            return String::new();
        }
        let unquoted = word.trim_start_matches(['\'', '"']);
        let bare = word.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
        // The first word that isn't a flag is the command sshpass runs.
        if self.program == Program::Sshpass && !unquoted.starts_with('-') {
            self.program = Program::Other;
        }
        if std::mem::take(&mut self.next).takes(word) {
            self.next = if is_scheme(bare) {
                Next::Value
            } else {
                after_value(word)
            };
            return MASK.to_owned();
        }
        if starts_like_token(bare) {
            return MASK.to_owned();
        }
        if let Some((fields, next)) = masked_fields(word) {
            self.next = next;
            return fields;
        }
        // A header, with its value in this word or the next.
        if let Some((name, value)) = word.split_once(':')
            && is_header_name(name)
            && names_credential(name)
        {
            if value.is_empty() || is_scheme(value) {
                // `"Cookie: a=1; b=2"` hides up to its closing quote.
                self.next = opens(word).map_or(Next::Value, |quote| Next::Quoted {
                    quote,
                    shown: false,
                });
                return word.to_owned();
            }
            self.next = after_value(word);
            return format!("{name}:{MASK}");
        }
        if let Some((name, value)) = word.split_once('=')
            && (names_credential(name)
                || starts_like_token(value.trim_start_matches(['\'', '"']))
                || login_mark(name).is_some_and(|mark| value.contains(mark)))
        {
            self.next = after_value(word);
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
        // `sshpass -p secret`, and `-psecret` for either program.
        if self.program != Program::Other
            && let Some(value) = unquoted.strip_prefix("-p")
        {
            if !value.is_empty() {
                return format!("-p{MASK}");
            }
            if self.program == Program::Sshpass {
                self.next = Next::Value;
            }
            return word.to_owned();
        }
        if let Some(mark) = login_mark(unquoted) {
            self.next = Next::Login(mark);
            return word.to_owned();
        }
        let flag = unquoted.starts_with('-') && names_credential(bare);
        let bearer = bare
            .rsplit(':')
            .next()
            .is_some_and(|last| last.eq_ignore_ascii_case("bearer"));
        if flag || bearer {
            self.next = Next::Value;
        }
        if let Some(program) = program(unquoted) {
            self.program = program;
        }
        word.to_owned()
    }
}

/// What follows a value that was just hidden: the rest of a quote it
/// opened, or the next pair of a cookie such as `a=1; session=abc`.
fn after_value(word: &str) -> Next {
    if let Some(quote) = opens(word) {
        Next::Quoted { quote, shown: true }
    } else if word.ends_with(';') && word.contains('=') {
        Next::Login('=')
    } else {
        Next::Plain
    }
}

/// The quote `word` opens and leaves open, as `"correct` does and
/// `abc"` and `"abc"` don't.
fn opens(word: &str) -> Option<char> {
    if word.ends_with(['\'', '"']) {
        return None;
    }
    let quote = word.chars().find(|c| matches!(c, '\'' | '"'))?;
    (word.matches(quote).count() % 2 == 1).then_some(quote)
}

/// `word` with the value of each JSON field that names a credential taken
/// out, as in `{"token":"abc"}`, and what that says about the next word.
fn masked_fields(word: &str) -> Option<(String, Next)> {
    if !word.contains("\":") {
        return None;
    }
    // Split at the quotes, what's inside them sits at the odd places.
    let mut parts: Vec<&str> = word.split('"').collect();
    let mut next = Next::Plain;
    let mut found = false;
    for at in (1..parts.len().saturating_sub(1)).step_by(2) {
        if parts[at + 1].trim_matches('\\') != ":" || !names_credential(parts[at]) {
            continue;
        }
        found = true;
        match parts.len() - at {
            // `{"token":` with its value in the next word.
            2 => next = Next::Value,
            // `{"password":"correct`, whose value goes on past this word.
            3 => {
                parts[at + 2] = MASK;
                next = Next::Quoted {
                    quote: '"',
                    shown: true,
                };
            }
            _ => parts[at + 2] = MASK,
        }
    }
    found.then(|| (parts.join("\""), next))
}

/// Which of the programs that take a password as `-p` `word` runs.
fn program(word: &str) -> Option<Program> {
    let name = word.rsplit('/').next()?;
    if name == "sshpass" {
        Some(Program::Sshpass)
    } else if name.starts_with("mysql") || name.starts_with("mariadb") {
        Some(Program::Mysql)
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;
    use crate::diagnostics::collect::{Collector, Root};

    #[test]
    fn a_sample_never_holds_the_credential() {
        // `read` keeps the shell waiting on its stdin, so it has no child to
        // leave behind when it's killed. On Windows `cmd /k` waits the same.
        #[cfg(unix)]
        let mut command = Command::new("/bin/sh");
        #[cfg(unix)]
        command.args(["-c", "read line", "sh", "--token", "abc123"]);
        #[cfg(windows)]
        let mut command = Command::new("cmd.exe");
        #[cfg(windows)]
        command.args(["/d", "/q", "/k", "rem", "--token", "abc123"]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
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

    /// The commands that `masked` gives back other than `kept` says.
    fn missed<'a>(cases: &[(&'a str, &'a str)]) -> Vec<(&'a str, String)> {
        cases
            .iter()
            .filter(|(command, kept)| masked(command) != *kept)
            .map(|(command, _)| (*command, masked(command)))
            .collect()
    }

    #[test]
    fn a_password_after_sshpass_or_mysql_dash_p_is_taken_out() {
        let kept = missed(&[
            (
                "sshpass -p s3cretpw ssh host",
                "sshpass -p [hidden] ssh host",
            ),
            (
                "sshpass -p s3cretpw ssh -p 22 host",
                "sshpass -p [hidden] ssh -p 22 host",
            ),
            ("sshpass -ps3cretpw ssh host", "sshpass -p[hidden] ssh host"),
            ("mysql -ps3cretpw db", "mysql -p[hidden] db"),
            (
                "/usr/bin/mysqldump -u root -ps3cretpw db",
                "/usr/bin/mysqldump -u root -p[hidden] db",
            ),
            (
                "sh -c 'mariadb -ps3cretpw db'",
                "sh -c 'mariadb -p[hidden] db'",
            ),
            // A bare `-p` asks for the password, and other programs' `-p`
            // means something else.
            ("mysql -u root -p db", "mysql -u root -p db"),
            ("ssh -p 22 host", "ssh -p 22 host"),
            (
                "sshpass -f pw.txt ssh -p 22 host",
                "sshpass -f pw.txt ssh -p 22 host",
            ),
            ("mysql -p db && cp -p a b", "mysql -p db && cp -p a b"),
        ]);
        assert!(kept.is_empty(), "{kept:#?}");
    }

    #[test]
    fn a_password_of_several_words_is_hidden_whole() {
        assert_eq!(
            masked_args(["tool", "--password", "correct horse battery", "go"]),
            "tool --password [hidden] go"
        );
        // A script with spaces is no value, so only its credential goes.
        assert_eq!(
            masked_args(["sh", "-c", "tool --token abc123 go"]),
            "sh -c tool --token [hidden] go"
        );
        let kept = missed(&[
            (
                "sh -c 'tool --password \"correct horse battery\" go'",
                "sh -c 'tool --password [hidden] go'",
            ),
            (
                "tool --password=\"correct horse battery\" go",
                "tool --password=[hidden] go",
            ),
        ]);
        assert!(kept.is_empty(), "{kept:#?}");
    }

    #[test]
    fn every_pair_of_a_cookie_is_hidden() {
        assert_eq!(
            masked_args([
                "curl",
                "-H",
                "Cookie: a=1; session=s3cretcookie",
                "https://x"
            ]),
            "curl -H Cookie: [hidden] [hidden] https://x"
        );
        let kept = missed(&[
            (
                "curl -H Cookie: a=1; session=s3cretcookie https://x",
                "curl -H Cookie: [hidden] [hidden] https://x",
            ),
            (
                "curl -H Cookie:a=1; session=s3cretcookie",
                "curl -H Cookie:[hidden] [hidden]",
            ),
            (
                "curl -b a=1; session=s3cretcookie",
                "curl -b [hidden] [hidden]",
            ),
            (
                "sh -c 'curl -H \"Cookie: a=1; session=s3cretcookie\" https://x'",
                "sh -c 'curl -H \"Cookie: [hidden] https://x'",
            ),
        ]);
        assert!(kept.is_empty(), "{kept:#?}");
    }

    #[test]
    fn a_json_field_that_names_a_credential_is_hidden() {
        let kept = missed(&[
            (r#"{"token":"s3crettoken"}"#, r#"{"token":"[hidden]"}"#),
            (
                r#"curl -d {"user":"me","password":"s3cret"} https://x"#,
                r#"curl -d {"user":"me","password":"[hidden]"} https://x"#,
            ),
            (
                r#"curl -d '{"token": "s3crettoken"}'"#,
                r#"curl -d '{"token": [hidden]"#,
            ),
            (
                r#"tool {"password":"correct horse"} go"#,
                r#"tool {"password":"[hidden] go"#,
            ),
            (
                r#"curl -d {"name":"app","port":5173}"#,
                r#"curl -d {"name":"app","port":5173}"#,
            ),
        ]);
        assert!(kept.is_empty(), "{kept:#?}");
        let escaped = masked(r#"sh -c "curl -d {\"token\":\"s3crettoken\"}""#);
        assert!(!escaped.contains("s3crettoken"), "{escaped}");
    }
}
