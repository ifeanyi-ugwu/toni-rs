// A provider factory is always async. A sync closure is refused at the call, naming the two async
// spellings, rather than compiling into a provider of whatever the closure returns.

fn main() {
    let _ = ulo::di::Provide::factory(|| 3000u16);
}
