//! Bindings as the module definition records them: the recipe, the declared dependencies, the
//! bounds, the readiness check and the closure hooks. The value API in `ModuleDef` writes these records
//! through the handles in `handle`; the graph freezes them at `wire()` (§2, §4).

pub(crate) mod alias;
pub(crate) mod contribute;
pub(crate) mod factory;
pub(crate) mod handle;

use std::any::{Any, TypeId, type_name};
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

use crate::construct::{Construct, ConstructError};
use crate::dependency::Dependencies;
use crate::hooks::HookRecord;
use crate::key::{BindingKind, Key};
use crate::resolver::Resolver;
use crate::scope::ScopeKind;
use crate::timer::{BoxFuture, Bound};

/// A built instance as the stores hold it: an `Arc<dyn Any>` wrapping the `Arc<T>` of the key's
/// type, so an unsized `T` such as `dyn Cache` round-trips. Every holder of a binding holds a
/// clone of that inner `Arc<T>`.
pub(crate) type Instance = Arc<dyn Any + Send + Sync>;

pub(crate) fn instance_of<T: ?Sized + Send + Sync + 'static>(value: Arc<T>) -> Instance {
    Arc::new(value)
}

pub(crate) fn downcast_instance<T: ?Sized + Send + Sync + 'static>(instance: &Instance) -> Option<Arc<T>> {
    instance.downcast_ref::<Arc<T>>().cloned()
}

/// The registry slot a constructor is erased into. A closure written inline at the slot does not
/// get the higher-ranked signature; one passed through [`ctor_fn`]'s bound does.
pub(crate) type ErasedCtor =
    Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Instance, ConstructError>> + Send + Sync>;

/// `Arc<T>` to `Arc<U>`, captured from a `|a| a` closure where the unsizing coercion happens.
pub(crate) type Coercion = Arc<dyn Fn(&Instance) -> Instance + Send + Sync>;

/// A readiness closure: `Ok(())` when the resource answers.
pub(crate) type CheckFn =
    Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<(), ConstructError>> + Send + Sync>;

/// Fixes a closure's signature to [`ErasedCtor`]'s higher-ranked one.
pub(crate) fn ctor_fn<C>(ctor: C) -> ErasedCtor
where
    C: for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Instance, ConstructError>> + Send + Sync + 'static,
{
    Arc::new(ctor)
}

/// Fixes a closure's signature to [`CheckFn`]'s higher-ranked one.
pub(crate) fn check_fn<C>(check: C) -> CheckFn
where
    C: for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<(), ConstructError>> + Send + Sync + 'static,
{
    Arc::new(check)
}

pub(crate) fn erase_construct<T: Construct>() -> ErasedCtor {
    ctor_fn(|r| {
        Box::pin(async move {
            let value = T::construct(r).await?;
            Ok::<_, ConstructError>(instance_of(Arc::new(value)))
        })
    })
}

pub(crate) fn coercion<T, U, F>(coerce: F) -> Coercion
where
    T: ?Sized + Send + Sync + 'static,
    U: ?Sized + Send + Sync + 'static,
    F: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
{
    Arc::new(move |instance: &Instance| -> Instance {
        match downcast_instance::<T>(instance) {
            Some(built) => instance_of::<U>(coerce(built)),
            // The resolver picks an `also_as` entry by type alone and may hand it an instance
            // that is not an `Arc<T>`. It passes through unwidened, and the resolver's downcast to
            // `Arc<U>` answers `WrongType` rather than this closure panicking.
            None => Arc::clone(instance),
        }
    })
}

/// One binding as `register` declared it, before the graph assigns it an id. A clone shares
/// every closure and value with the original.
#[derive(Clone)]
pub(crate) struct BindingRecord {
    /// `T @ ()` for a single binding, `U @ ()` for a contribution to `U`; the qualifier is
    /// applied from `qualifier` when the graph freezes the record.
    pub(crate) primary: Key,
    pub(crate) qualifier: Qualifier,
    /// `type_name` of the type the recipe builds, which differs from `primary` for a
    /// contribution; diagnostics print it.
    pub(crate) built: &'static str,
    /// For a contribution: the built `Arc<T>` to the collection's `Arc<U>`. Set on every
    /// contribution, as the identity for `Contribute::value`, whose value is already an `Arc<U>`;
    /// `None` on a single binding.
    pub(crate) into_primary: Option<Coercion>,
    /// Further keys reaching the same object (`also_as`), unqualified like `primary`.
    pub(crate) also: Vec<AlsoAs>,
    pub(crate) kind: BindingKind,
    pub(crate) scope: ScopeKind,
    /// Set by `ModuleDef::controller`; enhancer roles are found by the wiring pass.
    pub(crate) controller: bool,
    pub(crate) recipe: Recipe,
    pub(crate) dependencies: Dependencies,
    pub(crate) construct_bound: Bound,
    /// The check written last. A `.ready(..)` on a handle that already carries one replaces it
    /// and leaves the replaced check's location in `replaced_ready`.
    pub(crate) ready: Option<ReadyRecord>,
    /// The locations of checks a later `.ready(..)` replaced, in call order. Non-empty is
    /// `WiringError::DuplicateReadiness`: the first entry is the first check written, and the
    /// next entry, or `ready`'s location when there is none, the second.
    pub(crate) replaced_ready: Vec<&'static Location<'static>>,
    /// Trait hooks from `T::hooks` first, then closure hooks in the order written.
    pub(crate) hooks: Vec<HookRecord>,
    /// Whether this record is a `Construct` type registered through `provide`, `provide_with`
    /// or `controller`, so `hooks` carries `T::hooks`.
    pub(crate) constructs: bool,
    pub(crate) location: &'static Location<'static>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Qualifier {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
}

impl Qualifier {
    pub(crate) fn of<Q: 'static>() -> Self {
        Qualifier { id: TypeId::of::<Q>(), name: type_name::<Q>() }
    }

    pub(crate) fn none() -> Self {
        Qualifier::of::<()>()
    }
}

#[derive(Clone)]
pub(crate) struct AlsoAs {
    pub(crate) key: Key,
    pub(crate) coerce: Coercion,
}

#[derive(Clone)]
pub(crate) enum Recipe {
    /// `provide::<T>()` and `ModuleDef::controller::<C>()`: `T::construct`.
    Construct(ErasedCtor),
    /// A factory closure: `singleton`, `execution`, `transient`, their `try_` forms and
    /// `provide_with`.
    Factory(ErasedCtor),
    Value(Instance),
    /// `alias::<T, Q>().of::<Existing>()`: reads the binding under `target`.
    Alias { target: Key },
    /// A `try_value` whose expression failed. The error sits on the module node, reported by
    /// `wire()` beside every other wiring error; the record stays so its key is still known.
    Failed,
}

#[derive(Clone)]
pub(crate) struct ReadyRecord {
    pub(crate) check: CheckFn,
    pub(crate) dependencies: Dependencies,
    /// Attempts after the first: `.retries(5)` allows six in all. Zero unless written.
    pub(crate) retries: u32,
    /// The wait between one attempt's end and the next attempt. Zero unless written.
    pub(crate) backoff: Duration,
    /// `.timeout(..)`: the whole check, retries and backoff included. `Limit::Item` on expiry.
    pub(crate) whole: Bound,
    /// `.attempt_timeout(..)`: one attempt. `Limit::Attempt` when written, `Limit::Default`
    /// (`construct_timeout`) when left at `Default` with a `Timer`. It stays `Default` only on a
    /// check that writes no bound at all: writing either bound or `.unbounded()` opts out of the
    /// attempt default (§9.3), so `.timeout(..)` alone writes `Unbounded` here.
    pub(crate) attempt: Bound,
    pub(crate) location: &'static Location<'static>,
}

impl BindingRecord {
    /// A record with no qualifier, second key, bound, check or hook written yet.
    pub(crate) fn new(
        primary: Key,
        built: &'static str,
        kind: BindingKind,
        scope: ScopeKind,
        recipe: Recipe,
        dependencies: Dependencies,
        location: &'static Location<'static>,
    ) -> Self {
        BindingRecord {
            primary,
            qualifier: Qualifier::none(),
            built,
            into_primary: None,
            also: Vec::new(),
            kind,
            scope,
            controller: false,
            recipe,
            dependencies,
            construct_bound: Bound::Default,
            ready: None,
            replaced_ready: Vec::new(),
            hooks: Vec::new(),
            constructs: false,
            location,
        }
    }

    /// Every key this record answers to, with the qualifier applied.
    pub(crate) fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        std::iter::once(self.primary)
            .chain(self.also.iter().map(|a| a.key))
            .map(|k| k.with_qualifier(self.qualifier.id, self.qualifier.name))
    }
}
