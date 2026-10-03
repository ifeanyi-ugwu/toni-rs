//! Support for the code `ulo-macros` generates. Not part of the public API: names and shapes here
//! change with the macros.

pub use std::sync::Arc;

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

/// Whether `key` is one of `keys`, byte for byte: the assertion `#[routes]` emits for each
/// controller-level transport-scoped enhancer key against the `__ULO_KEY_<name>` constants its
/// handlers' transport attributes emit (transports DESIGN §2.1, X1, X11). A loop over bytes,
/// since a trait method such as `PartialEq::eq` is not callable in a `const fn` on stable.
pub const fn key_in(key: &str, keys: &[&str]) -> bool {
    let mut i = 0;
    while i < keys.len() {
        if bytes_eq(key.as_bytes(), keys[i].as_bytes()) {
            return true;
        }
        i += 1;
    }
    false
}

const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The impl-level `value` entries of one `#[routes]` impl, each built once by the generated
/// `Controller::mount` into an `Arc` and handed to every handler's mount function (transports
/// DESIGN §2.1, X2). `T` is a tuple `(Arc<V0>, Arc<V1>, ..)`, one element per `value` entry in the
/// order written across the impl's three enhancer attributes, entries scoped to other transports
/// included; `()` when the impl has none.
///
/// A handler's mount function takes `&Shared<(Arc<V0>, ..)>` with one type parameter per entry,
/// bounding by its transport's role only the entries that apply to it, and registers each with
/// `EnhancerSpec::{guard,interceptor,error_handler}_arc(Arc::clone(&shared.0.N))`.
pub struct Shared<T>(pub T);

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

/// The autoref probe `#[module]` writes at a factory's call site, for a closure entry of a
/// providers list and for a `with` entry of an `into` list: a closure whose future outputs a
/// `Result` reaches the `try_` registration, any other the plain one. The value API keeps two methods
/// because one method cannot serve both outputs.
///
/// `Args` is a parameter of `Probe` rather than of the methods: method probing checks an impl's
/// where-clauses, which is what lets the ranking fall through, and it never checks a method's.
pub mod factory {
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::sync::Arc;

    use crate::binding::contribute::Contribute;
    use crate::binding::factory::Factory;
    use crate::module::def::ModuleDef;
    use crate::scope::{Auto, PerExecution, Singleton, Transient};
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

    /// The providers arm: a single binding declared by closure, `with = ..`, `with(<scope>) = ..`
    /// or a bare closure, in the scope `S` (`Auto` for the first and the last). `bind_as` is the
    /// key-first form `K: with = ..`, which also binds the output under `K` through the coercion
    /// closure written at the expansion, where `Arc<Built>` and `Arc<K>` are concrete.
    ///
    /// `#[track_caller]` on every method carries the providers entry's location through to the
    /// binding record, which the wiring errors print.
    pub trait FallibleBinding {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>);

        #[track_caller]
        fn bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static;
    }
    impl<F, Args, T, E> FallibleBinding for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        type Built = T;

        #[track_caller]
        fn bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                S::try_bind::<Args, F, T, E>(m, factory);
            }
        }

        #[track_caller]
        fn bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::try_bind_as::<K, Args, F, T, E, C>(m, factory, coerce);
            }
        }
    }

    pub trait PlainBinding {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>);

        #[track_caller]
        fn bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static;
    }
    impl<F, Args> PlainBinding for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        type Built = <F as Factory<Args>>::Output;

        #[track_caller]
        fn bind<S: ProviderScope>(&self, m: &mut ModuleDef<'_>) {
            if let Some(factory) = self.factory.take() {
                S::bind::<Args, F>(m, factory);
            }
        }

        #[track_caller]
        fn bind_as<K, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            S: ProviderScope,
            C: Fn(Arc<Self::Built>) -> Arc<K> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::bind_as::<K, Args, F, C>(m, factory, coerce);
            }
        }
    }

    /// A scope marker as a providers entry's `with` names it, mapped to the `ModuleDef` method
    /// that registers a closure in that scope: `Auto` to `with`, `Singleton` to `singleton`,
    /// `PerExecution` to `execution`, `Transient` to `transient`, each with its `try_` twin.
    pub trait ProviderScope {
        #[track_caller]
        fn bind<Args, F>(m: &mut ModuleDef<'_>, factory: F)
        where
            F: Factory<Args>,
            F::Output: Send + Sync + 'static;

        #[track_caller]
        fn try_bind<Args, F, T, E>(m: &mut ModuleDef<'_>, factory: F)
        where
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static;

        #[track_caller]
        fn bind_as<K, Args, F, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            F: Factory<Args>,
            F::Output: Send + Sync + 'static,
            C: Fn(Arc<F::Output>) -> Arc<K> + Send + Sync + 'static;

        #[track_caller]
        fn try_bind_as<K, Args, F, T, E, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
        where
            K: ?Sized + Send + Sync + 'static,
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static,
            C: Fn(Arc<T>) -> Arc<K> + Send + Sync + 'static;
    }

    macro_rules! provider_scope {
        ($($marker:ty => $plain:ident, $fallible:ident;)*) => {$(
            impl ProviderScope for $marker {
                #[track_caller]
                fn bind<Args, F>(m: &mut ModuleDef<'_>, factory: F)
                where
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                {
                    m.$plain::<Args, F>(factory);
                }

                #[track_caller]
                fn try_bind<Args, F, T, E>(m: &mut ModuleDef<'_>, factory: F)
                where
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                {
                    m.$fallible::<Args, F, T, E>(factory);
                }

                #[track_caller]
                fn bind_as<K, Args, F, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
                where
                    K: ?Sized + Send + Sync + 'static,
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                    C: Fn(Arc<F::Output>) -> Arc<K> + Send + Sync + 'static,
                {
                    m.$plain::<Args, F>(factory).also_as::<K>(coerce);
                }

                #[track_caller]
                fn try_bind_as<K, Args, F, T, E, C>(m: &mut ModuleDef<'_>, factory: F, coerce: C)
                where
                    K: ?Sized + Send + Sync + 'static,
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                    C: Fn(Arc<T>) -> Arc<K> + Send + Sync + 'static,
                {
                    m.$fallible::<Args, F, T, E>(factory).also_as::<K>(coerce);
                }
            }
        )*};
    }

    provider_scope! {
        Auto => with, try_with;
        Singleton => singleton, try_singleton;
        PerExecution => execution, try_execution;
        Transient => transient, try_transient;
    }

    /// The `into` arm: a contribution to `U` declared by closure, in the scope `S`, which is
    /// `Auto` for `with = ..` and the written marker for `with(<scope>) = ..`. The coercion
    /// closure has to be written in the expansion, where `Arc<Built>` and `Arc<U>` are concrete
    /// types and the one unsizes to the other; a generic body cannot unsize. The expansion cannot
    /// name the built type, so `Built` is an associated type, resolved once the ranking has
    /// picked the arm.
    pub trait FallibleContribution {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static;
    }
    impl<F, Args, T, E> FallibleContribution for Probe<F, Args>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        type Built = T;

        #[track_caller]
        fn contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::try_contribute::<U, Args, F, T, E, C>(m.contribute::<U>(), factory, coerce);
            }
        }
    }

    pub trait PlainContribution {
        type Built: Send + Sync + 'static;

        #[track_caller]
        fn contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static;
    }
    impl<F, Args> PlainContribution for &Probe<F, Args>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        type Built = <F as Factory<Args>>::Output;

        #[track_caller]
        fn contribute_with<U, S, C>(&self, m: &mut ModuleDef<'_>, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            S: ContributionScope,
            C: Fn(Arc<Self::Built>) -> Arc<U> + Send + Sync + 'static,
        {
            if let Some(factory) = self.factory.take() {
                S::contribute::<U, Args, F, C>(m.contribute::<U>(), factory, coerce);
            }
        }
    }

    /// A scope marker as an `into` entry's `with` names it, mapped to the `Contribute` method
    /// that registers a closure in that scope: `Auto` to `with`, `Singleton` to `singleton`,
    /// `PerExecution` to `execution`, `Transient` to `transient`, each with its `try_` twin.
    pub trait ContributionScope {
        #[track_caller]
        fn contribute<U, Args, F, C>(into: Contribute<'_, U>, factory: F, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            F: Factory<Args>,
            F::Output: Send + Sync + 'static,
            C: Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static;

        #[track_caller]
        fn try_contribute<U, Args, F, T, E, C>(into: Contribute<'_, U>, factory: F, coerce: C)
        where
            U: ?Sized + Send + Sync + 'static,
            F: Factory<Args, Output = Result<T, E>>,
            T: Send + Sync + 'static,
            E: Into<BoxError> + Send + 'static,
            C: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static;
    }

    macro_rules! contribution_scope {
        ($($marker:ty => $plain:ident, $fallible:ident;)*) => {$(
            impl ContributionScope for $marker {
                #[track_caller]
                fn contribute<U, Args, F, C>(into: Contribute<'_, U>, factory: F, coerce: C)
                where
                    U: ?Sized + Send + Sync + 'static,
                    F: Factory<Args>,
                    F::Output: Send + Sync + 'static,
                    C: Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
                {
                    into.$plain::<Args, F>(factory, coerce);
                }

                #[track_caller]
                fn try_contribute<U, Args, F, T, E, C>(into: Contribute<'_, U>, factory: F, coerce: C)
                where
                    U: ?Sized + Send + Sync + 'static,
                    F: Factory<Args, Output = Result<T, E>>,
                    T: Send + Sync + 'static,
                    E: Into<BoxError> + Send + 'static,
                    C: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
                {
                    into.$fallible::<Args, F, T, E>(factory, coerce);
                }
            }
        )*};
    }

    contribution_scope! {
        Auto => with, try_with;
        Singleton => singleton, try_singleton;
        PerExecution => execution, try_execution;
        Transient => transient, try_transient;
    }
}
