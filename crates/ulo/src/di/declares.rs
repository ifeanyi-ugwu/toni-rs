//! What a type declares itself to be, so a module can name the type rather than the factory
//! generated beside it.
//!
//! `#[module(providers: [Db], controllers: [Orders])]` reaches each type's factory through
//! [`DeclaresProvider`] and [`DeclaresController`], as `DynamicModule::builder(..).provider::<Db>()`
//! and `.controller::<Orders>()` do.

use crate::di::provide::Declared;
use crate::dispatch::ControllerFactory;
use crate::spi::ProviderFactory;

/// A type with a provider declaration of its own: what a bare path in `providers:` or in
/// `provide!` registers.
///
/// `#[injectable]` and `#[websocket_gateway]` implement it, and so does `Extension<T>`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no provider declaration of its own",
    label = "a bare path here is a type's own declaration",
    note = "mark the type `#[injectable]`; in `provide!`, a value held in a const is written `value {Self}`"
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

/// A type `#[controller]` generated a controller factory for. See [`DeclaresProvider`].
pub trait DeclaresController {
    /// The factory that builds this target, and reports what it must be built after.
    fn controller_factory() -> impl ControllerFactory + 'static;
}
