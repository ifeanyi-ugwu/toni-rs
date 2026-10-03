use std::any::{Any, TypeId, type_name};
use std::sync::Arc;

/// The metadata declared on one handler with `#[meta(..)]`, on the method and on its controller
/// impl (transports DESIGN §2.5).
///
/// Values are plain `Send + Sync + 'static` values, built once when the handler mounts. A guard or
/// interceptor reads them through `cx.exec().handler()`; [`HandlerInfo::meta`](crate::HandlerInfo::meta)
/// answers the method's declaration when there is one and the controller's otherwise.
///
/// The type lives in the core because the core's `HandlerSpec` and `HandlerInfo` carry it;
/// `ulo-transport` re-exports it.
#[derive(Clone, Default)]
pub struct Metadata {
    entries: Vec<MetaValue>,
}

/// Which tier declared a metadata value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetaTier {
    /// `#[meta(..)]` on the `#[routes]` impl, applying to every handler in it.
    Controller,
    /// `#[meta(..)]` on the handler method.
    Method,
}

#[derive(Clone)]
struct MetaValue {
    ty: TypeId,
    name: &'static str,
    tier: MetaTier,
    value: Arc<dyn Any + Send + Sync>,
}

impl Metadata {
    pub fn new() -> Self {
        Metadata::default()
    }

    /// A value declared on the controller impl.
    pub fn controller<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.push(MetaTier::Controller, value)
    }

    /// A value declared on the handler method.
    pub fn method<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.push(MetaTier::Method, value)
    }

    /// The most specific declaration of `T`: the method's if it has one, otherwise the
    /// controller's. Within one tier the first written answers.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.tier::<T>(MetaTier::Method).next().or_else(|| self.tier::<T>(MetaTier::Controller).next())
    }

    /// Every declaration of `T`, the method's first, each tier in the order written.
    pub fn get_all<T: 'static>(&self) -> impl Iterator<Item = &T> + '_ {
        self.tier::<T>(MetaTier::Method).chain(self.tier::<T>(MetaTier::Controller))
    }

    /// Every declaration, as its type's name and the tier that declared it, the method's first.
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, MetaTier)> + '_ {
        let method = self.entries.iter().filter(|e| e.tier == MetaTier::Method);
        let controller = self.entries.iter().filter(|e| e.tier == MetaTier::Controller);
        method.chain(controller).map(|e| (e.name, e.tier))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn push<T: Send + Sync + 'static>(&mut self, tier: MetaTier, value: T) -> &mut Self {
        self.entries.push(MetaValue { ty: TypeId::of::<T>(), name: type_name::<T>(), tier, value: Arc::new(value) });
        self
    }

    fn tier<T: 'static>(&self, tier: MetaTier) -> impl Iterator<Item = &T> + '_ {
        self.entries
            .iter()
            .filter(move |e| e.tier == tier && e.ty == TypeId::of::<T>())
            .filter_map(|e| e.value.downcast_ref::<T>())
    }
}
