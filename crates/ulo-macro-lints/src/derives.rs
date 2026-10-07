//! `#[derive(Classify)]` on a struct and on an enum's variants, and `#[derive(Validate)]`.

use std::error::Error;
use std::fmt;

use serde::Deserialize;
use ulo_transport::{Classify, Validate};

/// A handler's own failure, one kind for the type.
#[derive(Debug, Classify)]
#[classify(conflict)]
pub struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refusal {}

/// A kind per variant.
#[derive(Debug, Classify)]
pub enum Lookup {
    #[classify(not_found)]
    Missing,
    #[classify(bad_request)]
    Malformed { field: String },
    #[classify(unavailable)]
    Down(u16),
}

impl fmt::Display for Lookup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lookup::Missing => f.write_str("missing"),
            Lookup::Malformed { field } => write!(f, "malformed {field}"),
            Lookup::Down(code) => write!(f, "down with {code}"),
        }
    }
}

impl Error for Lookup {}

/// A validated payload.
#[derive(Debug, Deserialize, Validate)]
pub struct Item {
    #[validate(length(min = 1, max = 8))]
    pub name: String,
}
