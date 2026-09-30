//! P13: `Transport::Reply: Send` with no written `'static`, boxed into a `'static` future after
//! `intercept` returns. Compiles: `Transport: 'static` makes `<T as Transport>::Reply: 'static`
//! by the projection-outlives rule, so a reply can never borrow the context it was made in.
use std::future::Future;
use std::pin::Pin;
pub trait Transport: 'static { type Cx: Send; type Reply: Send; }
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub fn finish<T: Transport>(reply: T::Reply) -> Pin<Box<dyn Future<Output = T::Reply> + Send + 'static>> {
    Box::pin(async move { reply })
}
fn main() {}
