//! Deriving Gerrit's REST base URL from a clone's `origin` remote.
//!
//! A change id names no repository, so cwd-inside-a-clone is what supplies the
//! Gerrit (design.md §13). Derivation is a **default**, never a guarantee: an
//! explicit override always wins, and a failure must report the URL that was
//! derived, because a bare 404 naming no host is not actionable.

/// Gerrit serves its authenticated REST API and its authenticated git URLs
/// under this path segment. Its presence in a remote is what makes a
/// subpath-hosted Gerrit derivable.
const AUTH_SEGMENT: &str = "/a/";

/// Gerrit's conventional SSH port. Dropped when moving to HTTPS.
const SSH_PORT: &str = ":29418";

/// Derive the REST base URL from a remote URL, always returning an
/// `https://…/` form with a trailing slash.
///
/// ```
/// # use remendo::gerrit::base_url::derive;
/// // The common HTTP form.
/// assert_eq!(derive("https://gerrit.corp/a/proj").unwrap(), "https://gerrit.corp/");
/// // SSH: host survives, the Gerrit SSH port does not.
/// assert_eq!(derive("ssh://me@gerrit.corp:29418/proj").unwrap(), "https://gerrit.corp/");
/// ```
///
/// **A Gerrit hosted under a URL subpath is recovered when the remote carries
/// the `/a/` segment**, since everything before it is the base:
///
/// ```
/// # use remendo::gerrit::base_url::derive;
/// assert_eq!(derive("https://corp.com/gerrit/a/proj").unwrap(), "https://corp.com/gerrit/");
/// ```
///
/// Without that segment a subpath is genuinely ambiguous — `corp.com/gerrit/proj`
/// could be host `corp.com` serving project `gerrit/proj`, or a Gerrit at
/// `corp.com/gerrit` serving `proj` — so the host alone is returned and the user
/// may need the override.
pub fn derive(remote_url: &str) -> Option<String> {
    let (_scheme, rest) = remote_url.split_once("://")?;
    // Strip any `user@` credential prefix from the authority.
    let rest = rest.rsplit_once('@').map_or(rest, |(_user, host)| host);

    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, String::new()),
    };
    let host = authority.strip_suffix(SSH_PORT).unwrap_or(authority);
    if host.is_empty() {
        return None;
    }

    // Everything before `/a/` is the Gerrit root, which recovers a subpath.
    let prefix = match path.find(AUTH_SEGMENT) {
        Some(idx) => &path[..idx],
        None => "",
    };
    Some(format!("https://{host}{prefix}/"))
}

/// The host of a derived base URL, for the credential lookup and error text.
///
/// ```
/// # use remendo::gerrit::base_url::host_of;
/// assert_eq!(host_of("https://corp.com/gerrit/").unwrap(), "corp.com");
/// ```
pub fn host_of(base_url: &str) -> Option<&str> {
    let (_scheme, rest) = base_url.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_http_form_yields_the_host() {
        assert_eq!(
            derive("https://gerrit.corp/a/proj").unwrap(),
            "https://gerrit.corp/"
        );
    }

    #[test]
    fn an_http_remote_without_the_auth_segment_still_yields_the_host() {
        assert_eq!(
            derive("https://gerrit.corp/proj/sub").unwrap(),
            "https://gerrit.corp/"
        );
    }

    #[test]
    fn ssh_remotes_move_to_https_and_drop_the_gerrit_port() {
        assert_eq!(
            derive("ssh://me@gerrit.corp:29418/proj").unwrap(),
            "https://gerrit.corp/"
        );
        assert_eq!(
            derive("ssh://gerrit.corp:29418/proj/nested").unwrap(),
            "https://gerrit.corp/"
        );
    }

    /// design.md §13's table listed this as the case derivation loses. It does
    /// not, when the remote carries `/a/` — everything before it is the root.
    #[test]
    fn a_subpath_hosted_gerrit_is_recovered_via_the_auth_segment() {
        assert_eq!(
            derive("https://corp.com/gerrit/a/proj").unwrap(),
            "https://corp.com/gerrit/"
        );
        assert_eq!(
            derive("https://corp.com/deep/nest/a/proj").unwrap(),
            "https://corp.com/deep/nest/"
        );
    }

    /// Without `/a/` a subpath is ambiguous, so the host alone comes back and
    /// the user may need the override. Documented rather than guessed at.
    #[test]
    fn a_subpath_without_the_auth_segment_is_ambiguous() {
        assert_eq!(
            derive("https://corp.com/gerrit/proj").unwrap(),
            "https://corp.com/",
            "cannot tell a subpath from a project prefix"
        );
    }

    #[test]
    fn a_user_prefix_is_not_mistaken_for_the_host() {
        assert_eq!(
            derive("https://guilherme@gerrit.corp/a/proj").unwrap(),
            "https://gerrit.corp/"
        );
    }

    #[test]
    fn a_non_gerrit_port_is_preserved() {
        // Only Gerrit's SSH port is dropped; a real HTTPS port must survive.
        assert_eq!(
            derive("https://gerrit.corp:8443/a/proj").unwrap(),
            "https://gerrit.corp:8443/"
        );
    }

    #[test]
    fn a_remote_with_no_path_still_derives() {
        assert_eq!(
            derive("https://gerrit.corp").unwrap(),
            "https://gerrit.corp/"
        );
    }

    #[test]
    fn an_unparseable_remote_yields_nothing() {
        // scp-style remotes carry no scheme; the caller must fall back to the
        // override rather than invent a URL.
        assert_eq!(derive("git@gerrit.corp:proj.git"), None);
        assert_eq!(derive(""), None);
        assert_eq!(derive("https://"), None);
    }

    #[test]
    fn host_is_extracted_from_a_derived_base() {
        assert_eq!(host_of("https://gerrit.corp/").unwrap(), "gerrit.corp");
        assert_eq!(host_of("https://corp.com/gerrit/").unwrap(), "corp.com");
        assert_eq!(
            host_of("https://gerrit.corp:8443/").unwrap(),
            "gerrit.corp:8443"
        );
        assert_eq!(host_of("nonsense"), None);
    }

    #[test]
    fn derivation_round_trips_into_host_lookup() {
        let base = derive("ssh://me@gerrit.corp:29418/proj").unwrap();
        assert_eq!(host_of(&base).unwrap(), "gerrit.corp");
    }
}
