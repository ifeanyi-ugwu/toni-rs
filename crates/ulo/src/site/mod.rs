//! Injection sites: the one rule every field, constructor parameter, factory parameter and
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

/// How every injection point is read. A site is its type: the container reads `T @ Q` for
/// `Dep<T, Q>`, whichever identifier or alias the user wrote.
///
/// The core implements it for [`Dep`], [`Many`], [`Ext`], `Option<S>`,
/// [`ModuleRef`](crate::ModuleRef) and [`ExecutionRef`](crate::ExecutionRef). An integration
/// crate adds its own, as `ulo-ws` does with `Session<T>`, by describing the keys it reads and
/// reading them through the [`Resolver`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an injection site",
    label = "the container cannot provide this",
    note = "write `Dep<{Self}>` to share the container's instance",
    note = "or set it inside the #[construct] fn / mark the field #[injectable(default)]"
)]
pub trait Site: Sized + Send + 'static {
    /// Static description used by the wiring pass: key, kind, optional, needs-execution.
    fn describe(d: &mut SiteDesc);

    /// Read the site from the container.
    fn read(r: &Resolver<'_>) -> impl Future<Output = Result<Self, LookupError>> + Send;
}

/// What one site reads, written by [`Site::describe`] and checked by the wiring pass against
/// the visibility of the module the site belongs to.
///
/// A site may read several keys. An execution input is described with [`SiteDesc::dep`]: the
/// graph knows which keys are inputs (§6.4).
#[derive(Default)]
pub struct SiteDesc {
    pub(crate) reads: Vec<SiteRead>,
}

impl SiteDesc {
    /// A single binding, or an execution input, under `key`.
    pub fn dep(&mut self, key: Key) {
        self.reads.push(SiteRead { kind: ReadKind::Single(key), optional: false });
    }

    /// Every contribution to the collection under `key`, from every module (§8.2).
    pub fn many(&mut self, key: Key) {
        self.reads.push(SiteRead { kind: ReadKind::Collection(key), optional: false });
    }

    /// The extension `T` of the current execution. Needs an execution.
    pub fn ext<T: 'static>(&mut self) {
        self.reads.push(SiteRead { kind: ReadKind::Extension(Key::of::<T, ()>()), optional: false });
    }

    /// The current execution itself. Needs an execution.
    pub fn execution(&mut self) {
        self.reads.push(SiteRead { kind: ReadKind::Execution, optional: false });
    }

    /// The enclosing module.
    pub fn module(&mut self) {
        self.reads.push(SiteRead { kind: ReadKind::Module, optional: false });
    }

    /// Everything `S` reads, marked optional: a key no module binds is not a wiring error, and
    /// the read answers `None` where `S` would fail with `LookupError::NotFound`.
    pub fn optional<S: Site>(&mut self) {
        let start = self.reads.len();
        S::describe(self);
        for read in &mut self.reads[start..] {
            read.optional = true;
        }
    }
}

pub(crate) struct SiteRead {
    pub(crate) kind: ReadKind,
    pub(crate) optional: bool,
}

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

/// The declared sites of a binding, a hook closure, a readiness closure or an enhancer closure:
/// what [`Construct::sites`](crate::Construct::sites) and the factory traits fill in, and what
/// the wiring pass resolves. `sites()` is the single source of truth for dependencies.
#[derive(Default)]
pub struct Sites {
    pub(crate) list: Vec<SiteRecord>,
}

impl Sites {
    /// A struct field, named in diagnostics as ``field `name` ``.
    pub fn field<S: Site>(&mut self, name: &'static str) -> &mut Self {
        self.push::<S>(SiteLabel::Field(name))
    }

    /// A constructor parameter, named in diagnostics as ``param `name` ``.
    pub fn param<S: Site>(&mut self, name: &'static str) -> &mut Self {
        self.push::<S>(SiteLabel::Param(name))
    }

    /// An unnamed site, such as a closure parameter, named in diagnostics by its position.
    pub fn site<S: Site>(&mut self) -> &mut Self {
        let position = self.list.len();
        self.push::<S>(SiteLabel::Position(position))
    }

    fn push<S: Site>(&mut self, label: SiteLabel) -> &mut Self {
        let mut desc = SiteDesc::default();
        S::describe(&mut desc);
        self.list.push(SiteRecord { label, type_name: std::any::type_name::<S>(), desc });
        self
    }

    pub(crate) fn needs_execution_directly(&self) -> bool {
        self.list.iter().flat_map(|s| &s.desc.reads).any(|r| r.kind.needs_execution())
    }
}

pub(crate) struct SiteRecord {
    pub(crate) label: SiteLabel,
    /// `type_name` of the site type, for the `Dep<RequestHead> (field `head`)` step of a path.
    pub(crate) type_name: &'static str,
    pub(crate) desc: SiteDesc,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SiteLabel {
    Field(&'static str),
    Param(&'static str),
    Position(usize),
}
