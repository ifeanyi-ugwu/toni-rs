//! P02a: "a Result output is fallible" as two blanket impls over `T` and `Result<T, E>`.
//! Expected: E0119.
pub trait FactoryOutput { type Value; fn into_result(self) -> Result<Self::Value, String>; }
impl<T> FactoryOutput for T { type Value = T; fn into_result(self) -> Result<T, String> { Ok(self) } }
impl<T, E: ToString> FactoryOutput for Result<T, E> {
    type Value = T;
    fn into_result(self) -> Result<T, String> { self.map_err(|e| e.to_string()) }
}
fn main() {}
