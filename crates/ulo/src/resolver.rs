use std::any::{TypeId, type_name};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::binding::{Coercion, Instance, Recipe, downcast_instance};
use crate::dependency::{Dep, Ext, Many};
use crate::error::{LookupError, LookupKind};
use crate::execution::{ExecShared, ExecutionRef};
use crate::graph::{BindingId, Graph, ModuleId, Visible};
use crate::key::{BindingKind, Key};
use crate::module::handle::ModuleRef;

/// The view an injection point reads through: the graph, the module whose visibility applies,
/// and the current execution when there is one.
///
/// `Resolver` is `Sync`, which is what lets
/// [`FromContainer::read`](crate::FromContainer::read) and
/// [`Construct::construct`](crate::Construct::construct) hold it across an await and still return
/// `Send` futures.
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
        Resolver { app, graph: app.graph(), module, exec, purpose }
    }

    /// The same view with another module's visibility: a binding is always constructed against
    /// its origin module.
    pub(crate) fn in_module(&self, module: ModuleId) -> Resolver<'a> {
        Resolver { app: self.app, graph: Arc::clone(&self.graph), module, exec: self.exec, purpose: self.purpose }
    }

    /// An instance of one binding in this resolver's context: the stored singleton, the
    /// execution's cached instance, or a fresh transient. Owned by the app (§3.8, §9.5).
    pub(crate) async fn instance(&self, id: BindingId) -> Result<Instance, LookupError> {
        self.app.obtain(self, id).await
    }

    /// The single binding `T` visible from this module, or the execution input under that key.
    pub async fn dep<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.dep_qualified::<T, ()>().await
    }

    /// The single binding `T @ Q`. A separate method because a function's type parameters take
    /// no defaults, and `dep::<PgPool>()` reads the unqualified key.
    pub async fn dep_qualified<T: ?Sized + Send + Sync + 'static, Q: 'static>(&self) -> Result<Dep<T, Q>, LookupError> {
        self.single::<T>(Key::of::<T, Q>()).await.map(Dep::from_arc)
    }

    /// Every contribution to `T`, constructed now, in collection order.
    pub async fn many<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Many<T>, LookupError> {
        self.many_qualified::<T, ()>().await
    }

    /// Every contribution to `T @ Q`, constructed now, in collection order.
    pub async fn many_qualified<T: ?Sized + Send + Sync + 'static, Q: 'static>(&self) -> Result<Many<T, Q>, LookupError> {
        let key = Key::of::<T, Q>();
        let ids = self.contributions(key)?;
        let mut items = Vec::with_capacity(ids.len());
        // In order, one at a time: after a failure no later contribution has started.
        for &id in ids.iter() {
            let instance = self.instance(id).await?;
            let item = self.widen::<T>(id, &instance).ok_or_else(|| wrong_type::<T>(key, BindingKind::Collection))?;
            items.push(item);
        }
        Ok(Many::from_items(items.into()))
    }

    /// One unresolved entry per contribution to `T`, in collection order. Each entry is built
    /// only when its [`Entry::resolve`] is awaited, which is how a transport walks a guard
    /// collection without building the guards after a refusal (§7).
    pub fn entries<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Entries<'_, T>, LookupError> {
        let ids = self.contributions(Key::of::<T, ()>())?;
        Ok(Entries { resolver: self, ids, next: 0, _t: PhantomData })
    }

    /// The extension `T` of the current execution.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Result<Ext<T>, LookupError> {
        let key = Key::of::<T, ()>();
        let exec = self.exec_for(key)?;
        exec.extensions.get::<T>().map(Ext::from_arc).ok_or_else(|| not_found(key, LookupKind::Extension))
    }

    /// The execution input `T`, as the transport seeded it. A standalone execution that did not
    /// seed it answers `LookupError::NotFound` with `LookupKind::Input`.
    pub fn input<T: Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        let key = Key::of::<T, ()>();
        let exec = self.exec_for(key)?;
        exec.inputs.get::<T>().map(Dep::from_arc).ok_or_else(|| not_found(key, LookupKind::Input))
    }

    /// A handle to the module this resolver reads for.
    pub fn module(&self) -> ModuleRef {
        let exec = self.exec.map(|shared| ExecutionRef { shared: Arc::clone(shared) });
        ModuleRef::new(Arc::clone(self.app), self.module, exec)
    }

    /// A handle to the current execution.
    pub fn execution(&self) -> Result<ExecutionRef, LookupError> {
        let shared = self.exec_for(Key::of::<ExecutionRef, ()>())?;
        Ok(ExecutionRef { shared: Arc::clone(shared) })
    }

    /// The binding under an erased `key`, asked for as `T`. The one lookup that can ask for a
    /// `T` the key does not hold, answered with `LookupError::WrongType` (§3.1, §10.2).
    pub async fn by_key<T: ?Sized + Send + Sync + 'static>(&self, key: Key) -> Result<Arc<T>, LookupError> {
        self.single::<T>(key).await
    }

    /// The single binding or input under `key`, as `T`. The key is located and its type checked
    /// before anything is built, so a wrong `T` constructs nothing.
    async fn single<T: ?Sized + Send + Sync + 'static>(&self, key: Key) -> Result<Arc<T>, LookupError> {
        let found = self.locate(key)?;
        if key.type_id() != TypeId::of::<T>() {
            return Err(wrong_type::<T>(key, BindingKind::Single));
        }
        match found {
            Found::Binding(id) => {
                let instance = self.instance(id).await?;
                self.widen::<T>(id, &instance).ok_or_else(|| wrong_type::<T>(key, BindingKind::Single))
            }
            Found::Input(input) => self.seeded::<T>(input),
        }
    }

    /// What `key` names in this module's visibility table. A collection key read as a single is
    /// `WrongKind`. A key with several sources fails any read of it at `wire()`, but a
    /// runtime lookup in a module other than the root can still meet one; it answers
    /// `Ambiguous` with the modules that export the key (§8.2).
    fn locate(&self, key: Key) -> Result<Found, LookupError> {
        match self.graph.lookup(self.module, key) {
            Some(Visible::Binding(id)) => Ok(Found::Binding(*id)),
            Some(Visible::Input(input)) => Ok(Found::Input(*input)),
            Some(Visible::Ambiguous(sources)) => Err(LookupError::Ambiguous {
                key: key.name(BindingKind::Single),
                sources: sources.iter().map(|(module, _)| self.graph.module(*module).name.clone()).collect(),
            }),
            None if !self.graph.collection(key).is_empty() => Err(LookupError::WrongKind {
                key: key.name(BindingKind::Collection),
                expected: BindingKind::Single,
                found: BindingKind::Collection,
            }),
            None => Err(not_found(key, LookupKind::Binding)),
        }
    }

    /// Every contribution to `key`, from every module. A collection nothing contributes to is
    /// empty, unless this module sees a single binding or input under the key, which is
    /// `WrongKind`.
    fn contributions(&self, key: Key) -> Result<Arc<[BindingId]>, LookupError> {
        let ids = self.graph.collection(key);
        if ids.is_empty() && self.graph.lookup(self.module, key).is_some() {
            return Err(LookupError::WrongKind {
                key: key.name(BindingKind::Single),
                expected: BindingKind::Collection,
                found: BindingKind::Single,
            });
        }
        Ok(ids)
    }

    /// The input under `input` as the transport seeded it. `Inputs` stores each input in the
    /// `Instance` shape, which is what lets an unsized-generic `T` downcast it.
    fn seeded<T: ?Sized + Send + Sync + 'static>(&self, input: Key) -> Result<Arc<T>, LookupError> {
        let exec = self.exec_for(input)?;
        let erased = exec.inputs.get_erased(input.type_id()).ok_or_else(|| not_found(input, LookupKind::Input))?;
        downcast_instance::<T>(&erased).ok_or_else(|| wrong_type::<T>(input, BindingKind::Single))
    }

    fn exec_for(&self, key: Key) -> Result<&'a Arc<ExecShared>, LookupError> {
        self.exec.ok_or(LookupError::ExecutionRequired { key: key.name(BindingKind::Single) })
    }

    /// `instance` as `T`. A binding is stored as the type it builds; a contribution's collection
    /// type and an `also_as` key are reached through the coercion the binding recorded.
    fn widen<T: ?Sized + Send + Sync + 'static>(&self, id: BindingId, instance: &Instance) -> Option<Arc<T>> {
        if let Some(arc) = downcast_instance::<T>(instance) {
            return Some(arc);
        }
        let coerce = self.coercion_to(id, TypeId::of::<T>())?;
        downcast_instance::<T>(&coerce(instance))
    }

    /// The coercion from binding `id`'s built type to `ty`: a contribution's own, or the
    /// `also_as` entry for `ty`. An alias has none of its own and defers to its target, located
    /// in the alias's module as its wiring edge was.
    fn coercion_to(&self, mut id: BindingId, ty: TypeId) -> Option<&Coercion> {
        // Bounded by the binding count, so an alias chain that loops ends here.
        for _ in 0..self.graph.bindings.len() {
            let binding = self.graph.binding(id);
            let record = &binding.record;
            if record.primary.type_id() == ty {
                if let Some(coerce) = &record.into_primary {
                    return Some(coerce);
                }
            }
            if let Some(also) = record.also.iter().find(|also| also.key.type_id() == ty) {
                return Some(&also.coerce);
            }
            let Recipe::Alias { target } = &record.recipe else { return None };
            match self.graph.lookup(binding.origin, *target) {
                Some(Visible::Binding(next)) => id = *next,
                _ => return None,
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
enum Found {
    Binding(BindingId),
    Input(Key),
}

fn not_found(key: Key, kind: LookupKind) -> LookupError {
    LookupError::NotFound { key: key.name(BindingKind::Single), kind }
}

fn wrong_type<T: ?Sized>(key: Key, kind: BindingKind) -> LookupError {
    LookupError::WrongType { key: key.name(kind), requested: type_name::<T>() }
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
        let instance = self.resolver.instance(self.id).await?;
        self.resolver
            .widen::<T>(self.id, &instance)
            .ok_or_else(|| wrong_type::<T>(Key::of::<T, ()>(), BindingKind::Collection))
    }
}
