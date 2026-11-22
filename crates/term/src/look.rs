// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the line a program waits on asks for.
//!
//! A program that reads input leaves the cursor after its question. These
//! read that text the way you would: whether it asks for a secret, whether
//! it ends like a question, and which answers it offers.

/// How a terminal looks to someone deciding what it waits for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// The program shows the alternate screen, as a full-screen one does.
    pub full_screen: bool,
    /// The program asked for application cursor keys.
    pub app_cursor: bool,
    /// The text before the cursor on its line, with the rows that line
    /// wrapped from, without styles.
    pub prompt: String,
}

/// Words that ask for something to keep to yourself.
pub(crate) const SECRET_WORDS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "passcode",
    "secret",
    "token",
    "key",
    "pin",
    "code",
    "otp",
];

/// First words of a picker's question, which can name a key or a token
/// without asking for one.
const PICKER_WORDS: &[&str] = &["select", "choose", "pick"];

/// Characters a prompt that hides input echoes for each key.
const MASKS: &[char] = &['*', '•', '●'];

impl Look {
    /// The prompt's words, lowercased, letters and digits only.
    fn words(&self) -> impl Iterator<Item = String> + '_ {
        self.prompt
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
    }

    /// Whether the line reads like it asks for a secret, such as
    /// `? Paste your secret:`, `Password:` or `Enter passphrase for key
    /// '/x':`. The masks a program echoes as you type don't change that.
    /// `Press any key to continue` and a picker's `Select a key type:`
    /// don't.
    pub fn asks_for_secret(&self) -> bool {
        let text = self.prompt.trim_end().trim_end_matches(MASKS).trim_end();
        if !text.ends_with(':') {
            return false;
        }
        let lower = text.to_lowercase();
        if lower.contains("[use arrows") {
            return false;
        }
        let mut words = self.words().peekable();
        if words
            .peek()
            .is_some_and(|first| PICKER_WORDS.contains(&first.as_str()))
        {
            return false;
        }
        words.any(|word| SECRET_WORDS.contains(&word.as_str()))
    }

    /// Whether the line ends the way a question you answer does: after
    /// `:`, `?` or `>`, or after `)` or `]` and a space, as in
    /// `package name: (demo) `. A progress bar that leaves the cursor
    /// after its text, such as `Building [==> ] 3/4: app(bin)`, doesn't.
    pub fn ends_like_a_question(&self) -> bool {
        let spaced = self.prompt.ends_with(' ');
        let text = self.prompt.trim_end();
        match text.chars().last() {
            Some(':' | '?' | '>') => true,
            Some(')' | ']') => spaced,
            _ => false,
        }
    }

    /// The answers the question offers, as you'd type them: `y` and `n`
    /// for `Continue? [y/N]` or `(Y/n)`, `yes` and `no` for `(yes/no)` or
    /// `(yes/no/[fingerprint])`. Empty when it offers no yes and no.
    pub fn choices(&self) -> Vec<String> {
        let text = self.prompt.trim_end().trim_end_matches(['?', ':', ' ']);
        let Some(close) = text.chars().last().filter(|c| matches!(c, ')' | ']')) else {
            return Vec::new();
        };
        let open = if close == ')' { '(' } else { '[' };
        let Some(start) = text.rfind(open) else {
            return Vec::new();
        };
        let inside = &text[start + 1..text.len() - 1];
        let mut yes = None;
        let mut no = None;
        for part in inside.split('/') {
            let part = part.trim().to_lowercase();
            match part.as_str() {
                "y" | "yes" if yes.is_none() => yes = Some(part),
                "n" | "no" if no.is_none() => no = Some(part),
                _ => {}
            }
        }
        match (yes, no) {
            (Some(yes), Some(no)) => vec![yes, no],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(prompt: &str) -> Look {
        Look {
            prompt: prompt.into(),
            ..Look::default()
        }
    }

    #[test]
    fn a_secret_prompt_reads_like_one() {
        for prompt in [
            "? Paste your secret: ",
            "Password:",
            "Enter passphrase for key '/x':",
            "API token: ",
            "? Paste your authentication token: ************",
            "Verification code:",
            "PIN: ••••",
        ] {
            assert!(at(prompt).asks_for_secret(), "{prompt:?}");
        }
        for prompt in [
            "Press any key to continue",
            "? Select a key type:",
            "Select a key type:  [Use arrows to move, type to filter]",
            "? Choose a token scope:",
            "Choose a keyboard layout:",
            "keyboard:",
            "package name: (demo) ",
            "Token saved.",
            "",
        ] {
            assert!(!at(prompt).asks_for_secret(), "{prompt:?}");
        }
    }

    #[test]
    fn a_question_ends_like_one() {
        for prompt in [
            "package name: (demo) ",
            "Name:",
            "Is this OK? ",
            ">>> ",
            "Continue? [y/N] ",
            "Password: ",
        ] {
            assert!(at(prompt).ends_like_a_question(), "{prompt:?}");
        }
        for prompt in [
            "",
            "   ",
            "    Building [=======================> ] 431/432: app(bin)",
            "Downloading 45%",
            "Serving HTTP on 127.0.0.1 port 8000 ...",
            "[y/N]",
        ] {
            assert!(!at(prompt).ends_like_a_question(), "{prompt:?}");
        }
    }

    #[test]
    fn yes_and_no_come_from_the_brackets() {
        let choices = |prompt: &str| at(prompt).choices();
        assert_eq!(choices("Continue? [y/N] "), ["y", "n"]);
        assert_eq!(choices("? Authenticate Git? (Y/n) "), ["y", "n"]);
        assert_eq!(choices("Proceed (yes/no)? "), ["yes", "no"]);
        assert_eq!(choices("Delete it? (y/n)"), ["y", "n"]);
        assert_eq!(
            choices("Are you sure you want to continue connecting (yes/no/[fingerprint])? "),
            ["yes", "no"]
        );
        assert!(choices("Is this OK? (yes) ").is_empty());
        assert!(choices("package name: (demo) ").is_empty());
        assert!(choices("Password: ").is_empty());
        assert!(choices("").is_empty());
    }
}
