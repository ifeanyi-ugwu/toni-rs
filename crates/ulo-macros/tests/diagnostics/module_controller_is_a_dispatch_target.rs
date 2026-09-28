// A bare path in `controllers:` is a dispatch target's own declaration. A provider written there is
// refused, sending it to `providers:`.

#[ulo::injectable]
pub struct Service {}

#[ulo::module(controllers: [Service])]
pub struct AppModule;

fn main() {}
