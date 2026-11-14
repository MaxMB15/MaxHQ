// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Values that must never reach a log, such as a setup-token or a GitHub
//! App's private key.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;
use zeroize::Zeroize;

/// A token, key or code on its way to `maxd`. Debug and Display print
/// `<secret>`, and the bytes are zeroed on drop. A string in TypeScript.
///
/// Only [`Secret::expose`] gives the value back, so every place that reads
/// it is easy to find.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value. The caller's copy is moved in, not left behind.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The value itself, for the one place that sends or uses it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the value is empty, which no real token or key is.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The value with every space and line break taken out, for a token
    /// copied from a terminal that wrapped it. Never for a key, whose line
    /// breaks belong to it. Room for the whole value up front means the
    /// buffer never moves, so it leaves no copy behind.
    pub fn without_whitespace(&self) -> Self {
        let mut value = String::with_capacity(self.0.len());
        value.extend(self.0.chars().filter(|c| !c.is_whitespace()));
        Self(value)
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<secret>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<secret>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUE: &str = "test-not-a-real-token";

    #[test]
    fn secrets_never_print_their_value() {
        let secret = Secret::new(VALUE.into());
        assert_eq!(format!("{secret}"), "<secret>");
        assert_eq!(format!("{secret:?}"), "<secret>");
        assert_eq!(format!("{secret:#?}"), "<secret>");
        let wrapped = Some(vec![("CLAUDE_CODE_OAUTH_TOKEN", secret)]);
        assert!(!format!("{wrapped:?}").contains(VALUE));
    }

    #[test]
    fn secrets_travel_as_plain_strings() {
        let secret = Secret::from(VALUE.to_string());
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, format!("\"{VALUE}\""));
        let parsed: Secret = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, secret);
        assert_eq!(parsed.expose(), VALUE);
        assert!(!parsed.is_empty());
    }

    #[test]
    fn a_wrapped_value_comes_back_whole() {
        let wrapped = Secret::new(" test-not-\n  a-real\t-token \r\n".into());
        assert_eq!(wrapped.without_whitespace().expose(), VALUE);
        assert!(Secret::new(" \n ".into()).without_whitespace().is_empty());
    }
}
