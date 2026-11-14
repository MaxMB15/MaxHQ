// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: finding the PRs
// a command output created, rewritten without regular expressions and for GitHub only: a shell
// wrapper is unwrapped, heredoc bodies, quoted text and comments don't run,
// a command that lists PRs voids the output, a number the other commands
// name existed before, and more URLs than create commands take none.

//! The PRs a shell command opened, read from the command and its output,
//! so linking one costs no call to GitHub.

use maxhq_protocol::pulls::PullKey;

/// The PRs `command` opened, going by `output`: the GitHub PR URLs on
/// lines of their own, when the command runs `gh pr create`. A PR the
/// command also names, as `gh pr view 12` does, was there before it ran.
pub fn created_by(command: &str, output: &str) -> Vec<PullKey> {
    if !command.contains("gh") {
        return Vec::new();
    }
    let script = unwrap_shell(command);
    let code = code_of(&script);
    let creates = creates_in(&code);
    if creates == 0 {
        return Vec::new();
    }
    let others: Vec<&str> = split_code(&code)
        .filter(|part| creates_in(part) == 0)
        .collect();
    if others.iter().any(|part| lists_pulls(part)) {
        return Vec::new();
    }
    let other_script: Vec<&str> = split_script(&script)
        .into_iter()
        .filter(|segment| creates_in(&code_of(segment)) == 0)
        .collect();
    let mut created: Vec<PullKey> = Vec::new();
    for line in output.lines() {
        let Some(key) = pull_url(line.trim_matches([' ', '\t'])) else {
            continue;
        };
        if other_script.iter().any(|text| names(text, key.number)) || created.contains(&key) {
            continue;
        }
        created.push(key);
    }
    if created.len() > creates {
        return Vec::new();
    }
    created
}

/// Whether `command` may have changed a PR on GitHub: it runs `git push`,
/// `gh pr merge` or `gh pr close`, so the watcher reads its repository
/// now rather than at its next turn.
pub fn changes_pulls(command: &str) -> bool {
    if !command.contains("git") && !command.contains("gh") {
        return false;
    }
    commands(&code_of(&unwrap_shell(command)))
        .iter()
        .any(|words| match words.as_slice() {
            ["git", "push", ..] => true,
            ["gh", "pr", verb, ..] => matches!(*verb, "merge" | "close"),
            _ => false,
        })
}

/// The script a command runs, out of a `sh -c` wrapper such as Codex's
/// `/bin/zsh -lc '...'`. The wrapper's argument may be quoted in pieces.
fn unwrap_shell(command: &str) -> String {
    let trimmed = command.trim_start();
    let Some((shell, rest)) = trimmed.split_once(char::is_whitespace) else {
        return command.to_owned();
    };
    let shell = shell.rsplit('/').next().unwrap_or(shell);
    let rest = rest.trim_start();
    let Some((flag, wrapped)) = rest.split_once(char::is_whitespace) else {
        return command.to_owned();
    };
    let wrapped = wrapped.trim_start();
    let is_shell = matches!(shell, "sh" | "bash" | "zsh" | "dash" | "ksh");
    if !is_shell || !matches!(flag, "-c" | "-lc") || !wrapped.starts_with(['"', '\'']) {
        return command.to_owned();
    }
    let mut script = String::new();
    let mut chars = wrapped.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => script.extend(chars.by_ref().take_while(|c| *c != '\'')),
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(next @ ('"' | '\\' | '$' | '`')) => script.push(next),
                            Some(next) => {
                                script.push('\\');
                                script.push(next);
                            }
                            None => script.push('\\'),
                        },
                        c => script.push(c),
                    }
                }
            }
            '\\' => script.extend(chars.next()),
            c if c.is_whitespace() => {}
            c => script.push(c),
        }
    }
    script
}

/// The commands a script runs, without the text it carries: heredoc
/// bodies become `<<`, quoted text `''`, and comments go.
fn code_of(script: &str) -> String {
    let script = without_heredocs(script);
    let mut code = String::with_capacity(script.len());
    let mut chars = script.chars().peekable();
    let mut word_start = true;
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                chars.by_ref().find(|c| *c == '\'');
                code.push_str("''");
            }
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            chars.next();
                        }
                        _ => {}
                    }
                }
                code.push_str("''");
            }
            '#' if word_start => while chars.next_if(|c| *c != '\n').is_some() {},
            c => code.push(c),
        }
        word_start = code
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || ";&|(".contains(c));
    }
    code
}

/// A script with each heredoc's body cut down to `<<`.
fn without_heredocs(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(at) = rest.find("<<") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        if let Some(string) = after.strip_prefix('<') {
            // A here-string, `<<<`, carries no body.
            out.push_str("<<<");
            rest = string;
            continue;
        }
        let spec = after.strip_prefix('-').unwrap_or(after);
        let spec = spec.trim_start_matches([' ', '\t']);
        let (quote, spec) = match spec.chars().next() {
            Some(q @ ('\'' | '"')) => (Some(q), &spec[1..]),
            _ => (None, spec),
        };
        let word_len = spec
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(spec.len());
        let word = &spec[..word_len];
        let closes = quote.is_none_or(|q| spec[word_len..].starts_with(q));
        if word.is_empty() || !closes {
            out.push_str("<<");
            rest = after;
            continue;
        }
        out.push_str("<<");
        // The body runs from the next line to a line holding only the
        // word, or to the end.
        rest = "";
        let Some(newline) = after.find('\n') else {
            break;
        };
        let mut at = newline + 1;
        while at < after.len() {
            let end = after[at..].find('\n').map_or(after.len(), |n| at + n);
            if after[at..end].trim_matches([' ', '\t']) == word {
                rest = &after[end..];
                break;
            }
            at = end + 1;
        }
    }
    out.push_str(rest);
    out
}

/// Code split where commands end. Quoted text is already gone from code,
/// so no separator hides in it.
fn split_code(code: &str) -> impl Iterator<Item = &str> {
    code.split(['\n', ';', '&', '|'])
}

/// A script split where commands end, outside quotes.
fn split_script(script: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (at, c) in script.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('"'), '\\') | (None, '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, ';' | '&' | '|' | '\n') => {
                segments.push(&script[start..at]);
                start = at + 1;
            }
            (None, _) => {}
        }
    }
    segments.push(&script[start..]);
    segments
}

/// The commands in one stretch of code, each as its words. A command
/// starts after a separator, an opening bracket or backtick, or `do`,
/// `then` and `else`, past `NAME=value` and wrappers like `timeout 60`.
fn commands(code: &str) -> Vec<Vec<&str>> {
    let mut commands = Vec::new();
    for part in split_code(code).flat_map(|part| part.split(['(', '{', '`'])) {
        let mut words = part.split_whitespace().peekable();
        while let Some(word) = words.peek() {
            let assignment = word.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            });
            match *word {
                "do" | "then" | "else" | "env" | "time" | "nohup" => {}
                "timeout" => {
                    words.next();
                }
                _ if assignment => {}
                _ => break,
            }
            words.next();
        }
        commands.push(words.collect());
    }
    commands
}

/// How many `gh pr create` commands some code runs. The subcommand must
/// end its word: `gh pr create#x` runs another one.
fn creates_in(code: &str) -> usize {
    commands(code)
        .iter()
        .filter(|words| match words.as_slice() {
            ["gh", "pr", create, ..] => create
                .strip_prefix("create")
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(')')),
            _ => false,
        })
        .count()
}

/// Whether code runs a command that prints other PRs' URLs, so output
/// beside it proves nothing.
fn lists_pulls(code: &str) -> bool {
    let words: Vec<&str> = code
        .split(|c: char| c.is_whitespace() || "(){}`".contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).enumerate().any(|(at, pair)| match pair {
        ["gh", "api"] => true,
        ["gh", "pr"] => matches!(words.get(at + 2), Some(&("list" | "status"))),
        ["gh", "search"] => words.get(at + 2) == Some(&"prs"),
        _ => false,
    })
}

/// The PR a line's URL names, as `https://github.com/owner/repo/pull/12`.
fn pull_url(line: &str) -> Option<PullKey> {
    if line.contains(char::is_whitespace) {
        return None;
    }
    let rest = line
        .strip_prefix("https://")
        .or_else(|| line.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let mut parts = path.splitn(4, '/');
    let (owner, name, pull, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let end = number
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(number.len());
    let (digits, after) = number.split_at(end);
    let tail_ok = after.is_empty() || after.starts_with(['/', '?', '#']);
    let number: u64 = digits.parse().ok().filter(|number| *number > 0)?;
    (pull == "pull" && !owner.is_empty() && !name.is_empty() && tail_ok)
        .then(|| super::key(&format!("{owner}/{name}"), number))
}

/// Whether `text` names PR `number` as a word of its own, as `#12`,
/// `/pull/12` or `view 12` do.
fn names(text: &str, number: u64) -> bool {
    let number = number.to_string();
    text.match_indices(&number).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + number.len()..].chars().next();
        before.is_none_or(|c| c.is_whitespace() || "#/=(\"'".contains(c))
            && after.is_none_or(|c| c.is_whitespace() || ");&|/\"'".contains(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/Owner/Repo/pull/12";

    fn numbers(command: &str, output: &str) -> Vec<u64> {
        created_by(command, output)
            .into_iter()
            .map(|key| key.number)
            .collect()
    }

    #[test]
    fn a_create_command_links_the_url_it_printed() {
        let output = format!("Creating pull request\n{URL}\n");
        assert_eq!(
            created_by("gh pr create --fill", &output),
            [crate::pulls::key("owner/repo", 12)]
        );
    }

    #[test]
    fn what_counts_as_a_create() {
        let output = format!("{URL}\n");
        let cases: &[(&str, &[u64])] = &[
            ("git push -u origin HEAD && gh pr create --fill", &[12]),
            ("GH_TOKEN=x timeout 60 gh pr create -t t", &[12]),
            ("cd repo; if true; then gh pr create; fi", &[12]),
            ("url=$(gh pr create --fill)", &[12]),
            ("/bin/zsh -lc 'gh pr create --title \"Fix it\"'", &[12]),
            ("bash -c \"gh pr create\"", &[12]),
            // Text that only mentions the command doesn't run it.
            ("echo \"gh pr create\"", &[]),
            ("echo hi # gh pr create", &[]),
            ("gh pr create#x", &[]),
            ("gh pr view 12", &[]),
            // A command that lists PRs voids the output.
            ("gh pr create --fill && gh pr list", &[]),
            ("gh pr create; gh api repos/owner/repo/pulls", &[]),
            // The other commands name it, so it was there before.
            ("gh pr create || gh pr view 12", &[]),
            (
                "gh pr create; printf 'https://github.com/owner/repo/pull/12'",
                &[],
            ),
            ("gh pr create; echo 123", &[12]),
        ];
        for (command, want) in cases {
            assert_eq!(numbers(command, &output), *want, "{command}");
        }
    }

    #[test]
    fn a_heredoc_body_is_text() {
        let command = "cat <<'EOF' > notes\ngh pr create\nEOF\necho done";
        assert!(numbers(command, URL).is_empty());
        let command = "gh pr create --body-file - <<EOF\nFixes the build\nEOF";
        assert_eq!(numbers(command, URL), [12]);
    }

    #[test]
    fn only_github_pr_urls_on_lines_of_their_own_count() {
        let output = "see https://github.com/owner/repo/pull/3\n\
                      https://example.com/owner/repo/pull/4\n\
                      https://github.com/owner/repo/issues/5\n\
                      \thttps://github.com/owner/repo/pull/6/files \n\
                      https://github.com/owner/repo/pull/0\n";
        assert_eq!(numbers("gh pr create", output), [6]);
    }

    #[test]
    fn more_urls_than_creates_take_none() {
        let output = "https://github.com/owner/repo/pull/1\nhttps://github.com/owner/repo/pull/2\n";
        assert!(numbers("gh pr create", output).is_empty());
        assert_eq!(
            numbers("gh pr create -B a && gh pr create -B b", output),
            [1, 2]
        );
        let twice = "https://github.com/owner/repo/pull/1\nhttps://github.com/owner/repo/pull/1\n";
        assert_eq!(numbers("gh pr create", twice), [1]);
    }

    #[test]
    fn a_push_merge_or_close_changes_prs() {
        for command in [
            "git push -u origin fix",
            "/bin/zsh -lc 'cargo test && git push'",
            "gh pr merge 12 --squash",
            "gh pr close 12",
        ] {
            assert!(changes_pulls(command), "{command}");
        }
        for command in [
            "git status",
            "echo 'git push'",
            "gh pr view 12",
            "# git push",
        ] {
            assert!(!changes_pulls(command), "{command}");
        }
    }
}
