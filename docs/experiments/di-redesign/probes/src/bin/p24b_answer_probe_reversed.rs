//! P24b (§2.3): the three arms placed in the order §2.3 lists them, `Classified` on `Probe`,
//! `Into<BoxError>` on `&Probe`, `Answer` on `&&Probe`, with the same `(&&&Probe(out)).answer()`
//! call. It compiles, and a `Classified` error takes the `Into<BoxError>` arm, since lookup reaches
//! `&Probe` before `Probe` and every `Classified` error converts into a `BoxError`. The kind is
//! lost before the error handlers see it. Expected: compiles, prints "boxed: user not found"
//! where P24 prints "classified: not_found".
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

pub struct Probe<X>(pub Cell<Option<X>>);
impl<X> Probe<X> {
    pub fn new(x: X) -> Self {
        Probe(Cell::new(Some(x)))
    }
}

pub trait ViaClassified {
    fn answer(&self) -> String;
}
impl<V: Answer, E: Classified> ViaClassified for Probe<Result<V, E>> {
    fn answer(&self) -> String {
        match self.0.take().unwrap() {
            Ok(v) => v.render(),
            Err(e) => format!("classified: {}", e.kind()),
        }
    }
}

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

pub trait ViaValue {
    fn answer(&self) -> String;
}
impl<V: Answer> ViaValue for &&Probe<V> {
    fn answer(&self) -> String {
        self.0.take().unwrap().render()
    }
}

fn main() {
    let out: Result<Json, ApiError> = Err(ApiError);
    println!("{}", (&&&Probe::new(out)).answer());
}
