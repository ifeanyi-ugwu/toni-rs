//! The one way every transport builds its `prepare` failure (transports DESIGN §12, X22).
//!
//! A server's `prepare` collects what it finds into [`Failures`] and answers
//! [`Failures::into_error`], a `PrepareError` naming every type its failures name, so the startup
//! report writes each type against every transport's failures: by its last path segment, and in
//! full only where another type in the report prints alike. [`zero_bound`] and [`zero_count`] are
//! the refusals of a setting that would refuse everything it governs, with one hint per type on
//! every transport.
//!
//! ```ignore
//! let mut failures = Failures::new();
//! failures.extend(zero_count("max_inflight", self.max_inflight, "shed every call"));
//! failures.extend(zero_bound("ping_interval", self.ping_interval, "close every connection before its first ping"));
//! if !failures.is_empty() {
//!     return Err(failures.into_error().into());
//! }
//! ```

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use ulo::{Bound, BoxError, PrepareError, TypeName};

use crate::count::Count;

/// The failures of one `prepare`, in the order found.
#[derive(Default)]
pub struct Failures(Vec<Failure>);

impl Failures {
    pub fn new() -> Self {
        Failures::default()
    }

    pub fn push(&mut self, failure: impl Into<Failure>) {
        self.0.push(failure.into());
    }

    /// A failure another component answered as a `BoxError`: an upgrade handler's `prepare`, a
    /// link's. A `PrepareError` keeps its names, so they enter the report's naming pass; any other
    /// error keeps its text.
    pub fn push_error(&mut self, error: BoxError) {
        self.0.push(Failure::from_error(error));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The error `prepare` answers, which `listen()` reports as one `StartupError::Configure`
    /// entry for the transport. It names every type the failures name, and writes one failure as
    /// its text and several as a list under "N problems:".
    pub fn into_error(self) -> PrepareError {
        let names: Vec<TypeName> = self.0.iter().flat_map(Failure::names).copied().collect();
        PrepareError::new(names, move |full| {
            let names = Names::new(full);
            match self.0.as_slice() {
                [] => "prepare failed".to_owned(),
                [failure] => failure.text(&names),
                failures => {
                    let mut text = format!("{} problems:", failures.len());
                    for failure in failures {
                        text.push_str("\n  - ");
                        text.push_str(&failure.text(&names));
                    }
                    text
                }
            }
        })
    }

    /// `Ok(())` with no failure, otherwise [`into_error`](Self::into_error) boxed: the tail of a
    /// `prepare`.
    pub fn into_result(self) -> Result<(), BoxError> {
        if self.is_empty() { Ok(()) } else { Err(Box::new(self.into_error())) }
    }
}

impl<F: Into<Failure>> Extend<F> for Failures {
    fn extend<I: IntoIterator<Item = F>>(&mut self, failures: I) {
        self.0.extend(failures.into_iter().map(Into::into));
    }
}

impl fmt::Debug for Failures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.0).finish()
    }
}

/// One failure of a `prepare`. One naming types is written only when the report is, since
/// whether a type prints by its full path depends on every other type the report names.
pub struct Failure(Inner);

enum Inner {
    Plain(String),
    Naming { names: Vec<TypeName>, text: Box<dyn Fn(&Names<'_>) -> String + Send + Sync> },
}

impl Failure {
    /// A failure whose text names no type.
    pub fn plain(text: impl Into<String>) -> Failure {
        Failure(Inner::Plain(text.into()))
    }

    /// A failure naming the types in `names`, each written in `text` through [`Names::of`].
    pub fn naming(names: Vec<TypeName>, text: impl Fn(&Names<'_>) -> String + Send + Sync + 'static) -> Failure {
        Failure(Inner::Naming { names, text: Box::new(text) })
    }

    /// A failure from another component's error: a `PrepareError` keeps its names and writes its
    /// text against the report's, any other error its `Display`.
    pub fn from_error(error: BoxError) -> Failure {
        match error.downcast::<PrepareError>() {
            Ok(prepared) => {
                let names = prepared.names().to_vec();
                Failure::naming(names, move |names| prepared.text(names.full))
            }
            Err(error) => Failure::plain(error.to_string()),
        }
    }

    /// The types this failure names.
    pub fn names(&self) -> &[TypeName] {
        match &self.0 {
            Inner::Plain(_) => &[],
            Inner::Naming { names, .. } => names,
        }
    }

    /// The text, each name written as `names` decides.
    pub fn text(&self, names: &Names<'_>) -> String {
        match &self.0 {
            Inner::Plain(text) => text.clone(),
            Inner::Naming { text, .. } => text(names),
        }
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Failure {
        Failure::plain(text)
    }
}

impl From<&str> for Failure {
    fn from(text: &str) -> Failure {
        Failure::plain(text)
    }
}

impl From<PrepareError> for Failure {
    fn from(error: PrepareError) -> Failure {
        Failure::from_error(Box::new(error))
    }
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = TypeName::colliding(self.names().iter().copied());
        f.write_str(&self.text(&Names::new(&full)))
    }
}

/// How one report writes a type: the names whose short form another type in the report shares.
pub struct Names<'a> {
    full: &'a HashSet<TypeName>,
}

impl<'a> Names<'a> {
    /// The report's set of names to write in full, as `PrepareError`'s text closure receives it.
    pub fn new(full: &'a HashSet<TypeName>) -> Self {
        Names { full }
    }

    /// `User`, or `my_app::User` where another type in the report also prints as `User`.
    pub fn of(&self, name: TypeName) -> String {
        if self.full.contains(&name) { format!("{name:#}") } else { name.to_string() }
    }
}

/// The refusal of `Bound::After(Duration::ZERO)` on the setting `setting`, which would `effect`
/// before what it times could begin; `None` for any other bound. The hint names
/// `Bound::Unbounded`, the type's spelling of no bound:
///
/// ``.ping_interval(Bound::After(Duration::ZERO))` would close every connection before its first
/// ping; write `Bound::Unbounded` to turn the timeout off``
pub fn zero_bound(setting: &str, bound: Bound, effect: &str) -> Option<Failure> {
    (bound == Bound::After(Duration::ZERO)).then(|| {
        Failure::plain(format!(
            "`.{setting}(Bound::After(Duration::ZERO))` would {effect}; write `Bound::Unbounded` to turn the timeout off"
        ))
    })
}

/// The refusal of `Count::Max(0)` on the setting `setting`, which would `effect`; `None` for any
/// other count. The hint names `Count::Unlimited`, the type's spelling of no limit:
///
/// ``.max_inflight(Count::Max(0))` would shed every request; write `Count::Unlimited` to remove
/// the limit``
pub fn zero_count(setting: &str, count: Count, effect: &str) -> Option<Failure> {
    (count == Count::Max(0)).then(|| {
        Failure::plain(format!("`.{setting}(Count::Max(0))` would {effect}; write `Count::Unlimited` to remove the limit"))
    })
}
