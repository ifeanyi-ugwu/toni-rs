// A key is a type. A string on `provide!`'s key side is refused, pointing at `key!`.

fn main() {
    let _ = ulo::provide!("app.port" => 3000u16);
}
