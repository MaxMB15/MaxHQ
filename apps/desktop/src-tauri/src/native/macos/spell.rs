// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Spell checking through `NSSpellChecker`, which follows the languages
//! and learned words in System Settings.
//!
//! AppKit leaves `NSSpellChecker` off the main thread's list, and checks
//! from a worker thread give the same answers, so these calls run off it.
//! The whole process shares one checker, so a lock takes the calls one at a
//! time. Every call uses one document tag, which makes Ignore Spelling last
//! until the app quits, in every composer.

// The checking call takes raw out pointers.
#![allow(unsafe_code)]

use std::ptr;
use std::sync::{Mutex, OnceLock, PoisonError};

use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{
    NSArray, NSInteger, NSOrthography, NSRange, NSString, NSTextCheckingResult, NSTextCheckingType,
};

use crate::native::{text, Misspelling};

/// How many guesses the menu offers at most.
const MAX_GUESSES: usize = 6;

/// Takes calls into the shared checker one at a time.
static CHECKER: Mutex<()> = Mutex::new(());

/// The app's one spell document tag.
static TAG: OnceLock<NSInteger> = OnceLock::new();

/// Runs `work` with the shared checker and the app's document tag.
fn with_checker<T>(work: impl FnOnce(&NSSpellChecker, NSInteger) -> T) -> T {
    let _turn = CHECKER.lock().unwrap_or_else(PoisonError::into_inner);
    autoreleasepool(|_| {
        let checker = NSSpellChecker::sharedSpellChecker();
        let tag = *TAG.get_or_init(NSSpellChecker::uniqueSpellDocumentTag);
        work(&checker, tag)
    })
}

/// Finds the misspelled words in `text`, one sentence at a time so each
/// gets its own language.
pub fn check(text: &str) -> Vec<Misspelling> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let sentences = text::sentences(&units);
    if sentences.is_empty() {
        return Vec::new();
    }
    with_checker(|checker, tag| {
        let mut found = Vec::new();
        for sentence in sentences {
            let Some(part) = text::slice(&units, sentence.clone()) else {
                continue;
            };
            // A lone surrogate becomes one replacement character, so the
            // string keeps the sentence's length in UTF-16.
            let string = NSString::from_str(&part);
            let mut orthography: Option<Retained<NSOrthography>> = None;
            // SAFETY: There are no options, the orthography slot outlives
            // the call, and a null word count asks for none.
            let results = unsafe {
                checker
                    .checkString_range_types_options_inSpellDocumentWithTag_orthography_wordCount(
                        &string,
                        NSRange::new(0, string.length()),
                        NSTextCheckingType::Spelling.0,
                        None,
                        tag,
                        Some(&mut orthography),
                        ptr::null_mut(),
                    )
            };
            let language = orthography
                .map(|orthography| orthography.dominantLanguage().to_string())
                .filter(|language| language != "und");
            for range in misspelled(&results) {
                let start = sentence.start + range.location;
                let end = start + range.length;
                let Some(word) = text::slice(&units, start..end) else {
                    continue;
                };
                found.push(Misspelling {
                    start: offset(start),
                    end: offset(end),
                    word,
                    language: language.clone(),
                });
            }
        }
        found
    })
}

/// The ranges of the misspelled words among a check's results. A sentence
/// whose language the checker can't settle, such as "file viewer", also
/// comes back with an orthography result over all of it, which is no word.
fn misspelled(results: &NSArray<NSTextCheckingResult>) -> impl Iterator<Item = NSRange> + '_ {
    results
        .iter()
        .filter(|result| result.resultType() == NSTextCheckingType::Spelling)
        .map(|result| result.range())
}

/// An offset for the webview. A composer never holds four billion
/// characters.
fn offset(units: usize) -> u32 {
    u32::try_from(units).unwrap_or(u32::MAX)
}

/// The checker's guesses for a misspelled word, best first.
pub fn guesses(word: &str, language: Option<&str>) -> Vec<String> {
    with_checker(|checker, tag| {
        let string = NSString::from_str(word);
        let language = language.map(NSString::from_str);
        checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                NSRange::new(0, string.length()),
                &string,
                language.as_deref(),
                tag,
            )
            .map(|guesses| {
                guesses
                    .iter()
                    .take(MAX_GUESSES)
                    .map(|guess| guess.to_string())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Ignores a word in every composer until the app quits.
pub fn ignore(word: &str) {
    with_checker(|checker, tag| {
        checker.ignoreWord_inSpellDocumentWithTag(&NSString::from_str(word), tag)
    });
}

/// Adds a word to your macOS dictionary, the same one TextEdit and Mail
/// use.
pub fn learn(word: &str) {
    with_checker(|checker, _| checker.learnWord(&NSString::from_str(word)));
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use objc2_foundation::NSThread;

    use super::*;

    /// Takes a word back out of your macOS dictionary when dropped, so a
    /// failed test can't leave it behind.
    struct Unlearn(&'static str);

    impl Drop for Unlearn {
        fn drop(&mut self) {
            with_checker(|checker, _| checker.unlearnWord(&NSString::from_str(self.0)));
        }
    }

    fn words(text: &str) -> Vec<String> {
        check(text).into_iter().map(|found| found.word).collect()
    }

    /// The words found in `text`, after checking that each one's offsets
    /// point at it.
    fn words_in_place(text: &str) -> Vec<String> {
        let units: Vec<u16> = text.encode_utf16().collect();
        check(text)
            .into_iter()
            .map(|found| {
                let range = found.start as usize..found.end as usize;
                assert_eq!(text::slice(&units, range).as_deref(), Some(&*found.word));
                found.word
            })
            .collect()
    }

    #[test]
    fn only_spelling_results_are_words() {
        let orthography = NSOrthography::defaultOrthographyForLanguage(&NSString::from_str("en"));
        let results = NSArray::from_retained_slice(&[
            NSTextCheckingResult::orthographyCheckingResultWithRange_orthography(
                NSRange::new(0, 17),
                &orthography,
            ),
            NSTextCheckingResult::spellCheckingResultWithRange(NSRange::new(6, 5)),
        ]);
        assert_eq!(
            misspelled(&results).collect::<Vec<_>>(),
            [NSRange::new(6, 5)]
        );
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn short_lines_in_no_clear_language_mark_nothing() {
        // A checklist as a note sends it, with its list marks blanked. The
        // checker can't tell the language of "file viewer" or "test
        // simulation" and says so over the whole line.
        let text = [
            "  [x] Right click notes to create notes",
            "  [x] file viewer",
            "    [ ] Test if agent can also open it for you",
            "  [ ] test simulation",
            "  [ ] updater",
        ]
        .join("\n");
        assert_eq!(words_in_place(&text), Vec::<String>::new());
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn finds_each_word_in_place_around_markdown() {
        // Lines as the note editor sends them: list marks, heading marks and
        // link targets are blanked, and bold's stars and a link's label stay.
        let text = [
            "  [x] Ship the recieve fix",
            "  [ ] wrold peace",
            "    [ ] Nested itme here",
            "   Heding",
            "**Bold** wrods and [a lnik]                  ",
            "Café 😀 naïve tommorow",
            "Tpyo first, then the rest",
            "The rest, then a tpyo",
        ]
        .join("\n");
        assert_eq!(
            words_in_place(&text),
            ["recieve", "wrold", "itme", "Heding", "wrods", "lnik", "tommorow", "Tpyo", "tpyo"]
        );
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn finds_misspellings_off_the_main_thread() {
        assert!(!NSThread::isMainThread_class());
        let text = "This sentance has a typo. Dit is een zin met een fout: probleeem.";
        let found = check(text);
        let units: Vec<u16> = text.encode_utf16().collect();
        for misspelling in &found {
            let range = misspelling.start as usize..misspelling.end as usize;
            assert_eq!(
                text::slice(&units, range).as_deref(),
                Some(&*misspelling.word)
            );
        }
        let words: Vec<&str> = found.iter().map(|found| &*found.word).collect();
        assert!(words.contains(&"sentance"), "{found:?}");
        assert!(words.contains(&"probleeem"), "{found:?}");
        // "Dit", "een" and "zin" are Dutch words, not typos.
        assert!(!words.contains(&"zin"), "{found:?}");
        eprintln!("{found:?}");
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn offsets_count_utf16_after_emoji() {
        let text = "Ship it 🚀 tommorow.";
        let found = check(text);
        assert_eq!(found.len(), 1, "{found:?}");
        // JavaScript's "Ship it 🚀 tommorow.".indexOf("tommorow") is 11.
        assert_eq!((found[0].start, found[0].end), (11, 19));
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn guesses_come_from_the_sentence_language() {
        let found = check("Dit is een groot probleeem voor ons.");
        let misspelling = found
            .iter()
            .find(|found| found.word == "probleeem")
            .expect("probleeem is misspelled");
        let guesses = guesses(&misspelling.word, misspelling.language.as_deref());
        eprintln!("{:?} {guesses:?}", misspelling.language);
        assert!(
            guesses.iter().any(|guess| guess == "probleem"),
            "{guesses:?}"
        );
    }

    #[test]
    #[ignore = "talks to the macOS spell checker"]
    fn ignored_words_stop_showing() {
        let word = "zqxwvmaxhqignore";
        assert_eq!(words(&format!("Say {word} now.")), [word]);
        ignore(word);
        assert!(words(&format!("Say {word} now.")).is_empty());
    }

    #[test]
    #[ignore = "talks to the macOS spell checker and learns, then unlearns, a word"]
    fn learned_words_stop_showing_and_unlearn_cleans_up() {
        let word = "zqxwvmaxhqlearn";
        let learned = with_checker(|checker, _| checker.hasLearnedWord(&NSString::from_str(word)));
        assert!(!learned, "a previous run left {word} in the dictionary");
        let _cleanup = Unlearn(word);
        learn(word);
        assert!(words(&format!("Say {word} now.")).is_empty());
        drop(_cleanup);
        let learned = with_checker(|checker, _| checker.hasLearnedWord(&NSString::from_str(word)));
        assert!(!learned);
    }

    #[test]
    #[ignore = "talks to the macOS spell checker and prints timings"]
    fn times_a_two_thousand_word_check() {
        let sentence =
            "The quick brown fox jumps over the lazy dog while the agent writes a short reply. ";
        let words_per_sentence = sentence.split_whitespace().count();
        let mut text = String::new();
        while text.split_whitespace().count() < 2_000 {
            text.push_str(sentence);
            if text.split_whitespace().count() % 160 < words_per_sentence {
                text.push_str("Ther is a tpyo here. Dit is een Nederlandse zin. ");
            }
        }
        let count = text.split_whitespace().count();

        let started = Instant::now();
        check("Warm up.");
        let warm_up = started.elapsed();

        let mut runs = Vec::new();
        let mut found = 0;
        for _ in 0..5 {
            let started = Instant::now();
            found = check(&text).len();
            runs.push(started.elapsed());
        }
        let started = Instant::now();
        let short = check("Ther is a tpyo here.").len();
        let one_sentence = started.elapsed();
        eprintln!(
            "first call {warm_up:?}; {count} words, {} UTF-16 units, {found} found: {runs:?}; one sentence ({short} found): {one_sentence:?}",
            text.encode_utf16().count()
        );
        assert!(found > 0);
    }
}
