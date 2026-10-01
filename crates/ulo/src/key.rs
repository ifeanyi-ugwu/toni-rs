use std::any::{TypeId, type_name};
use std::fmt;
use std::hash::{Hash, Hasher};

/// The pair of a type and a qualifier that a binding is stored under: `Key::of::<PgPool, Replica>()`.
///
/// Two keys compare on their `TypeId`s alone, so a type alias or a renamed import reads the same
/// binding. A qualifier is any `'static` type, usually a unit struct; `()` means unqualified.
/// An integration crate holding a `Key` at runtime looks it up erased through
/// [`Resolver::by_key`](crate::Resolver::by_key).
#[derive(Clone, Copy)]
pub struct Key {
    ty: TypeId,
    qualifier: TypeId,
    ty_name: &'static str,
    q_name: &'static str,
}

impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key {
        Key {
            ty: TypeId::of::<T>(),
            qualifier: TypeId::of::<Q>(),
            ty_name: type_name::<T>(),
            q_name: type_name::<Q>(),
        }
    }

    /// The same type under another qualifier: `qualified::<Q>()` on a handle, and the
    /// requalification of a keyed module's exports (§8.3).
    pub(crate) fn requalified<Q: 'static>(self) -> Key {
        Key { qualifier: TypeId::of::<Q>(), q_name: type_name::<Q>(), ..self }
    }

    /// Requalification by a qualifier known only as a runtime value, which is how a `Keyed`
    /// module's qualifier reaches its export boundary.
    pub(crate) fn with_qualifier(self, qualifier: TypeId, q_name: &'static str) -> Key {
        Key { qualifier, q_name, ..self }
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.ty
    }

    pub(crate) fn qualifier_id(&self) -> TypeId {
        self.qualifier
    }

    pub(crate) fn type_name(&self) -> &'static str {
        self.ty_name
    }

    /// `None` for the unqualified `()`.
    pub(crate) fn qualifier_name(&self) -> Option<&'static str> {
        (self.qualifier != TypeId::of::<()>()).then_some(self.q_name)
    }

    pub(crate) fn is_unqualified(&self) -> bool {
        self.qualifier == TypeId::of::<()>()
    }

    pub(crate) fn name(&self, kind: BindingKind) -> KeyName {
        KeyName { key: *self, kind }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty && self.qualifier == other.qualifier
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ty.hash(state);
        self.qualifier.hash(state);
    }
}

/// `PgPool` or `PgPool @ Replica`. A collection's `(collection)` suffix is written by
/// [`KeyName`], which carries the kind a bare `Key` does not.
impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Whether a key holds one binding or a collection of contributions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingKind {
    Single,
    Collection,
}

/// A key as the errors name it: `PgPool`, `PgPool @ Replica`, `dyn Plugin (collection)`.
///
/// Type names are shortened for display; the key itself, with its `TypeId`s, is reachable
/// through [`KeyName::key`], so a caller can compare it with `Key::of::<T, Q>()`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyName {
    key: Key,
    kind: BindingKind,
}

impl KeyName {
    pub fn key(&self) -> Key {
        self.key
    }

    pub fn kind(&self) -> BindingKind {
        self.kind
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl fmt::Debug for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The diagnostic spelling of a `type_name`: module paths stripped from every segment, so
/// `alloc::sync::Arc<my_app::db::PgPool>` reads `Arc<PgPool>`. Shared by `Key`, `KeyName`,
/// `ModuleName` and every wiring report.
pub(crate) fn short_type_name(full: &'static str) -> String {
    todo!()
}
