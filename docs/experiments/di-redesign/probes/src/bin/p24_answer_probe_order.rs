//! P24 (transports §2.3): the `AnswerProbe` arms by autoref. With `(&&&Probe(out)).answer()`,
//! lookup tries the impl on `&&Probe` first, then `&Probe`, then `Probe`: at each autoderef step
//! the by-value candidate is the impl one reference below the step's type, which is what P02c
//! shows with one reference. A `Classified` error also satisfies `Into<BoxError>`, so the
//! `Classified` arm goes on `&&Probe<Result<V, E>>`, the `Into<BoxError>` arm on `&Probe<..>` and
//! the `Answer` arm on `Probe<V>`; P24b places them the other way round. An alias of `Result` is
//! resolved before lookup. The value sits in a `Cell<Option<_>>` because the ranked methods take
//! `&self`, as `__private::factory::Probe` already does. Expected: compiles, prints
//! "classified: not_found / boxed: io / value: Json".
use std::cell::Cell;
use std::error::Error;
use std::fmt;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

pub trait Answer {
    fn render(self) -> String;
}
pub struct Json(pub &'static str);
impl Answer for Json {
    fn render(self) -> String {
        format!("value: {}", self.0)
    }
}

pub trait Classified: Error + Send + Sync + 'static {
    fn kind(&self) -> &'static str;
}

#[derive(Debug)]
pub struct ApiError;
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("user not found")
    }
}
impl Error for ApiError {}
impl Classified for ApiError {
    fn kind(&self) -> &'static str {
        "not_found"
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

pub struct Probe<X>(pub Cell<Option<X>>);
impl<X> Probe<X> {
    pub fn new(x: X) -> Self {
        Probe(Cell::new(Some(x)))
    }
}

/// Arm one, highest priority: on `&&Probe`.
pub trait ViaClassified {
    fn answer(&self) -> String;
}
impl<V: Answer, E: Classified> ViaClassified for &&Probe<Result<V, E>> {
    fn answer(&self) -> String {
        match self.0.take().unwrap() {
            Ok(v) => v.render(),
            Err(e) => format!("classified: {}", e.kind()),
        }
    }
}

/// Arm two: on `&Probe`.
pub trait ViaBoxed {
    fn answer(&self) -> String;
}
impl<V: Answer, E: Into<BoxError>> ViaBoxed for &Probe<Result<V, E>> {
    fn answer(&self) -> String {
        match self.0.take().unwrap() {
            Ok(v) => v.render(),
            Err(e) => {
                let boxed: BoxError = e.into();
                format!("boxed: {}", boxed)
            }
        }
    }
}

/// Arm three, lowest priority: on `Probe`.
pub trait ViaValue {
    fn answer(&self) -> String;
}
impl<V: Answer> ViaValue for Probe<V> {
    fn answer(&self) -> String {
        self.0.take().unwrap().render()
    }
}

fn classified() -> ApiResult<Json> {
    Err(ApiError)
}
fn boxed() -> Result<Json, std::io::Error> {
    Err(std::io::Error::other("io"))
}
fn plain() -> Json {
    Json("Json")
}

fn main() {
    // Whatever the return type is spelled as: the alias takes the `Classified` arm.
    println!("{}", (&&&Probe::new(classified())).answer());
    println!("{}", (&&&Probe::new(boxed())).answer());
    println!("{}", (&&&Probe::new(plain())).answer());
}
