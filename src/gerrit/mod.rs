//! Gerrit REST access: response decoding, the comment/thread model, and the
//! client that fetches them.
//!
//! The unit this module exposes upward is the **thread**, not the comment
//! (design.md §13). Gerrit returns a flat list of comments linked by
//! `in_reply_to`; everything above this layer sees them already assembled, with
//! a resolved state, an anchor, and a reply target.

pub mod anchor;
pub mod api;
pub mod base_url;
pub mod comment;
pub mod fake;
pub mod http;
pub mod load;
pub mod response;
pub mod thread;
pub mod worker;

pub use anchor::CommentAnchor;
pub use api::{ChangeInfo, GerritApi, ReviewComment, ReviewInput};
pub use comment::{Comment, CommentRange};
pub use fake::FakeGerrit;
pub use http::GerritHttp;
pub use load::{LoadedChange, load_change};
pub use thread::{Thread, ThreadSet};
pub use worker::{GerritEvent, GerritRequest, GerritWorker};

/// Failures reaching or decoding Gerrit.
///
/// Per-module enum rather than a crate-wide one, so a caller never matches on
/// variants its layer cannot produce (design.md §14). Every message carries the
/// offending value, per `config.yaml`.
#[derive(Debug, thiserror::Error)]
pub enum GerritError {
    /// The body carried no XSSI guard, so it is not the JSON API. Almost always
    /// an HTML login or SSO page returned because authentication failed.
    #[error(
        "response is not Gerrit's JSON API (no `)]}}'` guard) — \
         authentication may have failed and returned a login page. Body began: {preview}"
    )]
    NotJsonApi { preview: String },

    /// The guard was present but what followed was not the expected JSON.
    #[error("could not parse Gerrit JSON: {source}. Body began: {preview}")]
    MalformedJson {
        preview: String,
        source: serde_json::Error,
    },

    /// The change does not exist, or the user cannot see it. Reported before
    /// any worktree is created (specs/review-workspace).
    #[error("change `{change_id}` was not found, or you do not have access to it")]
    NoSuchChange { change_id: String },

    /// No REST base URL could be derived and none was configured.
    #[error(
        "could not derive Gerrit's URL from remote `{remote}` = `{url}`. \
         Configure the base URL explicitly."
    )]
    UndeterminedBaseUrl { remote: String, url: String },

    /// Certificate validation failed. Distinguished from an auth failure on
    /// purpose: a Gerrit that `git push` reaches but Remendo cannot is a
    /// trust-store difference, and reporting it as auth costs a long detour.
    #[error(
        "TLS certificate validation failed for `{host}`. Remendo trusts the system root \
         store; if `git push` works against this host, its CA is probably configured in \
         git — check `git config --get http.sslCAInfo`{configured}"
    )]
    TlsTrust { host: String, configured: String },

    /// Gerrit answered, but not with success.
    #[error("Gerrit returned {status} for {url}{body}")]
    HttpStatus {
        status: u16,
        url: String,
        body: String,
    },

    /// The request never completed.
    #[error("could not reach Gerrit at {url}: {message}")]
    Transport { url: String, message: String },
}

impl GerritError {
    /// Build a [`GerritError::TlsTrust`], folding in git's configured CA path
    /// when there is one so the message names the file that is already in play.
    pub fn tls_trust(host: &str, ca_info: Option<&str>) -> Self {
        let configured = match ca_info {
            Some(path) => format!(" (currently `{path}`)"),
            None => " (currently unset)".to_string(),
        };
        GerritError::TlsTrust {
            host: host.to_string(),
            configured,
        }
    }
}
