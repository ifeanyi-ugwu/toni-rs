use std::borrow::Cow;

use crate::dependency::Dependencies;
use crate::key::KeyName;
use crate::transport::Transport;
use crate::transport::enhancer::EnhancerSpec;
use crate::transport::metadata::{MetaTier, Metadata};

/// One handler, as a transport's attribute declares it to [`Mount::handler`](crate::Mount::handler)
/// (transports DESIGN §2.5, X3). Named as [`EnhancerSpec`] is.
///
/// `handler` is the transport's own handler value, which its server reads back from
/// [`MountedHandler::handler`](crate::MountedHandler::handler) when it prepares. The two
/// `EnhancerSpec` tiers default to empty, the metadata to none, the route to none and the shape
/// to [`Shape::Unary`].
///
/// `dependencies` lists what the handler's parameters read from the container, filled from each
/// parameter's `FromCall::dependencies`. The wiring pass adds those reads to its roots as it adds
/// an enhancer closure's, so a handler reading `Dep<RequestHead>` on an RPC controller, or a
/// `Dep<Foo>` its module cannot see, fails at `wire()` rather than at the first call.
pub struct HandlerSpec<T: Transport, H> {
    pub(crate) name: &'static str,
    pub(crate) handler: H,
    pub(crate) controller: EnhancerSpec<T>,
    pub(crate) method: EnhancerSpec<T>,
    pub(crate) meta: Metadata,
    pub(crate) route: Option<Cow<'static, str>>,
    pub(crate) shape: Shape,
    pub(crate) dependencies: Dependencies,
}

impl<T: Transport, H: Send + Sync + 'static> HandlerSpec<T, H> {
    /// `name` is the method's identifier without `r#`, as reports and `HandlerInfo::name` print it.
    pub fn new(name: &'static str, handler: H) -> Self {
        HandlerSpec {
            name,
            handler,
            controller: EnhancerSpec::new(),
            method: EnhancerSpec::new(),
            meta: Metadata::new(),
            route: None,
            shape: Shape::default(),
            dependencies: Dependencies::default(),
        }
    }

    /// The controller-level enhancers that apply to this handler.
    pub fn controller(mut self, spec: EnhancerSpec<T>) -> Self {
        self.controller = spec;
        self
    }

    /// The handler's own enhancers.
    pub fn method(mut self, spec: EnhancerSpec<T>) -> Self {
        self.method = spec;
        self
    }

    pub fn meta(mut self, meta: Metadata) -> Self {
        self.meta = meta;
        self
    }

    /// The route, pattern, gRPC path or event this handler answers, as the transport writes it.
    /// A `Cow`, so a path can be built at runtime.
    pub fn route(mut self, route: impl Into<Cow<'static, str>>) -> Self {
        self.route = Some(route.into());
        self
    }

    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    pub fn dependencies(mut self, dependencies: Dependencies) -> Self {
        self.dependencies = dependencies;
        self
    }
}

/// The call shape a handler's signature declares: a streamed request comes from an inbound-stream
/// parameter, a streamed reply from a stream return (transports DESIGN §5.1, §6.1).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// One request, one reply.
    #[default]
    Unary,
    /// One request, a streamed reply.
    ServerStreaming,
    /// A streamed request, one reply.
    ClientStreaming,
    /// Both streamed.
    Bidi,
}

/// A mounted handler as an enhancer and a report read it: its transport key, controller, name,
/// route, shape and declared metadata (transports DESIGN §2.5, X3).
///
/// `dispatch` sets it on the execution before the first guard runs, so a guard reads
/// `cx.exec().handler()`; [`AppHandle::handlers`](crate::AppHandle::handlers) lists every one,
/// for permission reports and generated documentation.
pub struct HandlerInfo {
    pub(crate) transport: &'static str,
    pub(crate) controller: KeyName,
    pub(crate) name: &'static str,
    pub(crate) route: Option<Cow<'static, str>>,
    pub(crate) prefix: Option<Cow<'static, str>>,
    pub(crate) shape: Shape,
    pub(crate) meta: Metadata,
}

impl HandlerInfo {
    /// The transport's key, `Transport::KEY`.
    pub fn transport(&self) -> &'static str {
        self.transport
    }

    pub fn controller(&self) -> KeyName {
        self.controller
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The route, pattern, gRPC path or event as the handler's attribute wrote it, without the
    /// controller's prefix.
    pub fn route(&self) -> Option<&str> {
        self.route.as_deref()
    }

    /// The prefix `ModuleDef::controller::<C>().at(prefix)` set on the controller, which the
    /// transport applies to every route and gateway path of it.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn metadata(&self) -> &Metadata {
        &self.meta
    }

    /// The method's declaration of `T` if there is one, otherwise the controller's: the most
    /// specific declaration wins.
    pub fn meta<T: 'static>(&self) -> Option<&T> {
        self.meta.get::<T>()
    }

    /// Both tiers' declarations of `T`, the method's first.
    pub fn meta_all<T: 'static>(&self) -> impl Iterator<Item = &T> + '_ {
        self.meta.get_all::<T>()
    }

    /// Every declaration, as (type name, tier).
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, MetaTier)> + '_ {
        self.meta.entries()
    }
}
