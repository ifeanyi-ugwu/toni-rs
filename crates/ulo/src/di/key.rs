//! What a key to the container is.

/// A key to the container: a type naming a slot, and what the slot holds.
///
/// A type is its own key with nothing to write: `#[inject] db: Db` reads `Db`'s slot, and a
/// declaration with no key binds under the type it builds. `Key` is what a position that reads a
/// slot's `Value` through a name needs, such as `provide!(K => ..)` and the `_key` lookups. A
/// marker type implementing it names a second slot for a type, or a slot for a role:
///
/// ```ignore
/// key!(pub Replica: Db);                    // a second `Db`
/// key!(pub Auth: dyn Guard<HttpContext>);   // a guard, applied where a route names `Auth`
/// ```
///
/// `#[injectable]`, `#[controller]` and `#[websocket_gateway]` implement it for their type with
/// `Value = Self`, and `#[derive(Key)]` does for any other type of the crate. A foreign type is
/// named by a marker: `key!(pub Main: sqlx::PgPool)`.
///
/// A slot holding a trait object hands out `Arc<Value>`. `provide!` binds any other slot to hand
/// out its value as the declaration would under its own type: a clone of a shared instance, a
/// fresh one for a transient.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a key",
    label = "a key position reads a type implementing `Key`",
    note = "a foreign type is named by a marker, `key!(pub Name: {Self})`; a type of this crate takes `#[derive(Key)]`"
)]
pub trait Key: 'static {
    /// What the slot holds.
    type Value: ?Sized + 'static;
}

/// Declares a marker type implementing [`Key`](crate::di::Key).
///
/// ```ignore
/// key!(pub Replica: Db);
/// // is
/// pub struct Replica;
/// impl Key for Replica { type Value = Db; }
/// ```
///
/// Attributes, doc comments included, go on the marker.
#[macro_export]
macro_rules! key {
    ($(#[$meta:meta])* $vis:vis $name:ident : $value:ty) => {
        $(#[$meta])*
        $vis struct $name;

        impl $crate::di::Key for $name {
            type Value = $value;
        }
    };
}
