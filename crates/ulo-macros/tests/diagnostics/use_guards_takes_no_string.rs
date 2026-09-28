// A key is a type. A string in `#[use_guards(..)]` is refused, naming the marker to declare instead.

#[ulo::controller("/api")]
pub struct Api;

#[ulo::routes]
impl Api {
    #[ulo::get("/")]
    #[use_guards("AUTH_GUARD")]
    fn index(&self) -> ulo::http::Body {
        ulo::http::Body::text("ok".to_string())
    }
}

fn main() {}
