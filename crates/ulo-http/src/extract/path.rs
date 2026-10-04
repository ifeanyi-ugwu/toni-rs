use std::error::Error;
use std::fmt;
use std::future::Future;
use std::ops::Deref;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use ulo::Key;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::cx::HttpCx;
use crate::extract::de::{Source, from_pairs};
use crate::transport::Http;

/// The matched route's path parameters as `T`: a struct by field name (serde renames included), a
/// tuple by position, a scalar or newtype from the one parameter.
///
/// Checked against its route when the server prepares: `T`'s `Deserialize` impl runs against a
/// deserializer that records what it asks for, so a field the route does not name, a tuple of the
/// wrong length, or a scalar on a route with other than one parameter is a startup error rather
/// than the first request's. A map, a `#[serde(flatten)]` struct and a hand-written impl calling
/// `deserialize_any` ask for no names and are not checked.
///
/// A value that does not parse as its field's type is `ExtractError::Malformed` naming the struct
/// field, or `"path"` for a tuple or a scalar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Path<T> {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let Some(route) = cx.matched() else {
                return Err(ExtractError::Missing { param: "path" });
            };
            let pairs: Vec<(&str, &str)> = route.params.pairs.iter().map(|(name, value)| (&**name, value.as_str())).collect();
            from_pairs::<T>(&pairs, Source::Path).map(Path).map_err(|error| error.into_extract("path", cx))
        }
    }
}

impl<T: Validate> Validate for Path<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The startup check of one `Path<T>` parameter against its route, which a handler attribute
/// records on the handler value through `ulo_http::__private::PathProbe`.
#[derive(Clone, Copy)]
pub struct PathCheck {
    pub(crate) check: fn(&[&str]) -> Result<(), String>,
    pub(crate) ty: Key,
}

impl PathCheck {
    /// The check of `T`: its `Deserialize` impl against a recording deserializer, the route's
    /// parameter names given.
    pub fn of<T: DeserializeOwned + 'static>() -> PathCheck {
        PathCheck { check: check_path::<T>, ty: Key::of::<T, ()>() }
    }
}

/// Runs `T`'s `Deserialize` impl against a deserializer that records the names, the tuple length
/// or the single value it asks for, and compares them with `params`, the route's parameter names.
///
/// A struct must name exactly the route's parameters, as a tuple must have exactly their number.
/// A newtype is checked as what it wraps, so `UserId(u64)` takes one parameter.
pub(crate) fn check_path<T: DeserializeOwned>(params: &[&str]) -> Result<(), String> {
    let asked = match T::deserialize(Recorder) {
        Ok(_) => return Ok(()),
        Err(Recorded(asked)) => asked,
    };
    match asked {
        Asked::Struct(fields) => {
            let absent: Vec<&str> = fields.iter().copied().filter(|field| !params.contains(field)).collect();
            let unnamed: Vec<&str> = params.iter().copied().filter(|param| !fields.iter().any(|field| field == param)).collect();
            let mut problems = Vec::new();
            if !absent.is_empty() {
                problems.push(format!("the route has no parameter {}", listed(&absent)));
            }
            if !unnamed.is_empty() {
                problems.push(format!("the struct has no field for {}", listed(&unnamed)));
            }
            if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
        }
        Asked::Tuple(len) if len != params.len() => Err(format!("a tuple of {len} on a route with {} parameters", params.len())),
        Asked::Single if params.len() != 1 => Err(format!("a single value on a route with {} parameters", params.len())),
        _ => Ok(()),
    }
}

fn listed(names: &[&str]) -> String {
    names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ")
}

/// What a `Deserialize` impl asked the recorder for.
#[derive(Debug)]
enum Asked {
    Struct(&'static [&'static str]),
    Tuple(usize),
    Single,
    /// A map, a sequence, `deserialize_any`, or anything else that names nothing to check.
    Unchecked,
}

/// The recorder's error, which stops the impl at its first request and carries what it asked.
#[derive(Debug)]
struct Recorded(Asked);

impl fmt::Display for Recorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "asked for {:?}", self.0)
    }
}

impl Error for Recorded {}

impl de::Error for Recorded {
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Recorded(Asked::Unchecked)
    }
}

struct Recorder;

macro_rules! record {
    ($asked:expr => $($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Recorded> {
            Err(Recorded($asked))
        }
    )*};
}

impl<'de> Deserializer<'de> for Recorder {
    type Error = Recorded;

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Recorded> {
        Err(Recorded(Asked::Struct(fields)))
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, _visitor: V) -> Result<V::Value, Recorded> {
        Err(Recorded(Asked::Tuple(len)))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, len: usize, _visitor: V) -> Result<V::Value, Recorded> {
        Err(Recorded(Asked::Tuple(len)))
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Recorded> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Recorded> {
        visitor.visit_some(self)
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, _visitor: V) -> Result<V::Value, Recorded> {
        Err(Recorded(Asked::Unchecked))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Recorded> {
        Err(Recorded(Asked::Single))
    }

    record! { Asked::Single =>
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64 deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf deserialize_identifier
    }

    record! { Asked::Unchecked =>
        deserialize_any deserialize_map deserialize_seq deserialize_unit deserialize_ignored_any
    }
}
