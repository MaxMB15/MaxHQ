// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A Windows command line back into its arguments, by the rules the C
//! runtime splits `argv` by, and so the rules Rust's `Command` quotes
//! for: blanks part arguments outside quotes, `2n` backslashes and a
//! quote are `n` backslashes and a quote that opens or closes, `2n + 1`
//! backslashes and a quote are `n` backslashes and a literal quote, and
//! a doubled quote inside quotes is a literal one. The program, first,
//! runs to the next quote when it starts with one, and to a blank if not.

use std::iter::{repeat_n, Peekable};
use std::str::Chars;

/// The arguments `line` holds, the program first.
pub fn split(line: &str) -> Vec<String> {
    let mut chars = line.chars().peekable();
    let mut args = Vec::new();
    let mut program = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        program.extend(chars.by_ref().take_while(|&c| c != '"'));
    } else {
        while let Some(&c) = chars.peek() {
            if blank(c) {
                break;
            }
            program.push(c);
            chars.next();
        }
    }
    if program.is_empty() && chars.peek().is_none() {
        return args;
    }
    args.push(program);
    loop {
        while chars.peek().copied().is_some_and(blank) {
            chars.next();
        }
        if chars.peek().is_none() {
            return args;
        }
        args.push(argument(&mut chars));
    }
}

fn blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The argument `chars` starts at, up to the blank that ends it.
fn argument(chars: &mut Peekable<Chars<'_>>) -> String {
    let mut arg = String::new();
    let mut quoted = false;
    while let Some(&c) = chars.peek() {
        match c {
            c if blank(c) && !quoted => break,
            '\\' => {
                let mut slashes = 0;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    slashes += 1;
                }
                if chars.peek() == Some(&'"') {
                    arg.extend(repeat_n('\\', slashes / 2));
                    if slashes % 2 == 1 {
                        arg.push('"');
                        chars.next();
                    }
                } else {
                    arg.extend(repeat_n('\\', slashes));
                }
            }
            '"' => {
                chars.next();
                if quoted && chars.peek() == Some(&'"') {
                    arg.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                }
            }
            c => {
                arg.push(c);
                chars.next();
            }
        }
    }
    arg
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn plain_arguments_part_at_blanks() {
        assert_eq!(
            split("maxd.exe run  --x\t1"),
            ["maxd.exe", "run", "--x", "1"]
        );
        assert_eq!(split(""), Vec::<String>::new());
        assert_eq!(split("maxd.exe   "), ["maxd.exe"]);
    }

    #[test]
    fn quotes_keep_blanks_in() {
        assert_eq!(
            split(r#""C:\Program Files\MaxHQ\maxd.exe" "C:\Users\<user>\My dir" --detached"#),
            [
                r"C:\Program Files\MaxHQ\maxd.exe",
                r"C:\Users\<user>\My dir",
                "--detached"
            ]
        );
        assert_eq!(split(r#"a b"c d"e"#), ["a", "bc de"]);
    }

    #[test]
    fn backslashes_escape_only_quotes() {
        // A path ending in a backslash, quoted as Rust's Command quotes it.
        assert_eq!(split(r#"a "C:\dir name\\""#), ["a", r"C:\dir name\"]);
        assert_eq!(split(r#"a \"quoted\""#), ["a", r#""quoted""#]);
        assert_eq!(split(r#"a \\\"x"#), ["a", r#"\"x"#]);
        assert_eq!(split(r"a C:\dir\file"), ["a", r"C:\dir\file"]);
        assert_eq!(split(r#"a "x""y""#), ["a", r#"x"y"#]);
    }

    #[test]
    fn the_program_takes_no_escapes() {
        assert_eq!(split(r#""C:\odd\"dir" x"#), [r"C:\odd\", "dir x"]);
    }
}
