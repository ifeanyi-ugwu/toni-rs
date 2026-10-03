use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::HashSet;
use std::panic::Location;
use std::sync::Arc;

use crate::construct::Construct;
use crate::dependency::Dependencies;
use crate::graph::{BindingId, ModuleId};
use crate::key::{Key, KeyName};
use crate::module::handle::ModuleRef;
use crate::scope::ScopeKind;
use crate::transport::enhancer::{ClosureId, EnhancerSpec};
use crate::transport::handler::{HandlerInfo, HandlerSpec, Shape};
use crate::transport::inputs::Inputs;
use crate::transport::metadata::Metadata;
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, Transport, transport_name};

/// A dispatch target: a `Construct` type whose handlers `mount` declares, one per transport
/// route, message pattern, gRPC method or WebSocket event. `#[routes]` writes the impl.
///
/// A controller takes part in the container like any binding. With `Auto` scope it is a
/// singleton if nothing below it needs an execution and is built per call otherwise.
pub trait Controller: Construct {
    fn mount(m: &mut Mount<'_>);
}

/// Where a controller declares its handlers. Called during `wire()`, so the wiring pass knows
/// every handler's transport, enhancers and parameter reads before anything is built.
pub struct Mount<'a> {
    pub(crate) handlers: &'a mut Vec<HandlerDecl>,
    /// The `K`s [`once`](Self::once) has run for during this controller's mount.
    pub(crate) once: HashSet<TypeId>,
}

impl<'a> Mount<'a> {
    pub(crate) fn new(handlers: &'a mut Vec<HandlerDecl>) -> Self {
        Mount { handlers, once: HashSet::new() }
    }

    /// One handler, declared through its [`HandlerSpec`]. Records the three role keys of `T`,
    /// whose contributions the freeze treats as enhancers, and `T::inputs`, which the freeze calls
    /// once per transport type.
    pub fn handler<T: Transport, H: Send + Sync + 'static>(&mut self, spec: HandlerSpec<T, H>) {
        let HandlerSpec { name, handler, controller, method, meta, route, shape, dependencies } = spec;
        let mut enhancer_deps = Vec::new();
        controller.deps("controller", &mut enhancer_deps);
        method.deps("method", &mut enhancer_deps);
        self.handlers.push(HandlerDecl {
            transport: TypeId::of::<T>(),
            transport_name: transport_name::<T>(),
            key: T::KEY,
            inputs: T::inputs,
            name,
            role_keys: [Key::of::<AnyGuard<T>, ()>(), Key::of::<AnyInterceptor<T>, ()>(), Key::of::<AnyErrorHandler<T>, ()>()],
            enhancer_deps,
            dependencies: Arc::new(dependencies),
            route,
            shape,
            meta,
            specs: Arc::new(Tiers { controller, method }),
            handler: Arc::new(handler),
        });
    }

    /// Runs `f` the first time this controller's mount calls `once::<K>`, and never again during
    /// it (transports DESIGN §4.1, X15). A WebSocket gateway's message handlers each call
    /// `m.once::<Self>(|m| <Self as GatewayConfig>::mount_gateway(m))`, so the gateway's connect
    /// handler mounts once whichever message handler mounts first.
    pub fn once<K: ?Sized + 'static>(&mut self, f: impl FnOnce(&mut Mount<'_>)) {
        if self.once.insert(TypeId::of::<K>()) {
            f(self);
        }
    }
}

/// A handler as a transport receives it when it prepares and binds: its name, its controller and
/// module, its enhancer tiers, its [`HandlerInfo`] and the transport's own handler value.
pub struct MountedHandler<T: Transport> {
    pub(crate) name: &'static str,
    pub(crate) controller: KeyName,
    pub(crate) module: ModuleRef,
    pub(crate) controller_spec: EnhancerSpec<T>,
    pub(crate) method_spec: EnhancerSpec<T>,
    pub(crate) info: Arc<HandlerInfo>,
    pub(crate) handler: Arc<dyn Any + Send + Sync>,
}

impl<T: Transport> MountedHandler<T> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The controller's key; a handler resolves its controller with `exec.get::<C>()` on an
    /// execution routed to [`module`](Self::module).
    pub fn controller(&self) -> KeyName {
        self.controller
    }

    /// The controller's module: open the call's execution here, or route an execution opened at
    /// the root here with `Execution::route_to`, so handlers and their per-execution services see
    /// what their module sees.
    pub fn module(&self) -> &ModuleRef {
        &self.module
    }

    /// What `dispatch` sets on the execution: route, prefix, shape and metadata.
    pub fn info(&self) -> &Arc<HandlerInfo> {
        &self.info
    }

    /// The value passed to `HandlerSpec::new`, by its type.
    pub fn handler<H: 'static>(&self) -> Option<&H> {
        self.handler.downcast_ref::<H>()
    }
}

/// A server receives its handlers borrowed for the length of `prepare` and `bind` and keeps
/// clones to dispatch with while it serves.
impl<T: Transport> Clone for MountedHandler<T> {
    fn clone(&self) -> Self {
        MountedHandler {
            name: self.name,
            controller: self.controller,
            module: self.module.clone(),
            controller_spec: self.controller_spec.clone(),
            method_spec: self.method_spec.clone(),
            info: Arc::clone(&self.info),
            handler: Arc::clone(&self.handler),
        }
    }
}

/// What [`ModuleDef::controller`](crate::ModuleDef::controller) returns: the controller it
/// registered, whose routes and gateway paths take a runtime prefix.
pub struct ControllerHandle<'m> {
    pub(crate) record: &'m mut ControllerRecord,
}

impl ControllerHandle<'_> {
    /// A prefix applied to every route and gateway path of this controller, read from
    /// configuration at registration: a GraphQL or health endpoint mounted where the application
    /// says (transports DESIGN §2.5, X3). The core stores it on each handler's [`HandlerInfo`];
    /// the transport joins it to the route by its own path rules.
    pub fn at(self, prefix: impl Into<Cow<'static, str>>) {
        self.record.prefix = Some(prefix.into());
    }
}

/// A controller as its module registered it: the binding, the mount function the wiring pass
/// calls, and the prefix `.at(..)` set.
#[derive(Clone)]
pub(crate) struct ControllerRecord {
    /// Index into the module node's `bindings`.
    pub(crate) binding: usize,
    pub(crate) mount: fn(&mut Mount<'_>),
    pub(crate) prefix: Option<Cow<'static, str>>,
}

/// One handler as `Mount::handler` recorded it, erased over the transport. A clone shares the
/// tiers, the handler value and the parameter reads with the original.
#[derive(Clone)]
pub(crate) struct HandlerDecl {
    pub(crate) transport: TypeId,
    pub(crate) transport_name: &'static str,
    /// `Transport::KEY`.
    pub(crate) key: &'static str,
    /// `Transport::inputs`, which the freeze calls the first time a handler of this transport
    /// mounts.
    pub(crate) inputs: fn(&mut Inputs),
    pub(crate) name: &'static str,
    /// `AnyGuard<T>`, `AnyInterceptor<T>`, `AnyErrorHandler<T>`: contributions under these keys
    /// are enhancers for the `Auto` rule, and the pipeline reads them through `entries`.
    pub(crate) role_keys: [Key; 3],
    /// What the wiring pass resolves for this handler's enhancers, beyond its controller.
    pub(crate) enhancer_deps: Vec<EnhancerDep>,
    /// What the handler's own parameters read from the container: roots of the wiring walk, as an
    /// enhancer closure's parameters are, and of the per-handler input check.
    pub(crate) dependencies: Arc<Dependencies>,
    pub(crate) route: Option<Cow<'static, str>>,
    pub(crate) shape: Shape,
    pub(crate) meta: Metadata,
    /// The two `EnhancerSpec<T>` tiers, controller then method.
    pub(crate) specs: Arc<dyn Any + Send + Sync>,
    pub(crate) handler: Arc<dyn Any + Send + Sync>,
}

#[derive(Clone)]
pub(crate) enum EnhancerDep {
    /// A by-type declaration: the enhancer's own key.
    Type(Key),
    Closure(ClosureDep),
}

/// A closure declaration as the wiring pass reads it: what its parameters read, the scope it
/// declares, and where a report finds it.
#[derive(Clone)]
pub(crate) struct ClosureDep {
    /// The key `Graph::closures` records the decided scope under.
    pub(crate) id: ClosureId,
    pub(crate) scope: ScopeKind,
    pub(crate) dependencies: Arc<Dependencies>,
    /// `guard`, `interceptor` or `error handler`.
    pub(crate) role: &'static str,
    /// `controller` or `method`.
    pub(crate) tier: &'static str,
    /// From 1, among the tier's declarations of the role, every form counted.
    pub(crate) position: usize,
    pub(crate) location: &'static Location<'static>,
}

/// A handler in the frozen graph.
#[derive(Clone)]
pub(crate) struct HandlerRecord {
    pub(crate) controller: BindingId,
    pub(crate) module: ModuleId,
    pub(crate) decl: HandlerDecl,
    /// Built at freeze from the declaration, the controller's key and its `.at(..)` prefix.
    pub(crate) info: Arc<HandlerInfo>,
}

impl HandlerRecord {
    /// The record of `decl`, mounted by the controller `controller` in `module` under `prefix`.
    pub(crate) fn new(
        controller: BindingId,
        controller_key: KeyName,
        module: ModuleId,
        prefix: Option<Cow<'static, str>>,
        decl: HandlerDecl,
    ) -> Self {
        let info = Arc::new(HandlerInfo {
            transport: decl.key,
            controller: controller_key,
            name: decl.name,
            route: decl.route.clone(),
            prefix,
            shape: decl.shape,
            meta: decl.meta.clone(),
        });
        HandlerRecord { controller, module, decl, info }
    }

    /// The handlers of transport `T`, typed again, for `Server::prepare` and `Server::bind`.
    pub(crate) fn mounted<T: Transport>(&self, module: ModuleRef, controller: KeyName) -> Option<MountedHandler<T>> {
        let tiers = self.decl.specs.downcast_ref::<Tiers<T>>()?;
        Some(MountedHandler {
            name: self.decl.name,
            controller,
            module,
            controller_spec: tiers.controller.clone(),
            method_spec: tiers.method.clone(),
            info: Arc::clone(&self.info),
            handler: Arc::clone(&self.decl.handler),
        })
    }
}

/// What `HandlerDecl::specs` holds. Its `TypeId` names the transport, so a downcast to another
/// transport's tiers answers `None`.
struct Tiers<T: Transport> {
    controller: EnhancerSpec<T>,
    method: EnhancerSpec<T>,
}
