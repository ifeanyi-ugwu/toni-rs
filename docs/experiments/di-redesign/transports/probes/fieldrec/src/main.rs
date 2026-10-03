//! §3.2's field-recording deserializer: run `T::deserialize` against a `Deserializer` that
//! records what `T` asks for and returns before producing a value. A derived struct asks through
//! `deserialize_struct(name, fields, ..)`, where `fields` carries the serde names (renames
//! included); a tuple through `deserialize_tuple(len, ..)`; a scalar through `deserialize_u64`
//! and kin; a newtype through `deserialize_newtype_struct`, which recurses. Two shapes carry no
//! names: a map (`HashMap`, and a struct with a `#[serde(flatten)]` field, which derives as a map)
//! and a hand-written impl calling `deserialize_any`. Expected: the six lines in `main`.
use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

#[derive(Debug)]
enum Found {
    #[allow(dead_code)]
    Fields(&'static [&'static str]),
    Tuple(usize),
    Scalar,
    Map,
    Any,
    Other(String),
}

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Found {}
impl de::Error for Found {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Found::Other(msg.to_string())
    }
}

struct Rec;

macro_rules! scalar {
    ($($m:ident)*) => {$(
        fn $m<V: Visitor<'de>>(self, _v: V) -> Result<V::Value, Found> { Err(Found::Scalar) }
    )*};
}

impl<'de> Deserializer<'de> for Rec {
    type Error = Found;
    fn deserialize_any<V: Visitor<'de>>(self, _v: V) -> Result<V::Value, Found> {
        Err(Found::Any)
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _v: V,
    ) -> Result<V::Value, Found> {
        Err(Found::Fields(fields))
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, _v: V) -> Result<V::Value, Found> {
        Err(Found::Tuple(len))
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _n: &'static str, len: usize, _v: V) -> Result<V::Value, Found> {
        Err(Found::Tuple(len))
    }
    fn deserialize_map<V: Visitor<'de>>(self, _v: V) -> Result<V::Value, Found> {
        Err(Found::Map)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _n: &'static str, v: V) -> Result<V::Value, Found> {
        v.visit_newtype_struct(Rec)
    }
    scalar! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64 deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf deserialize_unit deserialize_identifier deserialize_ignored_any
        deserialize_option deserialize_seq
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(self, _n: &'static str, _v: V) -> Result<V::Value, Found> {
        Err(Found::Scalar)
    }
    fn deserialize_enum<V: Visitor<'de>>(self, _n: &'static str, _vs: &'static [&'static str], _v: V) -> Result<V::Value, Found> {
        Err(Found::Other("enum".into()))
    }
}

fn shape<'de, T: Deserialize<'de>>() -> Found {
    match T::deserialize(Rec) {
        Ok(_) => Found::Other("value".into()),
        Err(found) => found,
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Params {
    id: u64,
    #[serde(rename = "slug")]
    name: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Flat {
    #[serde(flatten)]
    inner: Params,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Id(u64);

fn main() {
    println!("struct:        {}", shape::<Params>());
    println!("tuple:         {}", shape::<(u64, String)>());
    println!("scalar:        {}", shape::<u64>());
    println!("newtype:       {}", shape::<Id>());
    println!("map:           {}", shape::<HashMap<String, String>>());
    println!("flatten:       {}", shape::<Flat>());
}
