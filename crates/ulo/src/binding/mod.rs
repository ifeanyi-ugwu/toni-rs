//! Bindings as the module definition records them: the recipe, the declared sites, the bounds,
//! the readiness check and the closure hooks. The value API in `ModuleDef` writes these records
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
use crate::hooks::HookRecord;
use crate::key::{BindingKind, Key};
use crate::resolver::Resolver;
use crate::scope::ScopeKind;
use crate::site::Sites;
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
/// get the higher-ranked signature; the `erase_*` helpers' declared types do.
pub(crate) type ErasedCtor =
    Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Instance, ConstructError>> + Send + Sync>;

/// `Arc<T>` to `Arc<U>`, captured from a `|a| a` closure where the unsizing coercion happens.
pub(crate) type Coercion = Arc<dyn Fn(&Instance) -> Instance + Send + Sync>;

/// A readiness closure: `Ok(())` when the resource answers.
pub(crate) type CheckFn =
    Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<(), ConstructError>> + Send + Sync>;

pub(crate) fn erase_construct<T: Construct>() -> ErasedCtor {
    todo!()
}

pub(crate) fn coercion<T, U, F>(coerce: F) -> Coercion
where
    T: ?Sized + Send + Sync + 'static,
    U: ?Sized + Send + Sync + 'static,
    F: Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
{
    todo!()
}

/// One binding as `register` declared it, before the graph assigns it an id.
pub(crate) struct BindingRecord {
    /// `T @ ()` for a single binding, `U @ ()` for a contribution to `U`; the qualifier is
    /// applied from `qualifier` when the graph freezes the record.
    pub(crate) primary: Key,
    pub(crate) qualifier: Qualifier,
    /// `type_name` of the type the recipe builds, which differs from `primary` for a
    /// contribution; diagnostics print it.
    pub(crate) built: &'static str,
    /// For a contribution: the built `Arc<T>` to the collection's `Arc<U>`.
    pub(crate) into_primary: Option<Coercion>,
    /// Further keys reaching the same object (`also_as`), unqualified like `primary`.
    pub(crate) also: Vec<AlsoAs>,
    pub(crate) kind: BindingKind,
    pub(crate) scope: ScopeKind,
    /// Set by `ModuleDef::controller`; enhancer roles are found by the wiring pass.
    pub(crate) controller: bool,
    pub(crate) recipe: Recipe,
    pub(crate) sites: Sites,
    pub(crate) construct_bound: Bound,
    pub(crate) ready: Option<ReadyRecord>,
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

pub(crate) struct AlsoAs {
    pub(crate) key: Key,
    pub(crate) coerce: Coercion,
}

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

pub(crate) struct ReadyRecord {
    pub(crate) check: CheckFn,
    pub(crate) sites: Sites,
    pub(crate) retries: u32,
    pub(crate) backoff: Duration,
    /// `.timeout(..)`: the whole check, retries and backoff included. `Limit::Item` on expiry.
    pub(crate) whole: Bound,
    /// `.attempt_timeout(..)`: one attempt. `Limit::Attempt` when written, `Limit::Default`
    /// (`construct_timeout`) when left at `Default` with a `Timer`.
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
        sites: Sites,
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
            sites,
            construct_bound: Bound::Default,
            ready: None,
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
