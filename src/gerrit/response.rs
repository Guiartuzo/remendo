//! Decoding Gerrit REST response bodies.
//!
//! Every JSON endpoint prefixes its body with a `)]}'` guard line, which exists
//! to make the response invalid as a `<script>` source and so unusable for
//! cross-site inclusion. It must come off before `serde_json` sees the body.

use serde::de::DeserializeOwned;

use super::GerritError;

/// Gerrit's XSSI guard, the first line of every JSON response body.
const XSSI_GUARD: &str = ")]}'";

/// How much of an unexpected body to quote back in an error. Enough to
/// recognize an HTML login page; short enough not to dump a page into a TUI.
const PREVIEW_LEN: usize = 120;

/// Strip the XSSI guard line from a response body.
///
/// A body that does **not** carry the guard is an error rather than a
/// pass-through. Gerrit always emits it on the JSON API, so its absence means
/// the response is not the JSON API — in practice an HTML login or SSO page
/// served because authentication failed. Passing that to `serde_json` yields
/// "expected value at line 1 column 1", which sends you looking for a parsing
/// bug instead of an auth problem.
///
/// ```
/// # use remendo::gerrit::response::strip_xssi_guard;
/// assert_eq!(strip_xssi_guard(")]}'\n{\"a\":1}").unwrap(), "{\"a\":1}");
/// ```
pub fn strip_xssi_guard(body: &str) -> Result<&str, GerritError> {
    let rest = body
        .strip_prefix(XSSI_GUARD)
        .ok_or_else(|| GerritError::NotJsonApi {
            preview: preview_of(body),
        })?;
    // The guard is its own line; tolerate LF and CRLF, and a body that is
    // nothing but the guard.
    Ok(rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest))
}

/// Strip the guard and deserialize the remaining body.
///
/// ```
/// # use remendo::gerrit::response::decode;
/// # use std::collections::BTreeMap;
/// let map: BTreeMap<String, i32> = decode(")]}'\n{\"a\":1}").unwrap();
/// assert_eq!(map["a"], 1);
/// ```
pub fn decode<T: DeserializeOwned>(body: &str) -> Result<T, GerritError> {
    let json = strip_xssi_guard(body)?;
    serde_json::from_str(json).map_err(|source| GerritError::MalformedJson {
        preview: preview_of(json),
        source,
    })
}

/// The first `PREVIEW_LEN` characters of `body`, on one line, for error text.
fn preview_of(body: &str) -> String {
    let flat: String = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(PREVIEW_LEN)
        .collect();
    let flat = flat.trim().to_string();
    if body.chars().count() > PREVIEW_LEN {
        format!("{flat}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_is_stripped_from_a_normal_body() {
        assert_eq!(strip_xssi_guard(")]}'\n{\"id\":1}").unwrap(), "{\"id\":1}");
    }

    #[test]
    fn a_crlf_guard_line_is_stripped() {
        assert_eq!(
            strip_xssi_guard(")]}'\r\n{\"id\":1}").unwrap(),
            "{\"id\":1}"
        );
    }

    #[test]
    fn a_body_that_is_only_the_guard_leaves_an_empty_string() {
        assert_eq!(strip_xssi_guard(")]}'").unwrap(), "");
    }

    #[test]
    fn json_after_the_guard_deserializes() {
        #[derive(serde::Deserialize)]
        struct Change {
            branch: String,
        }
        let change: Change = decode(")]}'\n{\"branch\":\"main\"}").unwrap();
        assert_eq!(change.branch, "main");
    }

    /// The case this strictness exists for: auth failed and Gerrit served a
    /// login page. The error must say so rather than blame the JSON parser.
    #[test]
    fn an_html_body_is_reported_as_not_the_json_api() {
        let body = "<!DOCTYPE html><html><head><title>Sign in</title></head>";
        let err = strip_xssi_guard(body).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("<!DOCTYPE html>"),
            "error quotes the body: {msg}"
        );
    }

    #[test]
    fn a_long_body_is_truncated_in_the_error() {
        let body = "x".repeat(500);
        let err = strip_xssi_guard(&body).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains('…'), "preview is elided: {msg}");
        assert!(msg.len() < 300, "preview stays short: {} chars", msg.len());
    }

    #[test]
    fn control_characters_do_not_break_the_error_onto_new_lines() {
        let err = strip_xssi_guard("no\nguard\there").unwrap_err();
        assert!(!err.to_string().contains('\n'));
    }

    #[test]
    fn malformed_json_after_a_valid_guard_is_a_distinct_error() {
        let err = decode::<serde_json::Value>(")]}'\n{not json").unwrap_err();
        assert!(matches!(err, GerritError::MalformedJson { .. }));
    }
}
