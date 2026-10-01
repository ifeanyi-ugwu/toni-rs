//! Support for the code `ulo-macros` generates. Not part of the public API: names and shapes here
//! change with the macros.

pub use ulo_macros::{__enhancer_specs, __handler};

use crate::construct::ConstructError;
use crate::scope::{AllowedIn, Scope};
use crate::site::Site;
use crate::timer::BoxError;
use crate::transport::{ErrorHandler, Guard, Interceptor, Transport};

/// Emitted once per field or parameter with `quote_spanned!`, so a site that is not a `Site`, or
/// that needs an execution in an explicit singleton, fails on that field or parameter.
pub fn assert_site<S: Site + AllowedIn<Sc>, Sc: Scope>() {}

/// Emitted once per handler and enhancer, so an enhancer lacking the role for a handler's
/// transport fails naming the handler (the function's name carries it).
pub fn assert_guard<G: Guard<T>, T: Transport>() {}
pub fn assert_interceptor<I: Interceptor<T>, T: Transport>() {}
pub fn assert_error_handler<E: ErrorHandler<T>, T: Transport>() {}

/// What a constructor returns, `Self` or `Result<Self, E>`, decided by type rather than by the
/// spelling of the return type, so an alias for a `Result` is read as one.
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
pub mod factory {
    use std::cell::Cell;
    use std::marker::PhantomData;

    use crate::binding::factory::Factory;
    use crate::module::def::ModuleDef;
    use crate::timer::BoxError;

    pub struct Probe<F, Args> {
        factory: Cell<Option<F>>,
        _a: PhantomData<fn() -> Args>,
    }

    impl<F, Args> Probe<F, Args> {
        pub fn new(factory: F) -> Self {
            Probe { factory: Cell::new(Some(factory)), _a: PhantomData }
        }
    }

    pub trait Fallible {
        fn register_singleton(&self, m: &mut ModuleDef<'_>);
    }
    impl<F, Args, T, E> Fallible for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        fn register_singleton(&self, m: &mut ModuleDef<'_>) {
            todo!()
        }
    }

    pub trait Plain {
        fn register_singleton(&self, m: &mut ModuleDef<'_>);
    }
    impl<F, Args> Plain for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        fn register_singleton(&self, m: &mut ModuleDef<'_>) {
            todo!()
        }
    }
}
