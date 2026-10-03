//! The binding handle: one type whose state parameters name the item written last, whether its
//! bound is written, and the state of the binding item itself (§9.1).
//!
//! `Handle<'m, T, K, C>`:
//! - `K` is the item written last: the binding ([`Binding`] or [`Contribution`]), a readiness
//!   check ([`ReadyItem`]) or a closure hook ([`HookItem`]), each with its bound state.
//! - `C` is the binding item's state, carried across the other items so `also_as` and
//!   `qualified` return to the binding as it was left, and so the construction's bound is written
//!   once wherever the handle is on the binding. `C` also carries the scope, which decides
//!   whether hooks and a readiness check exist at all.
//!
//! `.timeout(..)` and `.unbounded()` write the bound of the item `K` names and move it to a state
//! that has neither, so a second bound on one item is E0599 at the second call. `.retries` and
//! `.backoff` exist on the readiness item in every state and are last-write-wins.

use std::convert::Infallible;
use std::marker::PhantomData;
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

use crate::binding::factory::{
    Factory, ShutdownFactory, erase_check, erase_destroy_hook, erase_init_hook, erase_signalled_hook,
};
use crate::binding::{AlsoAs, BindingRecord, Qualifier, ReadyRecord, coercion};
use crate::dependency::Dependencies;
use crate::hooks::{HookFn, HookKind, HookRecord};
use crate::key::Key;
use crate::scope::HookCapable;
use crate::timer::{BoxError, Bound};

/// A bound not yet written: `.timeout(..)` and `.unbounded()` exist.
pub enum Open {}

/// A bound written: neither `.timeout(..)` nor `.unbounded()` exists.
pub enum Set {}

/// The single binding as an item: scope `S`, construction bound `B`.
pub struct Binding<S, B>(Infallible, PhantomData<(S, B)>);

/// A collection contribution as an item: scope `S`, construction bound `B`. It has no
/// `also_as` or `qualified`: a contribution is not reachable under a key of its own.
pub struct Contribution<S, B>(Infallible, PhantomData<(S, B)>);

/// A readiness check as an item: the whole-check bound `W` and the one-attempt bound `A`.
pub struct ReadyItem<W, A>(Infallible, PhantomData<(W, A)>);

/// A closure hook as an item, with its bound `B`.
pub struct HookItem<B>(Infallible, PhantomData<B>);

mod sealed {
    use crate::binding::BindingRecord;
    use crate::timer::Bound;

    /// Which field of the record a state's bound methods write. The item a state names is in
    /// the record whenever the handle is in that state: the readiness item only follows
    /// `.ready(..)`, and the hook item only follows the push of its hook.
    pub trait State {
        fn write(record: &mut BindingRecord, bound: Bound);

        fn write_unbounded(record: &mut BindingRecord) {
            Self::write(record, Bound::Unbounded);
        }
    }
}

impl<S, B> sealed::State for Binding<S, B> {
    fn write(record: &mut BindingRecord, bound: Bound) {
        record.construct_bound = bound;
    }
}

impl<S, B> sealed::State for Contribution<S, B> {
    fn write(record: &mut BindingRecord, bound: Bound) {
        record.construct_bound = bound;
    }
}

impl<W, A> sealed::State for ReadyItem<W, A> {
    /// The whole-check bound. Writing it opts the attempt out of its `construct_timeout`
    /// default (§9.3), so an attempt bound not yet written becomes `Unbounded`; a later
    /// `.attempt_timeout(..)` still writes it.
    fn write(record: &mut BindingRecord, bound: Bound) {
        if let Some(ready) = record.ready.as_mut() {
            ready.whole = bound;
            if ready.attempt == Bound::Default {
                ready.attempt = Bound::Unbounded;
            }
        }
    }

    /// `.unbounded()` on a readiness check stands in for both bounds.
    fn write_unbounded(record: &mut BindingRecord) {
        if let Some(ready) = record.ready.as_mut() {
            ready.whole = Bound::Unbounded;
            ready.attempt = Bound::Unbounded;
        }
    }
}

impl<B> sealed::State for HookItem<B> {
    fn write(record: &mut BindingRecord, bound: Bound) {
        if let Some(hook) = record.hooks.last_mut() {
            hook.bound = bound;
        }
    }
}

/// `.timeout(..)` on an item whose bound is open: the item's next state, and the binding's.
pub trait Timeout<C>: sealed::State {
    type Next;
    type Construction;
}

impl<S> Timeout<Binding<S, Open>> for Binding<S, Open> {
    type Next = Binding<S, Set>;
    type Construction = Binding<S, Set>;
}

impl<S> Timeout<Contribution<S, Open>> for Contribution<S, Open> {
    type Next = Contribution<S, Set>;
    type Construction = Contribution<S, Set>;
}

impl<C> Timeout<C> for HookItem<Open> {
    type Next = HookItem<Set>;
    type Construction = C;
}

impl<A, C> Timeout<C> for ReadyItem<Open, A> {
    type Next = ReadyItem<Set, A>;
    type Construction = C;
}

/// `.unbounded()` on an item whose bound is open. On a readiness check it exists only while both
/// of its bounds are open, and it closes both.
pub trait Unbounded<C>: sealed::State {
    type Next;
    type Construction;
}

impl<S> Unbounded<Binding<S, Open>> for Binding<S, Open> {
    type Next = Binding<S, Set>;
    type Construction = Binding<S, Set>;
}

impl<S> Unbounded<Contribution<S, Open>> for Contribution<S, Open> {
    type Next = Contribution<S, Set>;
    type Construction = Contribution<S, Set>;
}

impl<C> Unbounded<C> for HookItem<Open> {
    type Next = HookItem<Set>;
    type Construction = C;
}

impl<C> Unbounded<C> for ReadyItem<Open, Open> {
    type Next = ReadyItem<Set, Set>;
    type Construction = C;
}

/// `.attempt_timeout(..)`: the readiness check's one-attempt bound, written once.
pub trait AttemptTimeout: sealed::State {
    type Next;
}

impl<W> AttemptTimeout for ReadyItem<W, Open> {
    type Next = ReadyItem<W, Set>;
}

/// The binding states `also_as` and `qualified` return to: a single binding.
pub trait SingleBinding: sealed::State {}

impl<S, B> SingleBinding for Binding<S, B> {}

/// The binding states whose handle carries closure hooks and a readiness check: a singleton or
/// an `Auto` binding. Hooks on an `Auto` binding that the wiring pass infers per-execution are
/// refused at `wire()`.
#[diagnostic::on_unimplemented(
    message = "closure hooks and readiness checks exist on singleton handles only",
    label = "this binding is execution-scoped or transient",
    note = "register the factory with `singleton(..)`, or move the hook to a singleton"
)]
pub trait HookHost: sealed::State {}

impl<S: HookCapable, B> HookHost for Binding<S, B> {}
impl<S: HookCapable, B> HookHost for Contribution<S, B> {}

/// What `provide`, `value`, the factory methods and the `contribute` builder return: a handle on
/// the binding registered by that call, through which its second keys, qualifier, construction bound,
/// readiness check and closure hooks are written.
pub struct Handle<'m, T: ?Sized, K, C = K> {
    record: &'m mut BindingRecord,
    _s: PhantomData<fn() -> (PhantomData<T>, K, C)>,
}

impl<'m, T: ?Sized, K, C> Handle<'m, T, K, C> {
    pub(crate) fn new(record: &'m mut BindingRecord) -> Self {
        Handle { record, _s: PhantomData }
    }

    fn to<K2, C2>(self) -> Handle<'m, T, K2, C2> {
        Handle { record: self.record, _s: PhantomData }
    }

    fn push_hook<K2>(
        self,
        kind: HookKind,
        run: HookFn,
        dependencies: Dependencies,
        location: &'static Location<'static>,
    ) -> Handle<'m, T, K2, C> {
        self.record.hooks.push(HookRecord { kind, bound: Bound::Default, run, dependencies, location });
        self.to()
    }
}

impl<'m, T: ?Sized + Send + Sync + 'static, K, C: SingleBinding> Handle<'m, T, K, C> {
    /// A second key under another type, reaching the same object: `.also_as::<dyn Cache>(|a| a)`.
    /// The closure's return is where `Arc<T>` unsizes to `Arc<U>`; a `T` that does not implement
    /// the trait fails to compile there.
    pub fn also_as<U: ?Sized + Send + Sync + 'static>(
        self,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, C, C> {
        self.record.also.push(AlsoAs { key: Key::of::<U, ()>(), coerce: coercion::<T, U, _>(coerce) });
        self.to()
    }

    /// Qualifies every key of the binding, its `also_as` keys included, with `Q`. A second call
    /// replaces the first qualifier.
    pub fn qualified<Q: 'static>(self) -> Handle<'m, T, C, C> {
        self.record.qualifier = Qualifier::of::<Q>();
        self.to()
    }
}

impl<'m, T: ?Sized, K, C: HookHost> Handle<'m, T, K, C> {
    /// A readiness check, run right after the binding is constructed and before anything that
    /// depends on it. A check that writes no bound takes `construct_timeout` as its attempt bound
    /// when a `Timer` exists (§9.3). One check per binding; a second is a wiring error.
    ///
    /// `.retries(..)` counts attempts after the first, zero unless written; `.backoff(..)` is the
    /// wait between attempts, zero unless written.
    #[track_caller]
    pub fn ready<Args, F, E>(self, check: F) -> Handle<'m, T, ReadyItem<Open, Open>, C>
    where
        F: Factory<Args, Output = Result<(), E>>,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        let ready = ReadyRecord {
            check: erase_check::<Args, F, E>(check),
            dependencies,
            retries: 0,
            backoff: Duration::ZERO,
            backoff_location: None,
            whole: Bound::Default,
            attempt: Bound::Default,
            location,
        };
        if let Some(replaced) = self.record.ready.replace(ready) {
            self.record.replaced_ready.push(replaced.location);
        }
        self.to()
    }

    /// An init hook, run once every singleton is built and checked, in connect order. Its `Err`
    /// fails `connect` as `ConnectError::Hook`.
    #[track_caller]
    pub fn on_init<Args, F, E>(self, hook: F) -> Handle<'m, T, HookItem<Open>, C>
    where
        F: Factory<Args, Output = Result<(), E>>,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        self.push_hook(HookKind::OnModuleInit, erase_init_hook::<Args, F, E>(hook), dependencies, location)
    }

    /// Runs after the drain, in reverse connect order.
    #[track_caller]
    pub fn on_destroy<Args, F>(self, hook: F) -> Handle<'m, T, HookItem<Open>, C>
    where
        F: Factory<Args, Output = ()>,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        self.push_hook(HookKind::OnModuleDestroy, erase_destroy_hook::<Args, F>(hook), dependencies, location)
    }

    /// Runs while the app still serves, before stop-accepting, with the shutdown's signal.
    #[track_caller]
    pub fn before_shutdown<Args, F>(self, hook: F) -> Handle<'m, T, HookItem<Open>, C>
    where
        F: ShutdownFactory<Args>,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as ShutdownFactory<Args>>::dependencies(&mut dependencies);
        self.push_hook(HookKind::BeforeApplicationShutdown, erase_signalled_hook::<Args, F>(hook), dependencies, location)
    }

    /// Runs last in the shutdown sequence, after the sockets close, with the shutdown's signal.
    #[track_caller]
    pub fn on_shutdown<Args, F>(self, hook: F) -> Handle<'m, T, HookItem<Open>, C>
    where
        F: ShutdownFactory<Args>,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as ShutdownFactory<Args>>::dependencies(&mut dependencies);
        self.push_hook(HookKind::OnApplicationShutdown, erase_signalled_hook::<Args, F>(hook), dependencies, location)
    }
}

impl<'m, T: ?Sized, K: Timeout<C>, C> Handle<'m, T, K, C> {
    /// Bounds the item written last: the construction directly after the binding is registered,
    /// the whole check after `.ready(..)`, the hook after an `on_*`. Needs a `Timer`.
    pub fn timeout(self, d: Duration) -> Handle<'m, T, K::Next, K::Construction> {
        <K as sealed::State>::write(&mut *self.record, Bound::After(d));
        self.to()
    }
}

impl<'m, T: ?Sized, K: Unbounded<C>, C> Handle<'m, T, K, C> {
    /// No bound on the item written last; still inside `shutdown_timeout` at shutdown.
    pub fn unbounded(self) -> Handle<'m, T, K::Next, K::Construction> {
        <K as sealed::State>::write_unbounded(&mut *self.record);
        self.to()
    }
}

impl<'m, T: ?Sized, K: AttemptTimeout, C> Handle<'m, T, K, C> {
    /// Bounds one attempt of the readiness check; an attempt that times out with retries left
    /// is retried like one that returned `Err`. Needs a `Timer`.
    pub fn attempt_timeout(self, d: Duration) -> Handle<'m, T, K::Next, C> {
        if let Some(ready) = self.record.ready.as_mut() {
            ready.attempt = Bound::After(d);
        }
        self.to()
    }
}

impl<'m, T: ?Sized, W, A, C> Handle<'m, T, ReadyItem<W, A>, C> {
    /// Attempts after the first; the last call wins.
    pub fn retries(self, n: u32) -> Self {
        if let Some(ready) = self.record.ready.as_mut() {
            ready.retries = n;
        }
        self
    }

    /// The wait between one attempt's end and the next, timed by the `Timer`; the last call
    /// wins. A nonzero wait on an app with no `Timer` is a wiring error naming this call.
    #[track_caller]
    pub fn backoff(self, d: Duration) -> Self {
        let location = Location::caller();
        if let Some(ready) = self.record.ready.as_mut() {
            ready.backoff = d;
            ready.backoff_location = Some(location);
        }
        self
    }
}
