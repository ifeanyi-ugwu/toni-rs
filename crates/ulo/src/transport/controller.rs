use std::any::{Any, TypeId};
use std::sync::Arc;

use crate::construct::Construct;
use crate::dependency::Dependencies;
use crate::graph::{BindingId, ModuleId};
use crate::key::{Key, KeyName};
use crate::module::handle::ModuleRef;
use crate::transport::enhancer::EnhancerSpec;
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
/// every handler's transport and enhancers before anything is built.
pub struct Mount<'a> {
    pub(crate) handlers: &'a mut Vec<HandlerDecl>,
}

impl Mount<'_> {
    /// One handler on transport `T`. `controller` holds the controller-level enhancers that
    /// apply to it, `method` its own; `handler` is the transport's own handler value, which the
    /// transport reads back from [`MountedHandler::handler`] when it binds.
    pub fn handler<T: Transport, H: Send + Sync + 'static>(
        &mut self,
        name: &'static str,
        controller: EnhancerSpec<T>,
        method: EnhancerSpec<T>,
        handler: H,
    ) {
        let mut enhancer_deps = Vec::new();
        controller.deps(&mut enhancer_deps);
        method.deps(&mut enhancer_deps);
        self.handlers.push(HandlerDecl {
            transport: TypeId::of::<T>(),
            transport_name: transport_name::<T>(),
            name,
            role_keys: [Key::of::<AnyGuard<T>, ()>(), Key::of::<AnyInterceptor<T>, ()>(), Key::of::<AnyErrorHandler<T>, ()>()],
            enhancer_deps,
            specs: Arc::new(Tiers { controller, method }),
            handler: Arc::new(handler),
        });
    }
}

/// A handler as a transport receives it when it binds: its name, its controller and module,
/// its enhancer tiers and the transport's own handler value.
pub struct MountedHandler<T: Transport> {
    pub(crate) name: &'static str,
    pub(crate) controller: KeyName,
    pub(crate) module: ModuleRef,
    pub(crate) controller_spec: EnhancerSpec<T>,
    pub(crate) method_spec: EnhancerSpec<T>,
    pub(crate) handler: Arc<dyn Any + Send + Sync>,
}

impl<T: Transport> MountedHandler<T> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The controller's key; a handler resolves its controller with
    /// `exec.get::<C>()` on an execution opened in [`module`](Self::module).
    pub fn controller(&self) -> KeyName {
        self.controller
    }

    /// The controller's module: open the call's execution here, so handlers and their
    /// per-execution services see what their module sees.
    pub fn module(&self) -> &ModuleRef {
        &self.module
    }

    /// The value passed to `Mount::handler`, by its type.
    pub fn handler<H: 'static>(&self) -> Option<&H> {
        self.handler.downcast_ref::<H>()
    }
}

/// A server receives its handlers borrowed for the length of `bind` and keeps clones to
/// dispatch with while it serves.
impl<T: Transport> Clone for MountedHandler<T> {
    fn clone(&self) -> Self {
        MountedHandler {
            name: self.name,
            controller: self.controller,
            module: self.module.clone(),
            controller_spec: self.controller_spec.clone(),
            method_spec: self.method_spec.clone(),
            handler: Arc::clone(&self.handler),
        }
    }
}

/// A controller as its module registered it: the binding and the mount function the wiring
/// pass calls.
#[derive(Clone, Copy)]
pub(crate) struct ControllerRecord {
    /// Index into the module node's `bindings`.
    pub(crate) binding: usize,
    pub(crate) mount: fn(&mut Mount<'_>),
}

/// One handler as `Mount::handler` recorded it, erased over the transport. A clone shares the
/// tiers and the handler value with the original.
#[derive(Clone)]
pub(crate) struct HandlerDecl {
    pub(crate) transport: TypeId,
    pub(crate) transport_name: &'static str,
    pub(crate) name: &'static str,
    /// `AnyGuard<T>`, `AnyInterceptor<T>`, `AnyErrorHandler<T>`: contributions under these keys
    /// are enhancers for the `Auto` rule, and the pipeline reads them through `entries`.
    pub(crate) role_keys: [Key; 3],
    /// What the wiring pass resolves for this handler beyond its controller.
    pub(crate) enhancer_deps: Vec<EnhancerDep>,
    /// The two `EnhancerSpec<T>` tiers, controller then method.
    pub(crate) specs: Arc<dyn Any + Send + Sync>,
    pub(crate) handler: Arc<dyn Any + Send + Sync>,
}

#[derive(Clone)]
pub(crate) enum EnhancerDep {
    /// A by-type declaration: the enhancer's own key.
    Type(Key),
    /// A closure declaration's dependencies. The closure builds its enhancer per execution.
    Closure(Arc<Dependencies>),
}

/// A handler in the frozen graph.
#[derive(Clone)]
pub(crate) struct HandlerRecord {
    pub(crate) controller: BindingId,
    pub(crate) module: ModuleId,
    pub(crate) decl: HandlerDecl,
}

impl HandlerRecord {
    /// The handlers of transport `T`, typed again, for `Server::bind`.
    pub(crate) fn mounted<T: Transport>(&self, module: ModuleRef, controller: KeyName) -> Option<MountedHandler<T>> {
        let tiers = self.decl.specs.downcast_ref::<Tiers<T>>()?;
        Some(MountedHandler {
            name: self.decl.name,
            controller,
            module,
            controller_spec: tiers.controller.clone(),
            method_spec: tiers.method.clone(),
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
