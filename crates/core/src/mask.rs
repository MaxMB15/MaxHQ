// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Masks secrets in a command's output before it goes to an agent. The
//! output pill's entry passes its text through [`mask_text`] when the
//! message is encoded (see [`crate::context`]), so the agent and the
//! transcript only ever hold the masked text.

use std::borrow::Cow;

/// `text` with the secrets it holds masked, such as tokens and keys a
/// command printed. For now it gives `text` back as it is: this is the one
/// spot where masking goes.
pub fn mask_text(text: &str) -> Cow<'_, str> {
    Cow::Borrowed(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_through_as_it_is_for_now() {
        for text in ["", "FAIL src/cache.test.ts\n1 failed", "```\n`x`"] {
            assert_eq!(mask_text(text), text);
        }
    }
}
