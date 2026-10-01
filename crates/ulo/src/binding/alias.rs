use std::marker::PhantomData;

use crate::module::def::ModuleNode;
use crate::transport::Transport;

/// `alias::<T, Q>()`: the new key `T @ Q`, waiting for the existing one in [`Alias::of`].
///
/// An alias keeps the type and changes the qualifier; both keys reach the same object. A second
/// key under another type is a coercion, written with `also_as`.
pub struct Alias<'m, T: ?Sized, Q> {
    node: &'m mut ModuleNode,
    _k: PhantomData<fn() -> (PhantomData<T>, Q)>,
}

impl<'m, T: ?Sized + Send + Sync + 'static, Q: 'static> Alias<'m, T, Q> {
    pub(crate) fn new(node: &'m mut ModuleNode) -> Self {
        Alias { node, _k: PhantomData }
    }

    /// `m.alias::<PgPool, ReadOnly>().of::<Replica>()`: `PgPool @ ReadOnly` reads the binding
    /// under `PgPool @ Replica`. An alias pointing at nothing visible is a wiring error.
    #[track_caller]
    pub fn of<Existing: 'static>(self) {
        todo!()
    }
}

/// `input::<T>()`: an execution input, waiting for the transport that seeds it in
/// [`Input::seeded_by`] (§6.4).
///
/// Inputs are app-wide and belong to transports. A keyed module declaring one, or a lazily
/// loaded module, is refused.
pub struct Input<'m, T> {
    node: &'m mut ModuleNode,
    _t: PhantomData<fn() -> T>,
}

impl<'m, T: Send + Sync + 'static> Input<'m, T> {
    pub(crate) fn new(node: &'m mut ModuleNode) -> Self {
        Input { node, _t: PhantomData }
    }

    /// The transport whose executions seed `T` with `Execution::seed`. Wiring checks every
    /// handler's reachable execution-scoped bindings against it.
    #[track_caller]
    pub fn seeded_by<Tr: Transport>(self) {
        todo!()
    }
}
