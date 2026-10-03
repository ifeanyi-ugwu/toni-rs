//! P27c (naming exchange, coherence claim a, the breaking side): P27 with `impl Classify for
//! CallError` added. The blanket `impl<E: Classify> From<E> for CallError` then covers
//! `From<CallError> for CallError`, which std's reflexive `impl<T> From<T> for T` already
//! provides. Expected: E0119, conflicting implementations of `From<CallError>` for `CallError`,
//! noting the conflicting implementation in crate `core`.
use std::error::Error;
use std::fmt;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Internal,
}

pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;
}

#[derive(Debug)]
pub struct CallError {
    kind: ErrorKind,
    source: Option<BoxError>,
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl Error for CallError {}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        CallError { kind: e.classify(), source: Some(Box::new(e)) }
    }
}

// The impl the design says `CallError` must never have.
impl Classify for CallError {
    fn classify(&self) -> ErrorKind {
        self.kind
    }
}

fn main() {}
