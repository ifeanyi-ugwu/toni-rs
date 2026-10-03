use std::error::Error;
use std::fmt;

use crate::key::short_type_name;
use crate::redact::Redacted;

/// Every failure the servers' `prepare` calls reported, one entry per failure, collected across
/// every server before any of them binds (transports DESIGN §2.7, X6). `StartupError::Configure`
/// carries it, as `StartupError::Wiring` carries `WiringErrors`.
///
/// A server's `prepare` answers one error listing its own failures; the core stores it as one
/// entry, redacted. `Display` writes every entry, and `Debug` writes the same text, so `main`
/// returning `Box<dyn Error>` prints the report.
pub struct ConfigureErrors {
    errors: Vec<ConfigureError>,
}

/// One server's configuration failure: route-table construction, a duplicate route, TLS loading,
/// CORS validation, endpoint parsing, an inherited socket that does not exist, or a backend limit.
#[non_exhaustive]
#[derive(Debug)]
pub struct ConfigureError {
    /// The transport's name, as `StartupError::Bind` names it.
    pub transport: &'static str,
    pub source: Redacted,
}

impl ConfigureErrors {
    pub(crate) fn new(errors: Vec<ConfigureError>) -> Self {
        ConfigureErrors { errors }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, ConfigureError> {
        self.errors.iter()
    }

    pub fn len(&self) -> usize {
        self.errors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}

impl ConfigureError {
    pub(crate) fn new(transport: &'static str, source: Redacted) -> Self {
        ConfigureError { transport, source }
    }
}

impl<'a> IntoIterator for &'a ConfigureErrors {
    type Item = &'a ConfigureError;
    type IntoIter = std::slice::Iter<'a, ConfigureError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter()
    }
}

impl fmt::Display for ConfigureErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.errors.len();
        let noun = if count == 1 { "error" } else { "errors" };
        write!(f, "error: configuration failed with {count} {noun}")?;
        for error in &self.errors {
            write!(f, "\n\n  × {error}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ConfigureErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for ConfigureErrors {}

impl fmt::Display for ConfigureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transport `{}`: {}", short_type_name(self.transport), self.source)
    }
}

impl Error for ConfigureError {}
