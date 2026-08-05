//! [`GerritApi`] over the real REST API, using `ureq`'s blocking client.
//!
//! Every request goes through Gerrit's `/a/` prefix, which is what makes it an
//! *authenticated* call — the unauthenticated paths return only what an
//! anonymous user may see, which on a private Gerrit is nothing.
//!
//! No async runtime: the client blocks, and [`super::worker`] keeps that off
//! the render loop by owning it on a background thread (specs/gerrit-client).

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::api::{ChangeInfo, GerritApi, ReviewInput};
use super::{Comment, GerritError, base_url, response};
use crate::git::Credential;

/// Path segment marking Gerrit's authenticated REST API.
const AUTH_PREFIX: &str = "a/";

/// Options asking Gerrit to include the current revision's detail, which is
/// where the patchset number and its fetch ref live.
const CURRENT_REVISION_OPTS: &str = "?o=CURRENT_REVISION";

/// A Gerrit REST client bound to one host and credential.
#[derive(Debug, Clone)]
pub struct GerritHttp {
    /// Base URL, always with a trailing slash (see [`base_url::derive`]).
    base_url: String,
    /// Pre-encoded `Basic …` header value. Built once so the credential is not
    /// re-encoded per request, and never logged.
    authorization: String,
    /// Git's configured CA path, folded into a TLS failure message so the error
    /// names the file already in play.
    ca_info: Option<String>,
}

impl GerritHttp {
    /// A client for `base_url` authenticating as `credential`.
    ///
    /// ```
    /// # use remendo::gerrit::http::GerritHttp;
    /// # use remendo::git::Credential;
    /// let client = GerritHttp::new(
    ///     "https://gerrit.corp/",
    ///     &Credential { username: "u".into(), password: "p".into() },
    ///     None,
    /// );
    /// assert_eq!(client.url_for("changes/1/comments"), "https://gerrit.corp/a/changes/1/comments");
    /// ```
    pub fn new(base_url: &str, credential: &Credential, ca_info: Option<String>) -> Self {
        let raw = format!("{}:{}", credential.username, credential.password);
        Self {
            base_url: ensure_trailing_slash(base_url),
            authorization: format!("Basic {}", BASE64.encode(raw)),
            ca_info,
        }
    }

    /// The absolute URL for an API path, under the authenticated `/a/` prefix.
    pub fn url_for(&self, path: &str) -> String {
        format!("{}{AUTH_PREFIX}{path}", self.base_url)
    }

    /// The host this client talks to, for error messages.
    fn host(&self) -> &str {
        base_url::host_of(&self.base_url).unwrap_or(&self.base_url)
    }

    /// GET `path` and decode the guarded JSON body.
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GerritError> {
        let url = self.url_for(path);
        let mut res = ureq::get(&url)
            .header("Authorization", &self.authorization)
            .call()
            .map_err(|err| self.map_transport_error(err, &url))?;
        let body = res
            .body_mut()
            .read_to_string()
            .map_err(|err| self.map_transport_error(err, &url))?;
        response::decode(&body)
    }

    /// Translate a `ureq` failure into this module's taxonomy.
    ///
    /// The TLS case is separated on purpose: a Gerrit that `git push` reaches
    /// but Remendo cannot is a trust-store difference, and reporting it as an
    /// auth or network failure costs a long detour (design.md §14).
    fn map_transport_error(&self, err: ureq::Error, url: &str) -> GerritError {
        if let ureq::Error::StatusCode(status) = err {
            return GerritError::HttpStatus {
                status,
                url: url.to_string(),
                body: String::new(),
            };
        }
        let message = err.to_string();
        if is_certificate_failure(&message) {
            return GerritError::tls_trust(self.host(), self.ca_info.as_deref());
        }
        GerritError::Transport {
            url: url.to_string(),
            message,
        }
    }
}

impl GerritApi for GerritHttp {
    fn change(&self, change_id: &str) -> Result<ChangeInfo, GerritError> {
        let path = format!(
            "changes/{}{CURRENT_REVISION_OPTS}",
            encode_segment(change_id)
        );
        self.get(&path).map_err(|err| match err {
            // Gerrit answers 404 for both "no such change" and "not visible to
            // you"; it deliberately does not distinguish them, and neither can we.
            GerritError::HttpStatus { status: 404, .. } => GerritError::NoSuchChange {
                change_id: change_id.to_string(),
            },
            other => other,
        })
    }

    fn comments(&self, change_id: &str) -> Result<HashMap<String, Vec<Comment>>, GerritError> {
        self.get(&format!("changes/{}/comments", encode_segment(change_id)))
    }

    fn robot_comments(
        &self,
        change_id: &str,
    ) -> Result<HashMap<String, Vec<Comment>>, GerritError> {
        // Not every Gerrit deployment serves this endpoint. A 404 here means
        // "no robot comments", not a failed fetch — erroring would make robot
        // support a hard requirement on every server.
        match self.get(&format!(
            "changes/{}/robotcomments",
            encode_segment(change_id)
        )) {
            Err(GerritError::HttpStatus { status: 404, .. }) => Ok(HashMap::new()),
            other => other,
        }
    }

    fn post_review(&self, change_id: &str, review: &ReviewInput) -> Result<(), GerritError> {
        let url = self.url_for(&format!(
            "changes/{}/revisions/current/review",
            encode_segment(change_id)
        ));
        ureq::post(&url)
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/json")
            .send_json(review)
            .map_err(|err| self.map_transport_error(err, &url))?;
        Ok(())
    }
}

/// Whether a transport error message describes a certificate rejection.
///
/// Matched on text because `ureq` erases the rustls error type behind its own
/// transport variant. Over-matching costs only a more helpful message on an
/// unrelated failure; under-matching costs the detour this exists to prevent.
fn is_certificate_failure(message: &str) -> bool {
    let lowered = message.to_lowercase();
    // Needles are lowercased and space-free because rustls names its errors in
    // CamelCase (`CertVerifyError`, `InvalidCertificate(UnknownIssuer)`), which
    // lowercases to one run of letters — a needle with a space never matches.
    ["certificate", "certverify", "unknownissuer", "selfsigned"]
        .iter()
        .any(|needle| lowered.contains(needle))
}

/// Normalize a base URL to end in exactly one `/`.
fn ensure_trailing_slash(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    format!("{trimmed}/")
}

/// Percent-encode one URL path segment.
///
/// Gerrit accepts a change id in three forms, and the triple form
/// `project~branch~ChangeId` embeds the project — which legitimately contains
/// `/`. Interpolated raw, `platform/base~main~I0abc` silently becomes two path
/// segments and addresses a change that does not exist. Unreserved characters
/// (RFC 3986: ALPHA / DIGIT / `-` `.` `_` `~`) pass through, so numeric ids and
/// `I0abc…` ids are untouched.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str) -> GerritHttp {
        GerritHttp::new(
            base,
            &Credential {
                username: "guilherme".into(),
                password: "s3cret".into(),
            },
            None,
        )
    }

    #[test]
    fn urls_go_through_the_authenticated_prefix() {
        let client = client("https://gerrit.corp/");
        assert_eq!(
            client.url_for("changes/12345/comments"),
            "https://gerrit.corp/a/changes/12345/comments"
        );
    }

    #[test]
    fn a_subpath_hosted_gerrit_keeps_its_subpath() {
        let client = client("https://corp.com/gerrit/");
        assert_eq!(
            client.url_for("changes/1"),
            "https://corp.com/gerrit/a/changes/1"
        );
    }

    /// Gerrit's triple-form change id embeds the project, which contains `/`.
    /// Raw, it becomes extra path segments and addresses nothing.
    #[test]
    fn a_triple_form_change_id_is_percent_encoded() {
        // The project's `/` becomes %2F, but the `~` separators stay raw: they
        // are unreserved in RFC 3986 and are Gerrit's own delimiter, so
        // encoding them would break the id Gerrit is trying to parse.
        assert_eq!(
            encode_segment("platform/base~main~I0abc"),
            "platform%2Fbase~main~I0abc"
        );
    }

    #[test]
    fn ordinary_change_ids_pass_through_untouched() {
        assert_eq!(encode_segment("12345"), "12345");
        assert_eq!(
            encode_segment("I0123456789abcdef0123456789abcdef01234567"),
            "I0123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn characters_that_would_break_a_url_are_encoded() {
        assert_eq!(encode_segment("a?b#c d"), "a%3Fb%23c%20d");
        assert_eq!(encode_segment("100%"), "100%25");
    }

    #[test]
    fn a_base_url_without_a_trailing_slash_is_normalized() {
        assert_eq!(
            client("https://gerrit.corp").url_for("changes/1"),
            "https://gerrit.corp/a/changes/1"
        );
        assert_eq!(
            client("https://gerrit.corp///").url_for("changes/1"),
            "https://gerrit.corp/a/changes/1"
        );
    }

    #[test]
    fn basic_auth_is_encoded_once_at_construction() {
        // "guilherme:s3cret" base64-encoded.
        assert_eq!(
            client("https://gerrit.corp/").authorization,
            "Basic Z3VpbGhlcm1lOnMzY3JldA=="
        );
    }

    /// The credential must not be recoverable by eye from a debug dump; the
    /// encoded header is unavoidable, but the plaintext password is not there.
    #[test]
    fn debug_does_not_contain_the_plaintext_password() {
        let shown = format!("{:?}", client("https://gerrit.corp/"));
        assert!(!shown.contains("s3cret"), "{shown}");
    }

    #[test]
    fn the_host_is_recovered_for_error_messages() {
        assert_eq!(client("https://gerrit.corp/").host(), "gerrit.corp");
        assert_eq!(client("https://corp.com/gerrit/").host(), "corp.com");
    }

    #[test]
    fn a_status_error_is_reported_with_its_url() {
        let err = client("https://gerrit.corp/").map_transport_error(
            ureq::Error::StatusCode(403),
            "https://gerrit.corp/a/changes/1",
        );
        assert!(matches!(err, GerritError::HttpStatus { status: 403, .. }));
        assert!(err.to_string().contains("changes/1"), "{err}");
    }

    #[test]
    fn certificate_messages_are_recognized() {
        // Shapes rustls actually produces, in CamelCase.
        assert!(is_certificate_failure(
            "tls connection failed: invalid peer certificate: UnknownIssuer"
        ));
        assert!(is_certificate_failure("CertVerifyError"));
        assert!(is_certificate_failure(
            "InvalidCertificate(SelfSignedCertificate)"
        ));
        assert!(!is_certificate_failure("connection refused"));
        assert!(!is_certificate_failure("dns error: name not resolved"));
        assert!(!is_certificate_failure("timed out"));
    }

    /// The whole point of separating this case: the message must send the user
    /// to git's CA configuration, not to their credentials.
    #[test]
    fn a_tls_failure_points_at_git_ssl_ca_info() {
        let client = GerritHttp::new(
            "https://gerrit.corp/",
            &Credential {
                username: "u".into(),
                password: "p".into(),
            },
            Some("/etc/corp/ca.pem".into()),
        );
        let err = client.map_transport_error(
            ureq::Error::Io(std::io::Error::other(
                "invalid peer certificate: UnknownIssuer",
            )),
            "https://gerrit.corp/a/changes/1",
        );
        let msg = err.to_string();
        assert!(msg.contains("http.sslCAInfo"), "{msg}");
        assert!(msg.contains("/etc/corp/ca.pem"), "names the configured CA");
        assert!(msg.contains("gerrit.corp"), "names the host");
    }

    #[test]
    fn an_unset_ca_path_still_produces_a_useful_message() {
        let err = client("https://gerrit.corp/").map_transport_error(
            ureq::Error::Io(std::io::Error::other("bad certificate")),
            "https://gerrit.corp/a/changes/1",
        );
        assert!(err.to_string().contains("unset"), "{err}");
    }
}
