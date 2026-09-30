//! P07: two `note =` entries in one `#[diagnostic::on_unimplemented]`, with `{Self}` inside a
//! note. Expected: E0277 printing both notes (or a warning that the second is ignored).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an injection site",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[injectable(default)]"
)]
pub trait Site: Sized + Send + 'static {}
pub struct PgPool;
fn assert_site<S: Site>() {}
fn main() { assert_site::<PgPool>(); }
