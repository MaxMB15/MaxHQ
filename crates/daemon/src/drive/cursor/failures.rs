// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Failures `agent acp` reports as an ordinary reply. A dropped connection
//! or a sign-in gone stale ends the turn with `end_turn` and the error as
//! the assistant's text, so the driver reads the whole reply against the
//! texts Cursor is known to send. Only a reply that is nothing but one of
//! them counts, so a reply that talks about such an error doesn't.

use crate::drive::acp::Failure;

/// The longest reply that can still be one of the error texts.
const MAX_CHARS: usize = 600;

/// Replies that mean the sign-in went stale, without case or a full stop.
const SIGN_IN: &[&str] = &["please sign in to continue", "please log in to continue"];

/// How replies that are a failed connection start.
const ERROR_STARTS: &[&str] = &["Error: RetriableError:", "Error: ConnectError:"];

/// Whole replies that are a failed connection.
const ERROR_TEXTS: &[&str] =
    &["Something went wrong communicating with the server. Please try again."];

/// What the reply `text` turned out to be, if it was a failure.
pub fn failure(text: &str) -> Option<Failure> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_CHARS || text.contains("```") {
        return None;
    }
    let bare = text.trim_end_matches('.').to_ascii_lowercase();
    if SIGN_IN.contains(&bare.as_str()) {
        return Some(Failure::SignIn);
    }
    let one_line = !text.contains('\n');
    if (one_line && ERROR_STARTS.iter().any(|start| text.starts_with(start)))
        || ERROR_TEXTS.contains(&text)
    {
        return Some(Failure::Error(text.to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_sign_in_reads_as_one() {
        assert_eq!(failure("Please sign in to continue"), Some(Failure::SignIn));
        assert_eq!(
            failure(" please sign in to continue.\n"),
            Some(Failure::SignIn)
        );
    }

    #[test]
    fn connection_errors_read_as_failures() {
        for text in [
            "Error: RetriableError: WritableIterable is closed",
            "Error: ConnectError: [unavailable] upstream connect error",
            "Error: ConnectError: [aborted] aborted",
            "Something went wrong communicating with the server. Please try again.",
        ] {
            assert_eq!(
                failure(text),
                Some(Failure::Error(text.to_owned())),
                "{text}"
            );
        }
    }

    #[test]
    fn replies_that_mention_errors_are_left_alone() {
        for text in [
            "",
            "Done. The tests pass.",
            "The log said Error: ConnectError: [aborted] aborted, so I retried.",
            "```\nError: RetriableError: x\n```",
            "Error: ConnectError: [aborted] aborted\nThen I fixed it.",
            "You need to sign in to the staging site before the test passes.",
        ] {
            assert_eq!(failure(text), None, "{text}");
        }
    }
}
