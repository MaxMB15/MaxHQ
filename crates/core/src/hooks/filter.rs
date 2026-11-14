// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: how each test reads a value and a `[]` list.

//! A task's filters, which a request must meet before it runs the task.

use maxhq_protocol::tasks::{Filter, FilterTest};
use serde_json::Value;

use super::template::Resolver;
use super::Incoming;

/// The first filter `request` fails, as its index and path, or `None`
/// when it meets them all.
pub fn failed<'f>(filters: &'f [Filter], request: &Incoming) -> Option<(usize, &'f str)> {
    if filters.is_empty() {
        return None;
    }
    let resolver = Resolver::new(request);
    filters
        .iter()
        .enumerate()
        .find(|(_, filter)| !matches(filter, resolver.resolve(&filter.path)))
        .map(|(index, filter)| (index, filter.path.as_str()))
}

/// What a delivery's log says about the filter that failed. Filters are
/// numbered from 1, as the task's settings list them.
pub fn describe(index: usize, path: &str) -> String {
    format!("Filter {} ({path}) did not match.", index + 1)
}

/// Whether `value` meets `filter`. Null counts as missing, as it does in
/// a prompt. On a path with `[]`, `NotEquals` holds when no element
/// equals the value, the others when any element meets them, and
/// `Exists` when the list has an element.
fn matches(filter: &Filter, value: Option<Value>) -> bool {
    match value {
        Some(Value::Array(list)) if filter.path.contains("[]") => {
            let mut texts = list.iter().filter_map(scalar);
            match &filter.test {
                FilterTest::NotEquals { value } => texts.all(|text| text != *value),
                _ => texts.any(|text| meets(&filter.test, &text)),
            }
        }
        None | Some(Value::Null) => false,
        Some(_) if filter.test == FilterTest::Exists => true,
        Some(value) => scalar(&value).is_some_and(|text| meets(&filter.test, &text)),
    }
}

/// Whether one value's text meets `test`.
fn meets(test: &FilterTest, text: &str) -> bool {
    match test {
        FilterTest::Equals { value } => text == value,
        FilterTest::NotEquals { value } => text != value,
        FilterTest::StartsWith { value } => text.starts_with(value.as_str()),
        FilterTest::In { values } => values.iter().any(|value| value == text),
        FilterTest::Exists => true,
    }
}

/// A text, number or true-or-false as text, or `None` for anything else.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::template::tests::headers;

    fn filter(path: &str, test: FilterTest) -> Filter {
        Filter {
            path: path.into(),
            test,
        }
    }

    fn equals(value: &str) -> FilterTest {
        FilterTest::Equals {
            value: value.into(),
        }
    }

    fn not_equals(value: &str) -> FilterTest {
        FilterTest::NotEquals {
            value: value.into(),
        }
    }

    /// Whether an issue comment fails `filters`, and which.
    fn check(filters: &[Filter]) -> Option<(usize, String)> {
        let headers = headers(&[("content-type", "application/json")]);
        let body = r#"{"action":"created","issue":{"number":7,"labels":[{"name":"bug"},{"name":"needs more info"}],"assignees":[]},"comment":{"user":{"login":"reporter","type":"User"}}}"#;
        let request = Incoming {
            method: "POST",
            path: "/maxhq/task",
            query: "",
            headers: &headers,
            body,
        };
        failed(filters, &request).map(|(i, path)| (i, path.to_owned()))
    }

    const LABELS: &str = "body.issue.labels[].name";

    #[test]
    fn a_wildcard_path_matches_when_any_element_does() {
        let tests = [
            equals("needs more info"),
            FilterTest::In {
                values: vec!["needs more info".into(), "triage".into()],
            },
            FilterTest::StartsWith {
                value: "needs".into(),
            },
            FilterTest::Exists,
        ];
        let filters: Vec<_> = tests.into_iter().map(|t| filter(LABELS, t)).collect();
        assert_eq!(check(&filters), None);
        assert_eq!(
            check(&[filter(LABELS, equals("wontfix"))]),
            Some((0, LABELS.into()))
        );
    }

    #[test]
    fn not_equals_on_a_wildcard_fails_when_any_element_equals() {
        assert_eq!(
            check(&[filter(LABELS, not_equals("bug"))]),
            Some((0, LABELS.into()))
        );
        assert_eq!(check(&[filter(LABELS, not_equals("wontfix"))]), None);
    }

    #[test]
    fn an_empty_or_missing_wildcard_list_has_nothing_to_match() {
        let none = "body.issue.assignees[].login";
        assert_eq!(
            check(&[filter(none, FilterTest::Exists)]),
            Some((0, none.into()))
        );
        assert_eq!(check(&[filter(none, not_equals("anyone"))]), None);
        let scalar = "body.issue.number[].x";
        assert_eq!(
            check(&[filter(scalar, FilterTest::Exists)]),
            Some((0, scalar.into()))
        );
    }

    #[test]
    fn scalars_compare_without_a_wildcard_and_the_first_failure_wins() {
        assert_eq!(
            check(&[
                filter("body.action", equals("created")),
                filter("body.comment.user.type", not_equals("Bot")),
                filter("body.issue.number", equals("7")),
            ]),
            None
        );
        // A list without `[]` in its path meets only Exists.
        assert_eq!(
            check(&[
                filter("body.issue.labels", FilterTest::Exists),
                filter("body.issue.labels", equals("bug")),
            ]),
            Some((1, "body.issue.labels".into()))
        );
        assert_eq!(
            describe(1, "body.issue.labels"),
            "Filter 2 (body.issue.labels) did not match."
        );
    }
}
