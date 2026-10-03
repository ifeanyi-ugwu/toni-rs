//! P27 (naming exchange, coherence claim a): `impl<E: Classify> From<E> for CallError` beside std's
//! `impl<T> From<T> for T`, with `CallError` not implementing `Classify`. The two overlap only on
//! `CallError: Classify`, which the defining crate controls, so coherence accepts the pair (the
//! `anyhow::Error` pattern). `?` then converts a `Classify` error inside a fn returning
//! `Result<_, CallError>`, and `E: Into<CallError>` is a bound an `IntoReply` probe arm can carry
//! in place of `E: Classify`, since `CallError` itself satisfies it through the reflexive `From`.
//! Expected: compiles, prints "not_found / not_found / internal".
use std::borrow::Cow;
use std::error::Error;
use std::fmt;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Internal,
}

impl ErrorKind {
    fn name(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "not_found",
            ErrorKind::Internal => "internal",
        }
    }
}

pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;
    fn public_message(&self) -> Cow<'_, str> {
        self.to_string().into()
    }
}

#[derive(Debug)]
pub struct CallError {
    kind: ErrorKind,
    message: String,
    source: Option<BoxError>,
}

impl CallError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CallError { kind, message: message.into(), source: None }
    }
    /// Inherent, never `Classify::classify`: see P27c.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.message)
    }
}

impl Error for CallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|e| e as &(dyn Error + 'static))
    }
}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        let kind = e.classify();
        let message = e.public_message().into_owned();
        CallError { kind, message, source: Some(Box::new(e)) }
    }
}

#[derive(Debug)]
pub struct UserError;
impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("user not found")
    }
}
impl Error for UserError {}
impl Classify for UserError {
    fn classify(&self) -> ErrorKind {
        ErrorKind::NotFound
    }
}

fn find() -> Result<(), UserError> {
    Err(UserError)
}

/// `?` on a `Classify` error inside a fn returning `Result<_, CallError>`.
fn handler() -> Result<(), CallError> {
    find()?;
    Ok(())
}

/// The arm bound the `IntoReply` probe can carry: everything that converts into `CallError`.
fn classify_err<E: Into<CallError>>(r: Result<(), E>) -> ErrorKind {
    match r {
        Ok(()) => ErrorKind::Internal,
        Err(e) => e.into().kind(),
    }
}

fn main() {
    println!("{}", handler().unwrap_err().kind().name());
    println!("{}", classify_err(find()).name());
    println!("{}", classify_err(Err::<(), CallError>(CallError::new(ErrorKind::Internal, "x"))).name());
}
