//! The deserializer behind `Path<T>`, `Query<T>` and `Form<T>`: a list of name and value pairs,
//! each value text parsed as the field's type asks.
//!
//! Serde names a struct's fields with `&'static str`, which is what lets a failure carry the
//! field's name as `ExtractError`'s `param`: a missing field is `Missing { param: "page" }`, a value
//! that does not parse is `Malformed { param: "page", .. }`.

use std::error::Error;
use std::fmt;

use percent_encoding::percent_decode;
use serde::de::{self, DeserializeOwned, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use ulo_transport::ExtractError;

use crate::cx::HttpCx;

/// Where the pairs come from, which decides what a top level other than a struct or a map means.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// A route's parameters: a tuple takes them in order and a scalar takes the only one.
    Path,
    /// A query string or a form body: a sequence takes `(name, value)` pairs.
    Form,
}

/// `T` from `pairs`.
pub(crate) fn from_pairs<T: DeserializeOwned>(pairs: &[(&str, &str)], source: Source) -> Result<T, ParamsError> {
    T::deserialize(Pairs { pairs, source })
}

/// `application/x-www-form-urlencoded` parsing as the WHATWG URL standard defines it: split at
/// `&`, empty pieces skipped, each split at its first `=`, `+` read as a space, then
/// percent-decoded and read as UTF-8 with U+FFFD in place of an invalid sequence.
pub(crate) fn parse_form(input: &[u8]) -> Vec<(String, String)> {
    input
        .split(|&byte| byte == b'&')
        .filter(|piece| !piece.is_empty())
        .map(|piece| match piece.iter().position(|&byte| byte == b'=') {
            Some(at) => (form_decode(&piece[..at]), form_decode(&piece[at + 1..])),
            None => (form_decode(piece), String::new()),
        })
        .collect()
}

fn form_decode(bytes: &[u8]) -> String {
    let spaced: Vec<u8> = bytes.iter().map(|&byte| if byte == b'+' { b' ' } else { byte }).collect();
    percent_decode(&spaced).decode_utf8_lossy().into_owned()
}

/// A failure deserializing the pairs, naming the field when serde names one.
#[derive(Debug)]
pub(crate) struct ParamsError {
    field: Option<&'static str>,
    missing: bool,
    message: String,
}

impl ParamsError {
    fn new(message: impl fmt::Display) -> Self {
        ParamsError { field: None, missing: false, message: message.to_string() }
    }

    /// Names `field` unless a deeper failure already named one.
    fn about(mut self, field: Option<&'static str>) -> Self {
        if self.field.is_none() {
            self.field = field;
        }
        self
    }

    /// The extraction failure: `Missing` for a field serde reported missing, `Malformed` otherwise,
    /// the error redacted through the app. `whole` names the parameter when no field is named.
    pub(crate) fn into_extract(self, whole: &'static str, cx: &HttpCx) -> ExtractError {
        let param = self.field.unwrap_or(whole);
        if self.missing {
            ExtractError::Missing { param }
        } else {
            ExtractError::Malformed { param, source: cx.app().redact(Box::new(self)) }
        }
    }
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ParamsError {}

impl de::Error for ParamsError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        ParamsError::new(message)
    }

    fn missing_field(field: &'static str) -> Self {
        ParamsError { field: Some(field), missing: true, message: format!("missing field `{field}`") }
    }

    fn duplicate_field(field: &'static str) -> Self {
        ParamsError { field: Some(field), missing: false, message: format!("`{field}` is given more than once") }
    }
}

struct Pairs<'a> {
    pairs: &'a [(&'a str, &'a str)],
    source: Source,
}

impl<'a> Pairs<'a> {
    /// The one value a scalar or newtype takes: a route's only parameter.
    fn single(&self) -> Result<Value<'a>, ParamsError> {
        match (self.source, self.pairs) {
            (Source::Path, &[(_, text)]) => Ok(Value { text }),
            (Source::Path, pairs) => Err(ParamsError::new(format!("a single value on a route with {} parameters", pairs.len()))),
            (Source::Form, _) => Err(ParamsError::new("a query or form deserializes into named fields, not a single value")),
        }
    }
}

macro_rules! single_value {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
            self.single()?.$method(visitor)
        }
    )*};
}

impl<'de, 'a> Deserializer<'de> for Pairs<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        match (self.source, self.pairs) {
            (Source::Path, &[(_, text)]) => Value { text }.deserialize_any(visitor),
            _ => self.deserialize_map(visitor),
        }
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_map(Entries { iter: self.pairs.iter(), fields: &[], value: None })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        visitor.visit_map(Entries { iter: self.pairs.iter(), fields, value: None })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        match self.source {
            Source::Path => visitor.visit_seq(Values { iter: self.pairs.iter() }),
            Source::Form => visitor.visit_seq(PairSeq { iter: self.pairs.iter() }),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        if self.source == Source::Path && len != self.pairs.len() {
            return Err(ParamsError::new(format!("a tuple of {len} on a route with {} parameters", self.pairs.len())));
        }
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        self.single()?.deserialize_enum(name, variants, visitor)
    }

    single_value! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64 deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf deserialize_identifier
    }
}

/// One value's text, parsed as the visitor asks.
#[derive(Clone, Copy)]
struct Value<'a> {
    text: &'a str,
}

macro_rules! parse_value {
    ($($method:ident => $visit:ident: $ty:ty,)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
            match self.text.parse::<$ty>() {
                Ok(value) => visitor.$visit(value),
                Err(_) => Err(ParamsError::new(concat!("the value is not a valid `", stringify!($ty), "`"))),
            }
        }
    )*};
}

impl<'de, 'a> Deserializer<'de> for Value<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_str(self.text)
    }

    parse_value! {
        deserialize_bool => visit_bool: bool,
        deserialize_i8 => visit_i8: i8,
        deserialize_i16 => visit_i16: i16,
        deserialize_i32 => visit_i32: i32,
        deserialize_i64 => visit_i64: i64,
        deserialize_i128 => visit_i128: i128,
        deserialize_u8 => visit_u8: u8,
        deserialize_u16 => visit_u16: u16,
        deserialize_u32 => visit_u32: u32,
        deserialize_u64 => visit_u64: u64,
        deserialize_u128 => visit_u128: u128,
        deserialize_f32 => visit_f32: f32,
        deserialize_f64 => visit_f64: f64,
        deserialize_char => visit_char: char,
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        IntoDeserializer::<'de, ParamsError>::into_deserializer(self.text).deserialize_enum(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        str string bytes byte_buf identifier unit_struct seq tuple tuple_struct map struct ignored_any
    }
}

/// A struct's or a map's entries; `fields` is the struct's field names, empty for a map.
struct Entries<'a> {
    iter: std::slice::Iter<'a, (&'a str, &'a str)>,
    fields: &'static [&'static str],
    /// The value of the key just read, with the field name serde gave for it.
    value: Option<(&'a str, Option<&'static str>)>,
}

impl<'de, 'a> MapAccess<'de> for Entries<'a> {
    type Error = ParamsError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, ParamsError> {
        let Some(&(key, text)) = self.iter.next() else {
            return Ok(None);
        };
        let field = self.fields.iter().copied().find(|field| *field == key);
        self.value = Some((text, field));
        seed.deserialize(IntoDeserializer::<'de, ParamsError>::into_deserializer(key)).map(Some).map_err(|error| error.about(field))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, ParamsError> {
        let Some((text, field)) = self.value.take() else {
            return Err(ParamsError::new("a value was asked for before its key"));
        };
        seed.deserialize(Value { text }).map_err(|error| error.about(field))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

/// A route's parameter values, in the order the pattern names them.
struct Values<'a> {
    iter: std::slice::Iter<'a, (&'a str, &'a str)>,
}

impl<'de, 'a> SeqAccess<'de> for Values<'a> {
    type Error = ParamsError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, ParamsError> {
        match self.iter.next() {
            Some(&(_, text)) => seed.deserialize(Value { text }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

/// A query's or a form's pairs as a sequence, each a `(name, value)` tuple.
struct PairSeq<'a> {
    iter: std::slice::Iter<'a, (&'a str, &'a str)>,
}

impl<'de, 'a> SeqAccess<'de> for PairSeq<'a> {
    type Error = ParamsError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, ParamsError> {
        match self.iter.next() {
            Some(&(name, text)) => seed.deserialize(Pair { name, text }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct Pair<'a> {
    name: &'a str,
    text: &'a str,
}

impl<'de, 'a> Deserializer<'de> for Pair<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        let texts = [("", self.name), ("", self.text)];
        visitor.visit_seq(Values { iter: texts.iter() })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any
    }
}
