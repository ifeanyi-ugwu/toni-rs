//! P27b (naming exchange, coherence claim b): `impl From<BoxError> for CallError` and
//! `impl From<std::io::Error> for CallError` beside the blanket `impl<E: Classify> From<E>`. Each
//! pair overlaps only if `Box<dyn Error + Send + Sync>: Classify` or `io::Error: Classify` could
//! hold. `Classify` is local to this crate, so only this crate can implement it for a foreign type
//! (`Box` is `#[fundamental]`, but `dyn Error` is std's, so no downstream crate may), and no
//! upstream crate knows the trait. The author expected coherence to reject this and avoided it by
//! naming the constructor `from_boxed`. Expected: compiles (coherence accepts; the trait being
//! local is what decides it, where `anyhow` is refused because `std::error::Error` is foreign),
//! prints "boxed: internal / io: internal / user: not_found".
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
    #[allow(dead_code)]
    source: Option<BoxError>,
}

impl CallError {
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.name())
    }
}

impl Error for CallError {}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        CallError { kind: e.classify(), source: Some(Box::new(e)) }
    }
}

// The pair the author avoided: a `From` for the erased error beside the blanket.
impl From<BoxError> for CallError {
    fn from(e: BoxError) -> Self {
        CallError { kind: ErrorKind::Internal, source: Some(e) }
    }
}

// And one for a concrete foreign type, the shape `From<LookupError>` would take.
impl From<std::io::Error> for CallError {
    fn from(e: std::io::Error) -> Self {
        CallError { kind: ErrorKind::Internal, source: Some(Box::new(e)) }
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

fn main() {
    let boxed: BoxError = Box::new(UserError);
    println!("boxed: {}", CallError::from(boxed).kind().name());
    println!("io: {}", CallError::from(std::io::Error::other("io")).kind().name());
    println!("user: {}", CallError::from(UserError).kind().name());
}
