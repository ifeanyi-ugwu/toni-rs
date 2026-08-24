//! Rendering a driver error without leaking the connection string.
//!
//! Driver messages routinely quote the URL they were given, and a URL carries credentials. The
//! panic that used to report these failures put them straight into the process output.

/// Masks the password in a `scheme://user:password@host/…` URL, leaving everything a reader needs
/// to identify the target. Returns the input unchanged when there is no userinfo to mask.
pub(crate) fn redact(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let authority_start = scheme_end + 3;
    // An `@` after the first `/` of the path belongs to the path, not to userinfo.
    let authority_end = url[authority_start..]
        .find('/')
        .map_or(url.len(), |i| authority_start + i);
    let Some(at) = url[authority_start..authority_end].rfind('@') else {
        return url.to_string();
    };
    let at = authority_start + at;
    let Some(colon) = url[authority_start..at].find(':') else {
        return url.to_string();
    };
    format!("{}:***{}", &url[..authority_start + colon], &url[at..])
}

/// `context: error`, with every occurrence of `url` replaced by its redacted form.
pub(crate) fn describe(context: &str, error: impl std::fmt::Display, url: &str) -> String {
    format!("{context}: {error}").replace(url, &redact(url))
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn masks_the_password() {
        assert_eq!(
            redact("postgres://someone:secret@db.internal:5432/app"),
            "postgres://someone:***@db.internal:5432/app"
        );
    }

    #[test]
    fn leaves_a_url_without_credentials_alone() {
        assert_eq!(redact("redis://127.0.0.1:6379"), "redis://127.0.0.1:6379");
        assert_eq!(
            redact("postgres://someone@host/app"),
            "postgres://someone@host/app"
        );
    }

    #[test]
    fn ignores_an_at_sign_in_the_path() {
        assert_eq!(
            redact("mongodb://host:27017/db@name"),
            "mongodb://host:27017/db@name"
        );
    }

    #[test]
    fn handles_a_url_with_no_scheme() {
        assert_eq!(redact("not-a-url"), "not-a-url");
    }
}
