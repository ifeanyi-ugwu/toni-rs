//! The state every `App` typestate, `AppHandle`, `ModuleRef` and execution shares.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock, Weak};

use crate::app::AppConfig;
use crate::binding::Instance;
use crate::error::{Closed, LookupError};
use crate::execution::notify::Notify;
use crate::execution::{ExecOptions, ExecShared, Execution};
use crate::graph::{BindingId, Graph, ModuleId};
use crate::lifecycle::phase::PhaseCell;
use crate::lifecycle::shutdown::ShutdownCell;
use crate::module::handle::ModuleRef;
use crate::resolver::Resolver;
use crate::transport::server::ErasedServer;

pub(crate) struct AppShared {
    /// Only `load` writes it, swapping in an extended graph; readers clone the `Arc` and never
    /// hold the lock across an await.
    pub(crate) graph: RwLock<Arc<Graph>>,
    pub(crate) config: AppConfig,
    pub(crate) singletons: SingletonStore,
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
        todo!()
    }

    pub(crate) fn graph(&self) -> Arc<Graph> {
        todo!()
    }

    pub(crate) fn root(self: &Arc<Self>) -> ModuleRef {
        todo!()
    }

    pub(crate) fn module_ref(self: &Arc<Self>, module: ModuleId) -> ModuleRef {
        ModuleRef::new(Arc::clone(self), module, None)
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
    pub(crate) async fn obtain(self: &Arc<Self>, r: &Resolver<'_>, id: BindingId) -> Result<Instance, LookupError> {
        todo!()
    }

    /// Opens an execution in `module`; refused with `Closed` by phase (§9.5): ordinary
    /// executions from Draining on, terminal ones from the drain's end on. A refused terminal
    /// execution counts in `Shutdown::terminal_skipped`.
    pub(crate) fn open(self: &Arc<Self>, module: ModuleId, opts: ExecOptions, terminal: bool) -> Result<Execution, Closed> {
        todo!()
    }

    /// The body of every `execute`: open, run the closure against the borrowed execution, fire
    /// cancellation at the deadline while still polling the closure, drop the execution when the
    /// closure's future completes.
    pub(crate) async fn execute<F, R>(self: &Arc<Self>, module: ModuleId, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        todo!()
    }
}

/// The singletons `connect` built, by binding. A binding absent here during `connect` is not
/// yet built.
#[derive(Default)]
pub(crate) struct SingletonStore {
    slots: RwLock<HashMap<BindingId, Instance>>,
}

impl SingletonStore {
    pub(crate) fn get(&self, id: BindingId) -> Option<Instance> {
        todo!()
    }

    pub(crate) fn insert(&self, id: BindingId, instance: Instance) {
        todo!()
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
    executions: HashMap<u64, Weak<ExecShared>>,
    /// Woken whenever an execution ends, so the drain re-checks for an empty set.
    waiters: Vec<std::task::Waker>,
}

impl LiveSet {
    /// Registers a new execution; the slot removes it when the execution's shared state drops.
    pub(crate) fn enter(self: &Arc<Self>) -> LiveSlot {
        todo!()
    }

    /// Links the slot to the execution it belongs to, once the shared state exists.
    pub(crate) fn attach(&self, slot: &LiveSlot, exec: &Arc<ExecShared>) {
        todo!()
    }

    pub(crate) fn until_empty(&self) -> impl Future<Output = ()> + Send + '_ {
        async { todo!() }
    }

    /// Fires cancellation on every live execution and returns how many there were, and how many
    /// of them were terminal: the drain's end (§9.5 step 4).
    pub(crate) fn cancel_all(&self) -> (usize, usize) {
        todo!()
    }
}

/// An execution's entry in the live set.
pub(crate) struct LiveSlot {
    pub(crate) set: Arc<LiveSet>,
    pub(crate) id: u64,
}

impl Drop for LiveSlot {
    fn drop(&mut self) {
        todo!()
    }
}
