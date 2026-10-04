use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use crate::redact::{Redacted, SecretRegistry, redact, redact_as};
use crate::timer::BoxError;
use crate::type_name::{TypeName, short_type_name};

/// Every failure the servers' `prepare` calls reported, one entry per failure, collected across
/// every server before any of them binds (transports DESIGN §2.7, X6). `StartupError::Configure`
/// carries it, as `StartupError::Wiring` carries `WiringErrors`.
///
/// A server's `prepare` answers one error listing its own failures; the core stores it as one
/// entry, redacted. `Display` writes every entry, and `Debug` writes the same text, so `main`
/// returning `Box<dyn Error>` prints the report.
///
/// The report applies the wiring report's naming rule across every server: a server answering a
/// [`PrepareFailure`] contributes the types its text names, and two different types printing
/// alike anywhere in the report, `a::User` from one transport and `b::User` from another, print
/// with their full paths in both entries. Each entry's text, `source` included, is written
/// against the whole report's names when the report is built, before it is redacted.
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
    /// The report of every `prepare` that failed, as `(transport name, error)`. Each
    /// [`PrepareFailure`] is written against the names every one of them contributes; any other
    /// error keeps its own text.
    pub(crate) fn collect(refused: Vec<(&'static str, BoxError)>, secrets: &SecretRegistry) -> Self {
        let failures = refused.iter().filter_map(|(_, error)| error.downcast_ref::<PrepareFailure>());
        let full = TypeName::colliding(failures.flat_map(|failure| failure.names.iter().copied()));
        let errors = refused
            .into_iter()
            .map(|(transport, error)| {
                let text = error.downcast_ref::<PrepareFailure>().map(|failure| failure.text(&full));
                let source = match text {
                    Some(text) => redact_as(secrets, error, text),
                    None => redact(secrets, error),
                };
                ConfigureError { transport, source }
            })
            .collect();
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

/// A `prepare` error whose text names types: what a server answers, boxed, so the startup report
/// decides how each name prints. The report runs [`TypeName::colliding`] over the names of every
/// server's `PrepareFailure` and calls `text` with the result, so a type that prints alike with a
/// different type another server names is written with its full path.
///
/// `text` writes each name in `names` with `{:#}` when the set it receives holds it, and with
/// `{}` otherwise. `Display` on a `PrepareFailure` alone collides its own names.
///
/// ```
/// use ulo::{PrepareFailure, TypeName};
///
/// struct User;
/// let user = TypeName::of::<User>();
/// let failure = PrepareFailure::new([user], move |full| {
///     let user = if full.contains(&user) { format!("{user:#}") } else { user.to_string() };
///     format!("`{user}` is read and nothing supplies it")
/// });
/// assert_eq!(failure.to_string(), "`User` is read and nothing supplies it");
/// ```
pub struct PrepareFailure {
    names: Vec<TypeName>,
    text: Box<dyn Fn(&HashSet<TypeName>) -> String + Send + Sync>,
}

impl PrepareFailure {
    pub fn new(
        names: impl IntoIterator<Item = TypeName>,
        text: impl Fn(&HashSet<TypeName>) -> String + Send + Sync + 'static,
    ) -> Self {
        PrepareFailure { names: names.into_iter().collect(), text: Box::new(text) }
    }

    pub fn names(&self) -> &[TypeName] {
        &self.names
    }

    /// The text, each name in `full` written with its full path.
    pub fn text(&self, full: &HashSet<TypeName>) -> String {
        (self.text)(full)
    }
}

impl fmt::Display for PrepareFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text(&TypeName::colliding(self.names.iter().copied())))
    }
}

impl fmt::Debug for PrepareFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for PrepareFailure {}
