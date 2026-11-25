// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows command lines, by the rules the C runtime splits `argv` by, and
//! so the rules Rust's `Command` quotes for: blanks part arguments outside
//! quotes, `2n` backslashes and a quote are `n` backslashes and a quote
//! that opens or closes, `2n + 1` backslashes and a quote are `n`
//! backslashes and a literal quote, and a doubled quote inside quotes is a
//! literal one. The program, first, runs to the next quote when it starts
//! with one, and to a blank if not.
//!
//! [`split`] reads a line back into its arguments and [`join`] writes one.
//! Both are text rules, so they build and are tested on every system.

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

/// `args` as one command line that [`split`] and the C runtime read back
/// as the same arguments, written into `line`: as UTF-16 for a call to
/// Windows, or as `char`s into a `String`. An argument goes as it is when
/// it holds no blank or quote, and otherwise in quotes, with a quote in it
/// after a backslash and the backslashes before a quote, or before the
/// closing one, doubled. Quoting at a line feed or a vertical tab too
/// changes nothing for the C runtime, which parts arguments only at a
/// space or tab, and keeps the argument whole for a reader that parts them
/// there as well.
pub fn join<U, A>(args: impl IntoIterator<Item = A>, line: &mut impl Extend<U>)
where
    U: Copy + PartialEq + From<u8>,
    A: Iterator<Item = U> + Clone,
{
    for (at, arg) in args.into_iter().enumerate() {
        if at > 0 {
            line.extend([U::from(b' ')]);
        }
        quote(arg, line);
    }
}

/// [`join`] for text.
pub fn join_str<'a>(args: impl IntoIterator<Item = &'a str>) -> String {
    let mut line = String::new();
    join(args.into_iter().map(str::chars), &mut line);
    line
}

/// Adds `arg` to `line` as [`join`] quotes it.
fn quote<U, A>(arg: A, line: &mut impl Extend<U>)
where
    U: Copy + PartialEq + From<u8>,
    A: Iterator<Item = U> + Clone,
{
    let [quote, backslash] = [b'"', b'\\'].map(U::from);
    let blanks = [b' ', b'\t', b'\n', 0x0b].map(U::from);
    let plain = arg.clone().next().is_some()
        && !arg
            .clone()
            .any(|unit| unit == quote || blanks.contains(&unit));
    if plain {
        line.extend(arg);
        return;
    }
    line.extend([quote]);
    let mut backslashes = 0;
    for unit in arg {
        if unit == backslash {
            backslashes += 1;
        } else {
            if unit == quote {
                line.extend(repeat_n(backslash, backslashes + 1));
            }
            backslashes = 0;
        }
        line.extend([unit]);
    }
    line.extend(repeat_n(backslash, backslashes));
    line.extend([quote]);
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
    use super::{join, join_str, split};

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

    #[test]
    fn arguments_are_quoted_only_when_they_need_it() {
        assert_eq!(join_str(["plain", ""]), r#"plain """#);
        assert_eq!(join_str(["two words"]), r#""two words""#);
        assert_eq!(join_str([r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(join_str([r"C:\a dir\"]), r#""C:\a dir\\""#);
        assert_eq!(join_str([r"C:\plain\path\"]), r"C:\plain\path\");
        assert_eq!(join_str([r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(join_str([r"in\between x"]), r#""in\between x""#);
    }

    #[test]
    fn a_line_feed_or_vertical_tab_is_quoted_too() {
        assert_eq!(join_str(["a\nb"]), "\"a\nb\"");
        assert_eq!(join_str(["a\u{b}b"]), "\"a\u{b}b\"");
    }

    #[test]
    fn what_join_writes_splits_back_into_the_same_arguments() {
        let cases: &[&[&str]] = &[
            &["maxd.exe", "run", "--x", "1"],
            &[r"C:\Program Files\MaxHQ\maxd.exe", "", "two words", "\t"],
            &[
                "ssh",
                r#"say "hi""#,
                r#"a\"b"#,
                r"C:\a dir\",
                r"\\server\share",
            ],
            &[
                "x",
                "a\nb",
                "a\u{b}b",
                r#"\\""#,
                "\"\"",
                "{\"json\": [1, \"two\"]}",
            ],
        ];
        for args in cases {
            assert_eq!(split(&join_str(args.iter().copied())), *args, "{args:?}");
        }
    }

    #[test]
    fn a_wide_line_is_quoted_as_text_is() {
        let mut line: Vec<u16> = Vec::new();
        join(["a b", "c"].map(str::encode_utf16), &mut line);
        assert_eq!(String::from_utf16(&line).unwrap(), r#""a b" c"#);
    }
}
