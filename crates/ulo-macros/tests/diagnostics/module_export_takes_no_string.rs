// `exports:` lists key types. A string is refused, pointing at `key!`.

#[ulo::module(exports: ["app.port"])]
pub struct AppModule;

fn main() {}
