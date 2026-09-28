//! What a type declares itself to be, so a module built at runtime can name the type rather than
//! the factory generated beside it.
//!
//! `#[module(providers: [Db], controllers: [Orders])]` names types, and the macro rewrites each to
//! the factory its attribute generated. A builder method has no macro to do that rewriting, so
//! these traits carry it: `DynamicModule::builder(..).provider_type::<Db>()` reaches the same
//! factory `providers: [Db]` does.

use crate::di::provide::Declared;
use crate::dispatch::ControllerFactory;
use crate::spi::ProviderFactory;

/// A type `#[injectable]` generated a provider factory for.
///
/// Implemented by the attribute, never by hand — what it answers with is what `providers:` uses.
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
