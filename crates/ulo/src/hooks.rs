//! Lifecycle hooks: the five traits a constructed type implements, the `Hooks<T>` registry
//! `Construct::hooks` fills, and the erased record the lifecycle runner executes (§3.5, §9.1).
//!
//! Each trait carries its own `TIMEOUT`, so a type implementing two hooks bounds each
//! separately. To read one on a type implementing several, write it fully qualified,
//! `<T as OnModuleInit>::TIMEOUT`; `Self::TIMEOUT` is ambiguous there.
//!
//! The `Construct<Scope: HookCapable>` supertrait makes a hook on an explicitly execution-scoped
//! or transient type a compile error.

use std::any::type_name;
use std::fmt;
use std::future::Future;
use std::panic::Location;
use std::sync::Arc;

use crate::binding::{Instance, downcast_instance};
use crate::construct::Construct;
use crate::dependency::Dependencies;
use crate::resolver::Resolver;
use crate::scope::HookCapable;
use crate::signal::Signal;
use crate::timer::{BoxError, BoxFuture, Bound};
use crate::type_name::short_type_name;

pub trait OnModuleInit: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

pub trait OnApplicationBootstrap: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_bootstrap(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

pub trait OnModuleDestroy: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_module_destroy(&self) -> impl Future<Output = ()> + Send;
}

pub trait BeforeApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn before_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}

pub trait OnApplicationShutdown: Construct<Scope: HookCapable> {
    const TIMEOUT: Bound = Bound::Default;
    fn on_application_shutdown(&self, signal: &Signal) -> impl Future<Output = ()> + Send;
}

/// Which hook a `ConnectError::Hook` or `ShutdownFailure::Hook` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookKind {
    OnModuleInit,
    OnApplicationBootstrap,
    OnModuleDestroy,
    BeforeApplicationShutdown,
    OnApplicationShutdown,
}

impl fmt::Display for HookKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HookKind::OnModuleInit => "OnModuleInit",
            HookKind::OnApplicationBootstrap => "OnApplicationBootstrap",
            HookKind::OnModuleDestroy => "OnModuleDestroy",
            HookKind::BeforeApplicationShutdown => "BeforeApplicationShutdown",
            HookKind::OnApplicationShutdown => "OnApplicationShutdown",
        })
    }
}

/// The hooks a [`Construct`] type registers. Each method compiles only when `T` implements the
/// matching trait; a hand-written `Construct` impl calls the ones it implemented, and
/// [`hooks!`](crate::hooks!) probes all five.
pub struct Hooks<T> {
    pub(crate) slots: Vec<TraitHook<T>>,
}

impl<T> Hooks<T> {
    pub(crate) fn new() -> Self {
        Hooks { slots: Vec::new() }
    }

    pub fn on_module_init(&mut self)
    where
        T: OnModuleInit,
    {
        self.slots.push(TraitHook {
            kind: HookKind::OnModuleInit,
            bound: <T as OnModuleInit>::TIMEOUT,
            run: TraitHookFn::Startup(Box::new(|t| Box::pin(t.on_module_init()))),
        });
    }

    pub fn on_application_bootstrap(&mut self)
    where
        T: OnApplicationBootstrap,
    {
        self.slots.push(TraitHook {
            kind: HookKind::OnApplicationBootstrap,
            bound: <T as OnApplicationBootstrap>::TIMEOUT,
            run: TraitHookFn::Startup(Box::new(|t| Box::pin(t.on_application_bootstrap()))),
        });
    }

    pub fn on_module_destroy(&mut self)
    where
        T: OnModuleDestroy,
    {
        self.slots.push(TraitHook {
            kind: HookKind::OnModuleDestroy,
            bound: <T as OnModuleDestroy>::TIMEOUT,
            run: TraitHookFn::Destroy(Box::new(|t| Box::pin(t.on_module_destroy()))),
        });
    }

    pub fn before_application_shutdown(&mut self)
    where
        T: BeforeApplicationShutdown,
    {
        self.slots.push(TraitHook {
            kind: HookKind::BeforeApplicationShutdown,
            bound: <T as BeforeApplicationShutdown>::TIMEOUT,
            run: TraitHookFn::Signalled(Box::new(|t, s| Box::pin(t.before_application_shutdown(s)))),
        });
    }

    pub fn on_application_shutdown(&mut self)
    where
        T: OnApplicationShutdown,
    {
        self.slots.push(TraitHook {
            kind: HookKind::OnApplicationShutdown,
            bound: <T as OnApplicationShutdown>::TIMEOUT,
            run: TraitHookFn::Signalled(Box::new(|t, s| Box::pin(t.on_application_shutdown(s)))),
        });
    }
}

pub(crate) struct TraitHook<T> {
    pub(crate) kind: HookKind,
    pub(crate) bound: Bound,
    pub(crate) run: TraitHookFn<T>,
}

pub(crate) enum TraitHookFn<T> {
    Startup(Box<dyn for<'a> Fn(&'a T) -> BoxFuture<'a, Result<(), BoxError>> + Send + Sync>),
    Destroy(Box<dyn for<'a> Fn(&'a T) -> BoxFuture<'a, ()> + Send + Sync>),
    Signalled(Box<dyn for<'a> Fn(&'a T, &'a Signal) -> BoxFuture<'a, ()> + Send + Sync>),
}

/// One hook as the lifecycle runner executes it, erased over the instance type: a trait hook
/// from `Construct::hooks`, a closure hook from a binding handle, or a module's own hook.
#[derive(Clone)]
pub(crate) struct HookRecord {
    pub(crate) kind: HookKind,
    pub(crate) bound: Bound,
    pub(crate) run: HookFn,
    /// The closure's dependencies, resolved by the wiring pass like any binding's. Empty for a
    /// trait hook.
    pub(crate) dependencies: Dependencies,
    pub(crate) location: &'static Location<'static>,
}

/// Shutdown hooks return `()`, and the erased form answers `Ok(())` for them once they ran. Its
/// `Err` from a destroy, before-shutdown or shutdown hook means the hook never ran: a closure
/// hook's dependency read failed, or the runner handed it no instance or no signal.
pub(crate) type HookFn = Arc<dyn for<'a> Fn(HookCx<'a>) -> BoxFuture<'a, Result<(), BoxError>> + Send + Sync>;

/// Fixes a closure's signature to [`HookFn`]'s higher-ranked one, which a closure only gets from
/// a bound it is passed to.
pub(crate) fn hook_fn<H>(hook: H) -> HookFn
where
    H: for<'a> Fn(HookCx<'a>) -> BoxFuture<'a, Result<(), BoxError>> + Send + Sync + 'static,
{
    Arc::new(hook)
}

pub(crate) struct HookCx<'a> {
    /// Reads with `Purpose::Lifecycle`, in the module the hook belongs to.
    pub(crate) resolver: &'a Resolver<'a>,
    /// The binding's own instance; `None` for a module hook.
    pub(crate) instance: Option<&'a Instance>,
    /// Present for `BeforeApplicationShutdown` and `OnApplicationShutdown`.
    pub(crate) signal: Option<&'a Signal>,
}

/// The trait hooks `T::hooks` registered, erased into records the binding carries.
pub(crate) fn erase_trait_hooks<T: Construct>(location: &'static Location<'static>) -> Vec<HookRecord> {
    let mut hooks = Hooks::<T>::new();
    T::hooks(&mut hooks);
    hooks
        .slots
        .into_iter()
        .map(|hook| HookRecord {
            kind: hook.kind,
            bound: hook.bound,
            run: erase_trait_hook(hook.kind, hook.run),
            dependencies: Dependencies::default(),
            location,
        })
        .collect()
}

fn erase_trait_hook<T: Construct>(kind: HookKind, run: TraitHookFn<T>) -> HookFn {
    let run = Arc::new(run);
    hook_fn(move |cx| {
        let run = Arc::clone(&run);
        let HookCx { instance, signal, .. } = cx;
        let instance = instance.and_then(downcast_instance::<T>);
        Box::pin(async move {
            let Some(instance) = instance else {
                return Err(not_run::<T>(kind, "instance"));
            };
            match &*run {
                TraitHookFn::Startup(hook) => hook(&*instance).await,
                TraitHookFn::Destroy(hook) => {
                    hook(&*instance).await;
                    Ok(())
                }
                TraitHookFn::Signalled(hook) => {
                    let Some(signal) = signal else {
                        return Err(not_run::<T>(kind, "shutdown signal"));
                    };
                    hook(&*instance, signal).await;
                    Ok(())
                }
            }
        })
    })
}

/// The lifecycle runner always passes a trait hook its instance and a shutdown hook its signal;
/// a record run without one reports this rather than panicking.
fn not_run<T>(kind: HookKind, missing: &str) -> BoxError {
    format!("the {kind} hook of `{}` did not run: it was handed no {missing}", short_type_name(type_name::<T>())).into()
}

/// `Construct::hooks` filled by autoref probing: each of the five hooks registers when the
/// concrete type implements its trait and is a no-op otherwise. `#[injectable]` emits the same
/// probes; this macro offers them to a hand-written `Construct` impl that wants all five checked.
///
/// ```ignore
/// fn hooks(h: &mut ulo::Hooks<Self>) { ulo::hooks!(h); }
/// ```
#[macro_export]
macro_rules! hooks {
    ($h:expr) => {{
        #[allow(unused_imports)]
        use $crate::__private::hooks::{
            NoBootstrap as _, NoBeforeShutdown as _, NoDestroy as _, NoInit as _, NoShutdown as _,
            RegisterBootstrap as _, RegisterBeforeShutdown as _, RegisterDestroy as _,
            RegisterInit as _, RegisterShutdown as _,
        };
        let h = $h;
        (&$crate::__private::hooks::Probe::of(&*h)).register_init(&mut *h);
        (&$crate::__private::hooks::Probe::of(&*h)).register_bootstrap(&mut *h);
        (&$crate::__private::hooks::Probe::of(&*h)).register_destroy(&mut *h);
        (&$crate::__private::hooks::Probe::of(&*h)).register_before_shutdown(&mut *h);
        (&$crate::__private::hooks::Probe::of(&*h)).register_shutdown(&mut *h);
    }};
}
