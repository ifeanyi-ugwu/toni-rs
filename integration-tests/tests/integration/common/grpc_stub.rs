//! The error a gRPC test's unserved methods answer with.
//!
//! A handler's error type implements `toni::Error`, so a proto method a test
//! does not exercise cannot answer `Status::unimplemented` directly. This is
//! that answer, once, rather than in every fixture.

use toni::{Error, ErrorKind};

#[derive(Debug)]
pub struct NotServed;

impl std::fmt::Display for NotServed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not part of this test")
    }
}

impl std::error::Error for NotServed {}

impl Error for NotServed {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Unimplemented
    }
}
