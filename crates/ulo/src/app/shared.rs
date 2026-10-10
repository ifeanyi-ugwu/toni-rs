//! The state every `App` typestate, `AppHandle`, `ModuleRef` and execution shares.

use std::any::{TypeId, type_name};
use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};
use std::task::{Poll, Waker};

use crate::app::AppConfig;
use crate::binding::{ErasedCtor, Instance, Qualifier, Recipe};
use crate::construct::ConstructError;
use crate::dependency::Dep;
use crate::error::{Closed, FailureReason, LookupError, LookupKind};
use crate::execution::cache::{OnceCells, Slot};
use crate::execution::extensions::{Extensions, Seeded};
use crate::execution::notify::Notify;
use crate::execution::{CancelReason, ExecOptions, ExecShared, Execution, StreamEnd};
use crate::graph::{BindingId, Effective, FrozenBinding, Graph, ModuleId, Visible};
use crate::key::{BindingKind, Key, KeyName};
use crate::lifecycle::phase::{Phase, PhaseCell};
use crate::lifecycle::run::{Outcome, run};
use crate::lifecycle::shutdown::ShutdownCell;
use crate::module::handle::ModuleRef;
use crate::redact::redact;
use crate::resolver::{Purpose, Resolver};
use crate::timer::{BoundKind, BoxFuture, resolve_bound};
use crate::transport::server::ErasedServer;

pub(crate) struct AppShared {
    /// Only `load` writes it, swapping in an extended graph; readers clone the `Arc` and never
    /// hold the lock across an await.
    pub(crate) graph: RwLock<Arc<Graph>>,
    pub(crate) config: AppConfig,
    pub(crate) singletons: SingletonStore,
    /// The enhancers declared by closure that the wiring pass decided are built once, each on
    /// its first use by a call (§7).
    pub(crate) closures: OnceCells,
    pub(crate) phase: PhaseCell,
    /// Fired when the app stops accepting: `draining()` on executions and handles.
    pub(crate) draining: Notify,
    pub(crate) live: Arc<LiveSet>,
    pub(crate) shutdown: ShutdownCell,
    /// The bound transports, set by `listen`; the shutdown sequence drains and closes them.
    pub(crate) servers: Mutex<Vec<Arc<dyn ErasedServer>>>,
    /// Serializes `load`: one lazy module wires against one graph at a time.
    pub(crate) load_lock: async_lock::Mutex<()>,
}

impl AppShared {
    pub(crate) fn new(graph: Graph, config: AppConfig) -> Arc<Self> {
        config.redactor.set(&graph.secrets);
        Arc::new(AppShared {
            graph: RwLock::new(Arc::new(graph)),
            config,
            singletons: SingletonStore::default(),
            closures: OnceCells::default(),
            phase: PhaseCell::new(Phase::Wired),
            draining: Notify::new(),
            live: Arc::new(LiveSet::default()),
            shutdown: ShutdownCell::new(),
            servers: Mutex::new(Vec::new()),
            load_lock: async_lock::Mutex::new(()),
        })
    }

    pub(crate) fn graph(&self) -> Arc<Graph> {
        let graph = self.graph.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&*graph)
    }

    /// Makes `graph` the app's, its secrets with it, for the redaction of a task spawned through
    /// the app's runtime.
    pub(crate) fn publish(&self, graph: Arc<Graph>) {
        self.config.redactor.set(&graph.secrets);
        *self.graph.write().unwrap_or_else(PoisonError::into_inner) = graph;
    }

    pub(crate) fn root(self: &Arc<Self>) -> ModuleRef {
        let root = self.graph().root;
        self.module_ref(root)
    }

    pub(crate) fn module_ref(self: &Arc<Self>, module: ModuleId) -> ModuleRef {
        ModuleRef::new(Arc::clone(self), module, None)
    }

    /// `T` with the root module's visibility, outside any execution: `App::get` and
    /// `AppHandle::get`.
    pub(crate) async fn get_root<T: ?Sized + Send + Sync + 'static>(self: &Arc<Self>) -> Result<Dep<T>, LookupError> {
        let r = Resolver::new(self, self.graph().root, None, Purpose::Lookup);
        r.dep::<T>().await
    }

    /// The one module whose identity names the type `M`, keyed by `qualifier` when given.
    pub(crate) fn find_module<M: 'static>(self: &Arc<Self>, qualifier: Option<Qualifier>) -> Result<ModuleRef, LookupError> {
        let id = self.graph().find_module(TypeId::of::<M>(), type_name::<M>(), qualifier)?;
        Ok(self.module_ref(id))
    }

    /// An instance of `id` in `r`'s context:
    /// - a singleton from the store: `NotReady` while `connect` has not built it,
    ///   `LookupError::Closed` from Destroying on unless `r` reads for the lifecycle;
    /// - an execution-scoped binding from the execution's cache, built on first read, or
    ///   `ExecutionRequired` without an execution;
    /// - a transient built fresh;
    /// - an alias through its target, a coerced key through its source instance.
    ///
    /// A build inside a call runs under `CONSTRUCT_TIMEOUT` with panics caught, and fails as
    /// `LookupError::Construct` with the reason redacted.
    ///
    /// The store and the execution cache hold an instance as its recipe built it, which is what
    /// a trait hook downcasts to. The answer is in the type of the binding's primary key: a
    /// contribution is widened to its collection's type here. A caller that reached the binding
    /// through an `also_as` key applies that key's coercion to the answer.
    pub(crate) async fn obtain(self: &Arc<Self>, r: &Resolver<'_>, id: BindingId) -> Result<Instance, LookupError> {
        let graph = Arc::clone(&r.graph);
        let binding = graph.binding(id);
        let key = key_name(binding);
        let built = match &binding.record.recipe {
            Recipe::Alias { target } => return self.obtain_alias(r, &graph, binding, *target).await,
            Recipe::Failed => return Err(LookupError::NotFound { key, kind: LookupKind::Binding }),
            Recipe::Value(value) => {
                if binding.effective == Effective::Singleton {
                    self.check_singleton_lookup(r, key)?;
                }
                Arc::clone(value)
            }
            Recipe::Construct(ctor) | Recipe::Factory(ctor) => match binding.effective {
                Effective::Singleton => {
                    self.check_singleton_lookup(r, key)?;
                    self.singletons.get(id).ok_or(LookupError::NotReady { key })?
                }
                Effective::PerExecution => {
                    let exec = r.exec.ok_or(LookupError::ExecutionRequired { key })?;
                    exec.cache.get_or_build(Slot::Binding(id), || self.build(r, binding, ctor, key)).await?
                }
                Effective::Transient => self.build(r, binding, ctor, key).await?,
            },
        };
        Ok(match &binding.record.into_primary {
            Some(widen) => widen(&built),
            None => built,
        })
    }

    /// Opens an execution in `module`; refused with `Closed` by phase (§9.5): ordinary
    /// executions from Draining on, terminal ones from the drain's end on. A refused terminal
    /// execution counts in `Shutdown::terminal_skipped`.
    pub(crate) fn open(self: &Arc<Self>, module: ModuleId, opts: ExecOptions, terminal: bool) -> Result<Execution, Closed> {
        // Entered before the phase check, so a drain that begins after the check passes finds
        // the entry; a refusal drops it again. The phase check counts a refused terminal
        // execution itself.
        let live = self.live.enter();
        if !self.phase.allows_execution(terminal) {
            return Err(Closed::new());
        }
        let shared = Arc::new(ExecShared {
            app: Arc::clone(self),
            module,
            routed: OnceLock::new(),
            cache: OnceCells::default(),
            extensions: Extensions::default(),
            inputs: Seeded::default(),
            cancel: Notify::new(),
            cancel_reason: OnceLock::new(),
            deadline: opts.deadline,
            handler: OnceLock::new(),
            late: AtomicBool::new(false),
            stream_end: Mutex::new(StreamEnd::default()),
            terminal,
            live,
        });
        self.live.attach(&shared.live, &shared);
        Ok(Execution { shared })
    }

    /// The body of every `execute`: open, run the closure against the borrowed execution, fire
    /// cancellation at the deadline while still polling the closure, drop the execution when the
    /// closure's future completes.
    pub(crate) async fn execute<F, R>(self: &Arc<Self>, module: ModuleId, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        let exec = self.open(module, opts, false)?;
        let out = match self.deadline_sleep(&exec) {
            None => f(&exec).await,
            Some(sleep) => cancel_at(&exec.shared, sleep, f(&exec)).await,
        };
        drop(exec);
        Ok(out)
    }

    /// The sleep that ends at `exec`'s deadline on the app's `Timer`. A deadline already past
    /// fires cancellation before the closure first runs. Without a `Timer` there is no clock to
    /// read a deadline against, and it never fires.
    fn deadline_sleep(&self, exec: &Execution) -> Option<BoxFuture<'static, ()>> {
        let at = exec.shared.deadline?;
        let timer = self.config.timer.as_deref()?;
        let left = at.saturating_duration_since(timer.now());
        if left.is_zero() {
            exec.shared.cancel_with(CancelReason::Deadline);
            return None;
        }
        Some(timer.sleep(left))
    }

    fn check_singleton_lookup(&self, r: &Resolver<'_>, key: KeyName) -> Result<(), LookupError> {
        if r.purpose == Purpose::Lookup && !self.phase.allows_singleton_lookup() {
            return Err(LookupError::Closed { key });
        }
        Ok(())
    }

    /// Builds an execution-scoped or transient binding inside a call, against its origin
    /// module's visibility.
    async fn build(
        self: &Arc<Self>,
        r: &Resolver<'_>,
        binding: &FrozenBinding,
        ctor: &ErasedCtor,
        key: KeyName,
    ) -> Result<Instance, LookupError> {
        let r = r.in_module(binding.origin);
        let defaults = self.config.defaults();
        let bound = resolve_bound(binding.record.construct_bound, BoundKind::Construction, defaults.as_ref());
        let secrets = &r.graph.secrets;
        let reason = match run(self.config.timer.as_deref(), bound, None, secrets, ctor(&r)).await {
            Outcome::Done(Ok(instance)) => return Ok(instance),
            Outcome::Done(Err(ConstructError::Dependency(e))) => return Err(e),
            Outcome::Done(Err(ConstructError::Failed(e))) => FailureReason::Errored(redact(secrets, e)),
            Outcome::Panicked(payload) => FailureReason::Panicked(payload),
            Outcome::TimedOut { after, limit } => FailureReason::TimedOut { after, limit },
        };
        Err(LookupError::Construct { key, reason })
    }

    /// An alias reads its target from its own module's visibility and answers in the target
    /// key's type, which is the alias's own: an alias changes the qualifier, never the type.
    async fn obtain_alias(
        self: &Arc<Self>,
        r: &Resolver<'_>,
        graph: &Graph,
        alias: &FrozenBinding,
        target: Key,
    ) -> Result<Instance, LookupError> {
        match graph.lookup(alias.origin, target) {
            Some(Visible::Binding(source)) => {
                let source = *source;
                let instance = self.obtain_boxed(r, source).await?;
                Ok(as_key(graph.binding(source), target, instance))
            }
            Some(Visible::Input(input)) => {
                let key = input.name(BindingKind::Single);
                let exec = r.exec.ok_or(LookupError::ExecutionRequired { key })?;
                exec.inputs.get_erased(input.type_id()).ok_or(LookupError::NotFound { key, kind: LookupKind::Input })
            }
            _ => Err(LookupError::NotFound { key: target.name(BindingKind::Single), kind: LookupKind::Binding }),
        }
    }

    /// `obtain` behind a named future type: an alias chain recurses, and an `async fn` cannot
    /// contain its own future.
    fn obtain_boxed<'a>(self: &'a Arc<Self>, r: &'a Resolver<'a>, id: BindingId) -> BoxFuture<'a, Result<Instance, LookupError>> {
        Box::pin(self.obtain(r, id))
    }
}

/// The binding's own key as errors name it, before any keyed module's boundary requalifies it.
fn key_name(binding: &FrozenBinding) -> KeyName {
    let record = &binding.record;
    record.primary.with_qualifier(record.qualifier.id, record.qualifier.name).name(record.kind)
}

/// `instance`, answered by `obtain` in `source`'s primary key type, converted to the type of
/// `key`, which names `source` under its primary key or under one of its `also_as` keys. Keys
/// are compared by type: a keyed module's boundary changes a key's qualifier, never its type.
fn as_key(source: &FrozenBinding, key: Key, instance: Instance) -> Instance {
    if key.type_id() == source.record.primary.type_id() {
        return instance;
    }
    match source.record.also.iter().find(|also| also.key.type_id() == key.type_id()) {
        Some(also) => (also.coerce)(&instance),
        None => instance,
    }
}

/// Polls `fut` to completion, firing `cancel` when `sleep` resolves and polling `fut` on past it:
/// cancellation is a signal to the closure, not the end of its future.
async fn cancel_at<Fut: Future>(exec: &ExecShared, sleep: BoxFuture<'static, ()>, fut: Fut) -> Fut::Output {
    let mut sleep = Some(sleep);
    let mut fut = pin!(fut);
    poll_fn(move |cx| {
        if let Some(pending) = sleep.as_mut() {
            if pending.as_mut().poll(cx).is_ready() {
                sleep = None;
                exec.cancel_with(CancelReason::Deadline);
            }
        }
        fut.as_mut().poll(cx)
    })
    .await
}

/// The singletons `connect` built, by binding. A binding absent here during `connect` is not
/// yet built.
///
/// Each instance is held as its recipe built it, not widened to a collection's type, so the
/// lifecycle runner can hand a contribution's trait hooks their own type.
#[derive(Default)]
pub(crate) struct SingletonStore {
    slots: RwLock<HashMap<BindingId, Instance>>,
}

impl SingletonStore {
    pub(crate) fn get(&self, id: BindingId) -> Option<Instance> {
        self.slots.read().unwrap_or_else(PoisonError::into_inner).get(&id).cloned()
    }

    pub(crate) fn insert(&self, id: BindingId, instance: Instance) {
        self.slots.write().unwrap_or_else(PoisonError::into_inner).insert(id, instance);
    }

    /// Drops what a failed `load` built: the ids it held are assigned again by the next load.
    pub(crate) fn remove(&self, id: BindingId) {
        self.slots.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

/// Every execution still alive, for the drain to wait on and, at its end, to cancel.
#[derive(Default)]
pub(crate) struct LiveSet {
    inner: Mutex<LiveInner>,
}

#[derive(Default)]
struct LiveInner {
    next: u64,
    /// An entry exists from `enter`; it holds an empty `Weak` until `attach`.
    executions: HashMap<u64, Weak<ExecShared>>,
    /// Woken whenever an execution ends, so the drain re-checks for an empty set.
    waiters: Vec<Waker>,
    /// Set by `cancel_all`: an execution attached after the drain's end is cancelled at once.
    cancelled: bool,
}

impl LiveSet {
    /// Registers a new execution; the slot removes it when the execution's shared state drops.
    pub(crate) fn enter(self: &Arc<Self>) -> LiveSlot {
        let mut inner = self.lock();
        let id = inner.next;
        inner.next += 1;
        inner.executions.insert(id, Weak::new());
        drop(inner);
        LiveSlot { set: Arc::clone(self), id }
    }

    /// Links the slot to the execution it belongs to, once the shared state exists.
    pub(crate) fn attach(&self, slot: &LiveSlot, exec: &Arc<ExecShared>) {
        let cancelled = {
            let mut inner = self.lock();
            if let Some(entry) = inner.executions.get_mut(&slot.id) {
                *entry = Arc::downgrade(exec);
            }
            inner.cancelled
        };
        if cancelled {
            exec.cancel_with(CancelReason::Drain);
        }
    }

    pub(crate) fn until_empty(&self) -> impl Future<Output = ()> + Send + '_ {
        poll_fn(move |cx| {
            let mut inner = self.lock();
            if inner.executions.is_empty() {
                return Poll::Ready(());
            }
            if !inner.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                inner.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
    }

    /// Fires cancellation on every live execution and returns how many there were, and how many
    /// of them were terminal: the drain's end (§9.5 step 4).
    pub(crate) fn cancel_all(&self) -> (usize, usize) {
        // Upgraded under the lock and released after it: dropping the last strong reference ends
        // an execution, and its slot takes the lock.
        let live: Vec<Arc<ExecShared>> = {
            let mut inner = self.lock();
            inner.cancelled = true;
            inner.executions.values().filter_map(Weak::upgrade).collect()
        };
        for exec in &live {
            exec.cancel_with(CancelReason::Drain);
        }
        let terminal = live.iter().filter(|exec| exec.terminal).count();
        (live.len(), terminal)
    }

    fn lock(&self) -> MutexGuard<'_, LiveInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An execution's entry in the live set.
pub(crate) struct LiveSlot {
    pub(crate) set: Arc<LiveSet>,
    pub(crate) id: u64,
}

impl Drop for LiveSlot {
    fn drop(&mut self) {
        let waiters = {
            let mut inner = self.set.lock();
            inner.executions.remove(&self.id);
            if inner.executions.is_empty() { std::mem::take(&mut inner.waiters) } else { Vec::new() }
        };
        for waker in waiters {
            waker.wake();
        }
    }
}
