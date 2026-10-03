//! Injection points: the one rule every field, constructor parameter, factory parameter and
//! closure parameter follows (§3.2, §5).

mod dep;
mod ext;
mod handles;
mod many;
mod option;

use std::future::Future;

use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;

pub use dep::Dep;
pub use ext::Ext;
pub use many::Many;

/// How every injection point is read. Its type decides what it reads: the container reads
/// `T @ Q` for `Dep<T, Q>`, whichever identifier or alias the user wrote.
///
/// The core implements it for [`Dep`], [`Many`], [`Ext`], `Option<S>`,
/// [`ModuleRef`](crate::ModuleRef) and [`ExecutionRef`](crate::ExecutionRef). An integration
/// crate adds its own, as `ulo-ws` does with `Session<T>`, by describing the keys it reads and
/// reading them through the [`Resolver`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be obtained from the container",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[injectable(default)]"
)]
pub trait FromContainer: Sized + Send + 'static {
    /// Static description used by the wiring pass: key, kind, optional, needs-execution.
    fn describe(req: &mut Requirement);

    /// Read the value from the container.
    fn from_container(r: &Resolver<'_>) -> impl Future<Output = Result<Self, LookupError>> + Send;
}

/// What one injection point reads, written by [`FromContainer::describe`] and checked by the
/// wiring pass against the visibility of the module the injection point belongs to.
///
/// An injection point may read several keys. An execution input is described with
/// [`Requirement::dep`]: the graph knows which keys are inputs (§6.4).
#[derive(Clone, Default)]
pub struct Requirement {
    pub(crate) reads: Vec<Read>,
}

impl Requirement {
    /// A single binding, or an execution input, under `key`.
    pub fn dep(&mut self, key: Key) {
        self.reads.push(Read { kind: ReadKind::Single(key), optional: false });
    }

    /// Every contribution to the collection under `key`, from every module (§8.2).
    pub fn many(&mut self, key: Key) {
        self.reads.push(Read { kind: ReadKind::Collection(key), optional: false });
    }

    /// The extension `T` of the current execution. Needs an execution.
    pub fn ext<T: 'static>(&mut self) {
        self.reads.push(Read { kind: ReadKind::Extension(Key::of::<T, ()>()), optional: false });
    }

    /// The current execution itself. Needs an execution.
    pub fn execution(&mut self) {
        self.reads.push(Read { kind: ReadKind::Execution, optional: false });
    }

    /// The enclosing module.
    pub fn module(&mut self) {
        self.reads.push(Read { kind: ReadKind::Module, optional: false });
    }

    /// Everything `S` reads, marked optional: a key no module binds is not a wiring error, and
    /// the read answers `None` where `S` would fail with `LookupError::NotFound`.
    pub fn optional<S: FromContainer>(&mut self) {
        let start = self.reads.len();
        S::describe(self);
        for read in &mut self.reads[start..] {
            read.optional = true;
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Read {
    pub(crate) kind: ReadKind,
    pub(crate) optional: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum ReadKind {
    Single(Key),
    Collection(Key),
    /// Keyed by the extension's type, unqualified.
    Extension(Key),
    Execution,
    Module,
}

impl ReadKind {
    pub(crate) fn needs_execution(&self) -> bool {
        matches!(self, ReadKind::Extension(_) | ReadKind::Execution)
    }
}

/// The declared injection points of a binding, a hook closure, a readiness closure or an
/// enhancer closure: what [`Construct::dependencies`](crate::Construct::dependencies) and the
/// factory traits fill in, and what the wiring pass resolves. The wiring pass learns what a
/// binding reads from this list alone.
#[derive(Clone, Default)]
pub struct Dependencies {
    pub(crate) list: Vec<DependencyRecord>,
}

impl Dependencies {
    /// A struct field, named in diagnostics as ``field `name` ``.
    pub fn field<S: FromContainer>(&mut self, name: &'static str) -> &mut Self {
        self.push::<S>(DependencyLabel::Field(name))
    }

    /// A constructor parameter, named in diagnostics as ``param `name` ``.
    pub fn param<S: FromContainer>(&mut self, name: &'static str) -> &mut Self {
        self.push::<S>(DependencyLabel::Param(name))
    }

    /// An unnamed injection point, such as a closure parameter, named in diagnostics by its
    /// position.
    pub fn add<S: FromContainer>(&mut self) -> &mut Self {
        let position = self.list.len();
        self.push::<S>(DependencyLabel::Position(position))
    }

    fn push<S: FromContainer>(&mut self, label: DependencyLabel) -> &mut Self {
        let mut requirement = Requirement::default();
        S::describe(&mut requirement);
        self.list.push(DependencyRecord { label, type_name: std::any::type_name::<S>(), requirement });
        self
    }

    pub(crate) fn needs_execution_directly(&self) -> bool {
        self.list.iter().flat_map(|d| &d.requirement.reads).any(|r| r.kind.needs_execution())
    }
}

#[derive(Clone)]
pub(crate) struct DependencyRecord {
    pub(crate) label: DependencyLabel,
    /// `type_name` of the injection point's type, for the `Dep<RequestHead> (field `head`)` step
    /// of a path.
    pub(crate) type_name: &'static str,
    pub(crate) requirement: Requirement,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DependencyLabel {
    Field(&'static str),
    Param(&'static str),
    Position(usize),
}
