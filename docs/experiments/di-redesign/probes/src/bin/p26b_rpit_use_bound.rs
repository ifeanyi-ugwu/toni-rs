//! P26b (§3.1): P26 with `+ use<>` on the opaque type, which is what the handler attribute has to
//! append (or the user write) for a streaming answer from a `&self` handler on edition 2024.
//! Expected: compiles, prints "[1, 2, 3]".
pub struct Sse<S>(pub S);

pub struct Ctl {
    pub from: u32,
}

impl Ctl {
    pub fn events(&self) -> Sse<impl Iterator<Item = u32> + use<>> {
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
