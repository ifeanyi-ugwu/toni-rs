//! What a type declares itself to be, so a module can name the type rather than the factory
//! generated beside it.
//!
//! `#[module(providers: [Db], controllers: [Orders])]` reaches each type's factory through
//! [`DeclaresProvider`] and [`DeclaresController`], and a builder passes the same values:
//! `DynamicModule::builder(..).provider(Db::provide())`.

use crate::di::provide::Declared;
use crate::dispatch::ControllerFactory;
use crate::spi::ProviderFactory;

/// A type with a provider declaration of its own: what a bare path in `providers:` or in
/// `provide!` registers.
///
/// `#[injectable]` and `#[websocket_gateway]` implement it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no provider declaration of its own",
    label = "a bare path here is a type's own declaration",
    note = "mark the type `#[injectable]`, or put a `#[controller]` type in `controllers:`; in `provide!`, a value held in a const is written `value {Self}`"
)]
pub trait DeclaresProvider {
    /// The factory that builds this type, and reports what it must be built after.
    fn provider_factory() -> impl ProviderFactory + 'static;

    /// This type's own declaration, what `providers: [T]` registers, as a value the
    /// [`Declaration`](crate::di::Declaration) modifiers take: `T::provide().under_key::<K>()`.
    fn provide() -> Declared<Self>
    where
        Self: Sized,
    {
        Declared::new()
    }
}

/// A type `#[controller]` generated a controller factory for: what a bare path in `controllers:`
/// registers. See [`DeclaresProvider`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a dispatch target",
    label = "a dispatch target is a type marked `#[controller]`",
    note = "a provider, a gateway included, goes in `providers:` or `.provider(..)`"
)]
pub trait DeclaresController {
    /// The factory that builds this target, and reports what it must be built after.
    fn controller_factory() -> impl ControllerFactory + 'static;
}
