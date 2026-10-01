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
    pub(crate) fn register(&mut self, text: String) {
        todo!()
    }

    /// Registers the value when `value` is a `Secret<String>`, which is how `m.value(..)`
    /// registers the secret it binds.
    pub(crate) fn register_if_secret(&mut self, value: &dyn Any) {
        todo!()
    }
}

/// The one function every outside error passes through before a core error type stores it.
pub(crate) fn redact(secrets: &SecretRegistry, error: BoxError) -> Redacted {
    todo!()
}

/// A caught panic payload, converted to a message and redacted the same way.
pub(crate) fn redact_panic(secrets: &SecretRegistry, payload: Box<dyn Any + Send>) -> Redacted {
    todo!()
}

/// The backstop: `scheme://user:password@host` becomes `scheme://[redacted]@host`.
pub(crate) fn strip_userinfo(text: &str) -> String {
    todo!()
}
