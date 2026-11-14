// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the usage limit
// code pattern and the check on an error, translated to Rust without a
// regex. The rest is MaxHQ's: where an ACP error keeps the code.

//! Whether an error from `agent acp` is Cursor's usage limit. Cursor names
//! it with codes such as `PRO_USER_USAGE_LIMIT` or `RATE_LIMITED`.

use serde_json::Value;

use crate::drive::acp::rpc::RpcError;

/// Whether `code` is one of Cursor's usage limit codes, as the pattern
/// `/^(?:resource_exhausted|rate_limited(?:_changeable)?|\w*_usage_limit|\w*_rate_limit(?:_exceeded)?)$/i`
/// matches them.
pub fn is_limit_code(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    if !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    matches!(
        code.as_str(),
        "resource_exhausted" | "rate_limited" | "rate_limited_changeable"
    ) || ["_usage_limit", "_rate_limit", "_rate_limit_exceeded"]
        .iter()
        .any(|end| code.ends_with(end))
}

/// Whether an error answer is the usage limit: a limit code anywhere the
/// CLI puts one, an error named `RateLimitError`, or HTTP status 429.
pub fn is_limit(error: &RpcError) -> bool {
    let data = &error.data;
    let named = |name: &str| data.get(name).and_then(Value::as_str);
    named("name") == Some("RateLimitError")
        || ["status", "httpStatus"]
            .iter()
            .any(|name| data.get(*name).and_then(Value::as_u64) == Some(429))
        || ["code", "errorCode", "type", "reason"]
            .iter()
            .filter_map(|name| named(name))
            .any(is_limit_code)
        || data.as_str().is_some_and(is_limit_code)
        || error.message.split_whitespace().any(|word| {
            is_limit_code(word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codes_match_the_limit_pattern() {
        for code in [
            "resource_exhausted",
            "RATE_LIMITED",
            "rate_limited_changeable",
            "PRO_USER_USAGE_LIMIT",
            "_usage_limit",
            "team_rate_limit",
            "api_rate_limit_exceeded",
        ] {
            assert!(is_limit_code(code), "{code}");
        }
        for code in [
            "rate_limit",
            "usage_limit",
            "rate_limited_now",
            "pro-usage_limit",
            "unknown",
            "",
        ] {
            assert!(!is_limit_code(code), "{code}");
        }
    }

    fn error(message: &str, data: Value) -> RpcError {
        RpcError {
            code: -32603,
            message: message.into(),
            data,
        }
    }

    #[test]
    fn errors_name_the_limit_in_their_data_or_message() {
        assert!(is_limit(&error(
            "x",
            json!({ "code": "PRO_USER_USAGE_LIMIT" })
        )));
        assert!(is_limit(&error("x", json!({ "name": "RateLimitError" }))));
        assert!(is_limit(&error("x", json!({ "status": 429 }))));
        assert!(is_limit(&error(
            "Request failed: RATE_LIMITED.",
            Value::Null
        )));
        assert!(!is_limit(&error(
            "Internal error",
            json!({ "code": "internal" })
        )));
        assert!(!is_limit(&error("rate limited soon", Value::Null)));
    }
}
