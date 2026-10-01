//! Support for the code `ulo-macros` generates. Not part of the public API: names and shapes here
//! change with the macros.

pub use ulo_macros::{__enhancer_specs, __handler};

use crate::construct::ConstructError;
use crate::dependency::{Dependencies, FromContainer};
use crate::scope::{AllowedIn, Scope};
use crate::timer::BoxError;

/// A struct field, declared under its name. The bounds are the per-field assertion:
/// `#[injectable]` emits one call per field with `quote_spanned!`, so a field that is not
/// `FromContainer`, or that needs an execution in an explicit singleton, fails on that field.
/// Declaring and asserting in one call keeps a bad field to one error here rather than two.
pub fn field<S: FromContainer + AllowedIn<Sc>, Sc: Scope>(d: &mut Dependencies, name: &'static str) {
    d.field::<S>(name);
}

/// A constructor parameter, declared under its name, with the same assertion as [`field`].
pub fn param<S: FromContainer + AllowedIn<Sc>, Sc: Scope>(d: &mut Dependencies, name: &'static str) {
    d.param::<S>(name);
}

/// What a constructor returns, `Self` or `Result<Self, E>`, decided by type rather than by the
/// spelling of the return type, so an alias for a `Result` is read as one.
#[diagnostic::on_unimplemented(
    message = "a constructor returns `{T}` or `Result<{T}, E>`, not `{Self}`",
    label = "returned by this constructor",
    note = "the error type `E` must convert into `BoxError`, as any `std::error::Error + Send + Sync` does"
)]
pub trait IntoConstructed<T> {
    fn into_constructed(self) -> Result<T, ConstructError>;
}

impl<T> IntoConstructed<T> for T {
    fn into_constructed(self) -> Result<T, ConstructError> {
        Ok(self)
    }
}

impl<T, E: Into<BoxError>> IntoConstructed<T> for Result<T, E> {
    fn into_constructed(self) -> Result<T, ConstructError> {
        self.map_err(ConstructError::failed)
    }
}

/// The autoref probes behind `hooks!` and `#[injectable]`'s generated `Construct::hooks`:
/// `(&Probe::of(h)).register_init(h)` reaches `RegisterInit` when the concrete type implements
/// `OnModuleInit` and falls back, one autoref further, to the no-op `NoInit` otherwise.
pub mod hooks {
    use std::marker::PhantomData;

    use crate::hooks::{
        BeforeApplicationShutdown, Hooks, OnApplicationBootstrap, OnApplicationShutdown, OnModuleDestroy,
        OnModuleInit,
    };

    pub struct Probe<T>(PhantomData<fn() -> T>);

    impl<T> Probe<T> {
        pub fn of(_hooks: &Hooks<T>) -> Self {
            Probe(PhantomData)
        }
    }

    pub trait RegisterInit<T> {
        fn register_init(&self, h: &mut Hooks<T>);
    }
    impl<T: OnModuleInit> RegisterInit<T> for Probe<T> {
        fn register_init(&self, h: &mut Hooks<T>) {
            h.on_module_init();
        }
    }
    pub trait NoInit<T> {
        fn register_init(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoInit<T> for &Probe<T> {}

    pub trait RegisterBootstrap<T> {
        fn register_bootstrap(&self, h: &mut Hooks<T>);
    }
    impl<T: OnApplicationBootstrap> RegisterBootstrap<T> for Probe<T> {
        fn register_bootstrap(&self, h: &mut Hooks<T>) {
            h.on_application_bootstrap();
        }
    }
    pub trait NoBootstrap<T> {
        fn register_bootstrap(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoBootstrap<T> for &Probe<T> {}

    pub trait RegisterDestroy<T> {
        fn register_destroy(&self, h: &mut Hooks<T>);
    }
    impl<T: OnModuleDestroy> RegisterDestroy<T> for Probe<T> {
        fn register_destroy(&self, h: &mut Hooks<T>) {
            h.on_module_destroy();
        }
    }
    pub trait NoDestroy<T> {
        fn register_destroy(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoDestroy<T> for &Probe<T> {}

    pub trait RegisterBeforeShutdown<T> {
        fn register_before_shutdown(&self, h: &mut Hooks<T>);
    }
    impl<T: BeforeApplicationShutdown> RegisterBeforeShutdown<T> for Probe<T> {
        fn register_before_shutdown(&self, h: &mut Hooks<T>) {
            h.before_application_shutdown();
        }
    }
    pub trait NoBeforeShutdown<T> {
        fn register_before_shutdown(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoBeforeShutdown<T> for &Probe<T> {}

    pub trait RegisterShutdown<T> {
        fn register_shutdown(&self, h: &mut Hooks<T>);
    }
    impl<T: OnApplicationShutdown> RegisterShutdown<T> for Probe<T> {
        fn register_shutdown(&self, h: &mut Hooks<T>) {
            h.on_application_shutdown();
        }
    }
    pub trait NoShutdown<T> {
        fn register_shutdown(&self, _h: &mut Hooks<T>) {}
    }
    impl<T> NoShutdown<T> for &Probe<T> {}
}

/// The autoref probe `#[module]` writes at a factory's call site in a providers list: a closure
/// whose future outputs a `Result` reaches the `try_` registration, any other the plain one. The
/// value API keeps two methods because one method cannot serve both outputs.
///
/// `Args` is a parameter of `Probe` rather than of the methods: method probing checks an impl's
/// where-clauses, which is what lets the ranking fall through, and it never checks a method's.
pub mod factory {
    use std::cell::Cell;
    use std::marker::PhantomData;

    use crate::binding::factory::Factory;
    use crate::module::def::ModuleDef;
    use crate::timer::BoxError;

    /// The factory sits in a `Cell` because the probe methods take `&self`, which the autoref
    /// ranking needs, and registration moves the factory into the binding.
    pub struct Probe<F, Args> {
        factory: Cell<Option<F>>,
        _a: PhantomData<fn() -> Args>,
    }

    impl<F, Args> Probe<F, Args> {
        pub fn new(factory: F) -> Self {
            Probe { factory: Cell::new(Some(factory)), _a: PhantomData }
        }
    }

    /// `#[track_caller]` on both traits carries the providers entry's location through to the
    /// binding record, which the wiring errors print.
    pub trait Fallible {
        #[track_caller]
        fn register_singleton(&self, m: &mut ModuleDef<'_>);
    }
    impl<F, Args, T, E> Fallible for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        #[track_caller]
        fn register_singleton(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                m.try_singleton::<Args, F, T, E>(factory);
            }
        }
    }

    pub trait Plain {
        #[track_caller]
        fn register_singleton(&self, m: &mut ModuleDef<'_>);
    }
    impl<F, Args> Plain for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        #[track_caller]
        fn register_singleton(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                m.singleton::<Args, F>(factory);
            }
        }
    }
}
