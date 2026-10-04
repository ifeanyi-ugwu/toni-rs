//! The `ulo` core: keys, bindings, injection points, scopes, executions, modules, lifecycle,
//! errors, and the enhancer traits a transport implements against.
//!
//! The core has no async runtime. It uses `std::future`, the [`BoxFuture`] alias and a pluggable
//! [`Timer`]; transports and runtime adapters bring the executor, sockets, timers and signals.
//! Every macro expands to calls on the value-level API re-exported here, which integration
//! crates call directly.

mod binding;
mod construct;
mod dependency;
mod error;
mod execution;
mod graph;
mod hooks;
mod key;
mod lifecycle;
mod module;
mod redact;
mod resolver;
mod signal;
mod timer;
mod transport;
mod type_name;

pub mod app;
pub mod scope;
pub mod testing;

/// The binding-handle typestate: the state markers `Handle`'s parameters take and the traits
/// that decide which methods exist in each state (§9.1). Also the marks on `Contribute`'s last
/// parameter: `Plain` on the builder `contribute` returns, `Enhancer` on the one `enhancer`
/// returns, which has no `qualified`.
pub mod handle {
    pub use crate::binding::contribute::{Enhancer, Plain};
    pub use crate::binding::handle::{
        AttemptTimeout, Binding, Contribution, Handle, HookHost, HookItem, Open, ReadyItem, Set,
        SingleBinding, Timeout, Unbounded,
    };
}

#[doc(hidden)]
pub mod __private;

pub use ulo_macros::{construct, error_handlers, guards, injectable, interceptors, meta, module, routes};

pub use app::{App, AppBuilder, AppHandle, Connected, Phase, Wired};
pub use binding::alias::{Alias, Input};
pub use binding::contribute::Contribute;
pub use binding::factory::{Factory, ShutdownFactory};
pub use binding::handle::Handle;
pub use construct::{Construct, ConstructError};
pub use dependency::{Dep, Dependencies, Ext, FromContainer, Many, Requirement};
pub use error::{
    Closed, ConfigureError, ConfigureErrors, ConnectError, DispatchStage, EndStream, FailureReason, GuardRejected,
    Limit, LoadError, LoadRefusal, LookupError, LookupKind, PanicRecovered, PrepareError, Shutdown, ShutdownError,
    ShutdownFailure, StartupError, TimerMissing, is_panic,
};
pub use error::wiring::{InputOrigin, WiringError, WiringErrors};
pub use execution::extensions::Extensions;
pub use execution::notify::{Cancelled, Draining};
pub use execution::{CancelReason, ExecOptions, Execution, ExecutionRef, StreamOutcome};
pub use hooks::{
    BeforeApplicationShutdown, HookKind, Hooks, OnApplicationBootstrap, OnApplicationShutdown,
    OnModuleDestroy, OnModuleInit,
};
pub use key::{BindingKind, Key, KeyName};
pub use type_name::TypeName;
pub use module::dynamic::DynamicModule;
pub use module::handle::ModuleRef;
pub use module::keyed::Keyed;
pub use module::meta::Meta;
pub use module::def::{ModuleDef, ModuleHook};
pub use module::{Module, ModuleIdentity, ModuleName};
pub use redact::{Redacted, Secret};
pub use resolver::{Entries, Entry, Resolver};
pub use scope::{AllowedIn, HookCapable, Scope, ScopeKind};
pub use signal::Signal;
pub use timer::{BoxError, BoxFuture, Bound, Timer};
pub use transport::controller::{Controller, ControllerHandle, Mount, MountedHandler};
pub use transport::enhancer::EnhancerSpec;
pub use transport::handler::{HandlerInfo, HandlerSpec, Shape};
pub use transport::inputs::Inputs;
pub use transport::metadata::{MetaTier, Metadata, TransportMetadata};
pub use transport::next::Next;
pub use transport::pipeline::{LateOutcome, dispatch, dispatch_late, recover};
pub use transport::server::{BoundAddr, DrainToken, InputReader, Mounted, Server};
pub use transport::{
    AnyErrorHandler, AnyGuard, AnyInterceptor, ErasedErrorHandler, ErasedGuard,
    ErasedInterceptor, ErrorHandler, Guard, Interceptor, Role, Transport,
};
pub use testing::TestApp;
