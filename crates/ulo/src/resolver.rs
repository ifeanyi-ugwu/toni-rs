use std::marker::PhantomData;
use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::binding::Instance;
use crate::error::LookupError;
use crate::execution::{ExecShared, ExecutionRef};
use crate::graph::{BindingId, Graph, ModuleId};
use crate::key::Key;
use crate::module::handle::ModuleRef;
use crate::site::{Dep, Ext, Many};

/// The view a site reads through: the graph, the module whose visibility applies, and the
/// current execution when there is one.
///
/// `Resolver` is `Sync`, which is what lets [`Site::read`](crate::Site::read) and
/// [`Construct::construct`](crate::Construct::construct) hold it across an await and still
/// return `Send` futures.
pub struct Resolver<'a> {
    pub(crate) app: &'a Arc<AppShared>,
    /// A snapshot: `load` swaps the app's graph for an extended one and never mutates this one.
    pub(crate) graph: Arc<Graph>,
    pub(crate) module: ModuleId,
    pub(crate) exec: Option<&'a Arc<ExecShared>>,
    pub(crate) purpose: Purpose,
}

/// Who is reading. The lifecycle runner reads the bindings its own hooks name while the app is
/// Destroying, where every other singleton lookup answers `LookupError::Closed` (§9.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Lookup,
    Lifecycle,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(
        app: &'a Arc<AppShared>,
        module: ModuleId,
        exec: Option<&'a Arc<ExecShared>>,
        purpose: Purpose,
    ) -> Self {
        todo!()
    }

    /// The same view with another module's visibility: a binding is always constructed against
    /// its origin module.
    pub(crate) fn in_module(&self, module: ModuleId) -> Resolver<'a> {
        Resolver { app: self.app, graph: Arc::clone(&self.graph), module, exec: self.exec, purpose: self.purpose }
    }

    /// An instance of one binding in this resolver's context: the stored singleton, the
    /// execution's cached instance, or a fresh transient. Owned by the app (§3.8, §9.5).
    pub(crate) async fn instance(&self, id: BindingId) -> Result<Instance, LookupError> {
        todo!()
    }

    /// The single binding `T` visible from this module, or the execution input under that key.
    pub async fn dep<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.dep_qualified::<T, ()>().await
    }

    /// The single binding `T @ Q`. A separate method because a function's type parameters take
    /// no defaults, and `dep::<PgPool>()` reads the unqualified key.
    pub async fn dep_qualified<T: ?Sized + Send + Sync + 'static, Q: 'static>(&self) -> Result<Dep<T, Q>, LookupError> {
        todo!()
    }

    /// Every contribution to `T`, constructed now, in collection order.
    pub async fn many<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Many<T>, LookupError> {
        self.many_qualified::<T, ()>().await
    }

    /// Every contribution to `T @ Q`, constructed now, in collection order.
    pub async fn many_qualified<T: ?Sized + Send + Sync + 'static, Q: 'static>(&self) -> Result<Many<T, Q>, LookupError> {
        todo!()
    }

    /// One unresolved entry per contribution to `T`, in collection order. Each entry is built
    /// only when its [`Entry::resolve`] is awaited, which is how a transport walks a guard
    /// collection without building the guards after a refusal (§7).
    pub fn entries<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Entries<'_, T>, LookupError> {
        todo!()
    }

    /// The extension `T` of the current execution.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        todo!()
    }

    /// The execution input `T`, as the transport seeded it. A standalone execution that did not
    /// seed it answers `LookupError::NotFound` with `LookupKind::Input`.
    pub fn input<T: Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        todo!()
    }

    /// A handle to the module this resolver reads for.
    pub fn module(&self) -> ModuleRef {
        todo!()
    }

    /// A handle to the current execution.
    pub fn execution(&self) -> Result<ExecutionRef, LookupError> {
        todo!()
    }

    /// The binding under an erased `key`, asked for as `T`. The one lookup that can ask for a
    /// `T` the key does not hold, answered with `LookupError::WrongType` (§3.1, §10.2).
    pub async fn by_key<T: ?Sized + Send + Sync + 'static>(&self, key: Key) -> Result<Arc<T>, LookupError> {
        todo!()
    }
}

/// The contributions to one collection, unresolved, in collection order.
pub struct Entries<'r, T: ?Sized> {
    pub(crate) resolver: &'r Resolver<'r>,
    pub(crate) ids: Arc<[BindingId]>,
    pub(crate) next: usize,
    pub(crate) _t: PhantomData<fn() -> Box<T>>,
}

impl<'r, T: ?Sized + Send + Sync + 'static> Iterator for Entries<'r, T> {
    type Item = Entry<'r, T>;

    fn next(&mut self) -> Option<Entry<'r, T>> {
        let id = *self.ids.get(self.next)?;
        self.next += 1;
        Some(Entry { resolver: self.resolver, id, _t: PhantomData })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.ids.len() - self.next;
        (left, Some(left))
    }
}

impl<T: ?Sized + Send + Sync + 'static> ExactSizeIterator for Entries<'_, T> {}

/// One contribution to a collection, built on [`Entry::resolve`]: the shared singleton, the
/// value, or a per-execution build cached in the current execution.
pub struct Entry<'r, T: ?Sized> {
    pub(crate) resolver: &'r Resolver<'r>,
    pub(crate) id: BindingId,
    pub(crate) _t: PhantomData<fn() -> Box<T>>,
}

impl<T: ?Sized + Send + Sync + 'static> Entry<'_, T> {
    pub async fn resolve(&self) -> Result<Arc<T>, LookupError> {
        todo!()
    }
}
