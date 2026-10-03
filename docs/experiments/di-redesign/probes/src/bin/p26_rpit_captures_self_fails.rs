//! P26 (transports §3.1, §4.1, §5.1): the examples write `fn events(&self) -> Sse<impl Stream>`
//! and `fn watch(&self) -> impl Stream`. On edition 2024 a return-position `impl Trait` captures
//! every in-scope lifetime, `&self`'s included, whatever the hidden type borrows, so the reply
//! cannot be held as `'static` once the handler returns and the transport writes it. `Iterator`
//! stands in for `Stream`; the capture rule is the same. Expected: an error at the `Box<dyn
//! Iterator + 'static>` ("borrowed data escapes" / "lifetime may not live long enough");
//! P26b adds `+ use<>`.
pub struct Sse<S>(pub S);

pub struct Ctl {
    pub from: u32,
}

impl Ctl {
    pub fn events(&self) -> Sse<impl Iterator<Item = u32>> {
        let from = self.from;
        Sse(from..from + 3)
    }
}

fn hold(ctl: &Ctl) -> Box<dyn Iterator<Item = u32> + Send + 'static> {
    Box::new(ctl.events().0)
}

fn main() {
    let ctl = Ctl { from: 1 };
    println!("{:?}", hold(&ctl).collect::<Vec<_>>());
}
