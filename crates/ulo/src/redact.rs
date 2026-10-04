//! Redaction (§9.3, §10.2). Every error the core did not create itself, from user code,
//! integrations or transports, passes through [`redact`] before it is stored in any core error
//! type, and what is stored is a [`Redacted`]. The scope is the error's origin, not a list of
//! variants.
//!
//! The function replaces every `Secret<_>` registered with the graph and, as a backstop, strips
//! the userinfo from anything shaped like a URL. On the `try_value` path the registry has
//! nothing to replace, since the secret sits inside the configuration that failed to load; the
//! strip is the only protection there, a backstop and not a guarantee.

use std::any::Any;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::timer::BoxError;

/// An error from outside the core, after the redaction function. Formatting writes the redacted
/// text and never the original; `source()` is `None`, so a reporter walking the chain stops at
/// the text too. The original is reached through [`downcast_ref`](Self::downcast_ref) and
/// [`into_inner`](Self::into_inner) alone, which is what lets an error handler map a
/// constructor's domain error to a status.
pub struct Redacted {
    inner: BoxError,
    text: String,
}

impl Redacted {
    /// The original, by type.
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E> {
        self.inner.downcast_ref::<E>()
    }

    pub fn into_inner(self) -> BoxError {
        self.inner
    }

    pub(crate) fn from_parts(inner: BoxError, text: String) -> Self {
        Redacted { inner, text }
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Error for Redacted {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// A value the redaction function replaces wherever it appears in an error this graph reports,
/// once registered: by `m.secret(&self.url)`, by `#[module]` for every `Secret<_>` field of a
/// configured module, or by binding it with `m.value(..)`.
///
/// `Debug` and `Display` never print the value. Equality and hashing are the value's, so a
/// configured module holding one keeps identity by value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T> {
    value: T,
}

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Secret { value }
    }

    pub fn expose(&self) -> &T {
        &self.value
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: Hash> Hash for Secret<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret<String> {
    fn from(value: String) -> Self {
        Secret::new(value)
    }
}

impl From<&str> for Secret<String> {
    fn from(value: &str) -> Self {
        Secret::new(value.to_owned())
    }
}

/// The secrets one graph registered, as the text the redaction function replaces. Per graph,
/// never process-wide, which would leak between tests.
#[derive(Clone, Default)]
pub(crate) struct SecretRegistry {
    pub(crate) texts: Vec<String>,
}

impl SecretRegistry {
    /// An empty text is ignored: it would match between every two characters.
    pub(crate) fn register(&mut self, text: String) {
        if !text.is_empty() && !self.texts.contains(&text) {
            self.texts.push(text);
        }
    }

    /// Registers the value when `value` is a `Secret<String>`, which is how `m.value(..)`
    /// registers the secret it binds. The stored `Arc<Secret<String>>` of an instance is
    /// recognized too.
    pub(crate) fn register_if_secret(&mut self, value: &dyn Any) {
        let secret = value
            .downcast_ref::<Secret<String>>()
            .or_else(|| value.downcast_ref::<Arc<Secret<String>>>().map(|s| &**s));
        if let Some(secret) = secret {
            self.register(secret.expose().clone());
        }
    }

    /// Every registered text replaced by the marker, longest first, so a secret containing
    /// another is replaced whole. One pass over the text, so a replacement is never matched
    /// again by a shorter secret.
    fn replace(&self, text: &str) -> String {
        let mut secrets: Vec<&str> = self.texts.iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
        if secrets.is_empty() {
            return text.to_owned();
        }
        secrets.sort_by(|a, b| b.len().cmp(&a.len()));

        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'scan: while !rest.is_empty() {
            for secret in &secrets {
                if let Some(after) = rest.strip_prefix(*secret) {
                    out.push_str(MARKER);
                    rest = after;
                    continue 'scan;
                }
            }
            let mut chars = rest.chars();
            if let Some(c) = chars.next() {
                out.push(c);
            }
            rest = chars.as_str();
        }
        out
    }
}

const MARKER: &str = "[redacted]";

/// The one function every outside error passes through before a core error type stores it.
///
/// The text is the error's message followed by each message down its `source()` chain, since
/// `Redacted::source()` answers `None` and a reporter can reach no further than the text. A link
/// whose message the text already holds is not repeated.
pub(crate) fn redact(secrets: &SecretRegistry, error: BoxError) -> Redacted {
    let text = scrub(secrets, &chain_text(&*error));
    Redacted::from_parts(error, text)
}

/// An outside error under a text the core wrote for it, such as a [`PrepareFailure`] written
/// against the whole startup report's names, scrubbed the same way.
///
/// [`PrepareFailure`]: crate::PrepareFailure
pub(crate) fn redact_as(secrets: &SecretRegistry, error: BoxError, text: String) -> Redacted {
    Redacted::from_parts(error, scrub(secrets, &text))
}

/// A caught panic payload, converted to a message and redacted the same way.
pub(crate) fn redact_panic(secrets: &SecretRegistry, payload: Box<dyn Any + Send>) -> Redacted {
    let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic whose payload is not a string".to_owned()
    };
    let text = scrub(secrets, &message);
    Redacted::from_parts(Box::new(PanicMessage(message)), text)
}

/// The backstop: `scheme://user:password@host` becomes `scheme://[redacted]@host`.
///
/// The authority ends where RFC 3986 ends it, at `/`, `?` or `#`, or at whitespace, a quote or
/// an angle bracket around a URL in prose; its last `@` ends the userinfo.
pub(crate) fn strip_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + "://".len());
        out.push_str(head);
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#' | '"' | '\'' | '`' | '<' | '>') || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(i) => {
                out.push_str(MARKER);
                out.push_str(&authority[i..]);
            }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn scrub(secrets: &SecretRegistry, text: &str) -> String {
    strip_userinfo(&secrets.replace(text))
}

/// Bounded, in case a `source()` chain loops back on itself.
const MAX_CHAIN: usize = 32;

fn chain_text(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut next = error.source();
    let mut links = 0;
    while let Some(cause) = next {
        if links == MAX_CHAIN {
            break;
        }
        let message = cause.to_string();
        if !message.is_empty() && !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        next = cause.source();
        links += 1;
    }
    text
}

/// What a panic payload becomes inside a `Redacted`: the message, unredacted, reached only
/// through `into_inner`.
#[derive(Debug)]
struct PanicMessage(String);

impl fmt::Display for PanicMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PanicMessage {}
