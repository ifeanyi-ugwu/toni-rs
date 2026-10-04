use std::any::TypeId;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::type_name::TypeName;

/// The pair of a type and a qualifier that a binding is stored under: `Key::of::<PgPool, Replica>()`.
///
/// Two keys compare on their `TypeId`s alone, so a type alias or a renamed import reads the same
/// binding. A qualifier is any `'static` type, usually a unit struct; `()` means unqualified.
/// An integration crate holding a `Key` at runtime looks it up erased through
/// [`Resolver::by_key`](crate::Resolver::by_key).
///
/// `Display` writes a key through the [`TypeName`]s of its type and its qualifier, and a report
/// decides full paths per `TypeName`, not per key: `Store @ a::Replica` beside
/// `Store @ b::Replica`. [`Key::type_name`] is the key's type.
#[derive(Clone, Copy)]
pub struct Key {
    ty: TypeName,
    qualifier: TypeName,
}

impl Key {
    pub fn of<T: ?Sized + 'static, Q: 'static>() -> Key {
        Key { ty: TypeName::of::<T>(), qualifier: TypeName::of::<Q>() }
    }

    /// The type the key binds, without its qualifier.
    pub fn type_name(&self) -> TypeName {
        self.ty
    }

    /// The same type under another qualifier: `qualified::<Q>()` on a handle, and the
    /// requalification of a keyed module's exports (§8.3).
    pub(crate) fn requalified<Q: 'static>(self) -> Key {
        Key { qualifier: TypeName::of::<Q>(), ..self }
    }

    /// Requalification by a qualifier known only as a runtime value, which is how a `Keyed`
    /// module's qualifier reaches its export boundary.
    pub(crate) fn with_qualifier(self, qualifier: TypeId, q_name: &'static str) -> Key {
        Key { qualifier: TypeName::from_parts(qualifier, q_name), ..self }
    }

    /// A key rebuilt from ids held as runtime values, as a module's identity holds its type and
    /// a keyed module its qualifier.
    pub(crate) fn from_parts(
        ty: TypeId,
        ty_name: &'static str,
        qualifier: TypeId,
        q_name: &'static str,
    ) -> Key {
        Key { ty: TypeName::from_parts(ty, ty_name), qualifier: TypeName::from_parts(qualifier, q_name) }
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.ty.id()
    }

    pub(crate) fn qualifier_id(&self) -> TypeId {
        self.qualifier.id()
    }

    /// `None` for the unqualified `()`.
    pub(crate) fn qualifier(&self) -> Option<TypeName> {
        (!self.is_unqualified()).then_some(self.qualifier)
    }

    /// `None` for the unqualified `()`.
    pub(crate) fn qualifier_name(&self) -> Option<&'static str> {
        self.qualifier().map(|q| q.full())
    }

    pub(crate) fn is_unqualified(&self) -> bool {
        self.qualifier.id() == TypeId::of::<()>()
    }

    /// The type and, when there is one, the qualifier: every name the key prints.
    pub(crate) fn type_names(self) -> impl Iterator<Item = TypeName> {
        std::iter::once(self.ty).chain(self.qualifier())
    }

    pub(crate) fn name(&self, kind: BindingKind) -> KeyName {
        KeyName { key: *self, kind }
    }

    /// Built whole so `Display` can pad it as one string. `full` keeps every path.
    fn text(&self, full: bool) -> String {
        self.text_with(|name| if full { format!("{name:#}") } else { name.to_string() })
    }

    /// The key as a report writes it: each of its names in `full` with its full path, the others
    /// short, so `a::Config @ Replica` beside `b::Config`.
    pub(crate) fn text_in(&self, full: &HashSet<TypeName>) -> String {
        self.text_with(|name| name.written(full))
    }

    fn text_with(&self, name: impl Fn(TypeName) -> String) -> String {
        let mut text = role_spelling(&name(self.ty));
        if let Some(q) = self.qualifier() {
            text.push_str(" @ ");
            text.push_str(&name(q));
        }
        text
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
/// [`KeyName`], which carries the kind a bare `Key` does not. The alternate form, `{:#}`, writes
/// full type paths: `my_app::db::PgPool @ my_app::Replica`.
impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.text(f.alternate()))
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
/// Type names are shortened for display, and `{:#}` writes them with their full paths. The key
/// itself, with its `TypeId`s, is reachable through [`KeyName::key`], so a caller can compare it
/// with `Key::of::<T, Q>()`.
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

impl KeyName {
    /// The name as a report writes it, each type in `full` with its full path.
    pub(crate) fn text_in(&self, full: &HashSet<TypeName>) -> String {
        self.with_kind(self.key.text_in(full))
    }

    fn with_kind(&self, mut text: String) -> String {
        if self.kind == BindingKind::Collection {
            text.push_str(" (collection)");
        }
        text
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.with_kind(self.key.text(f.alternate())))
    }
}

impl fmt::Debug for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A role key as a user writes it: the key's type, `dyn ErasedGuard<Http>`, reads `AnyGuard<Http>`,
/// and `dyn ulo::transport::ErasedGuard<..>` reads `ulo::transport::AnyGuard<..>`. Text not opening
/// with one of the three erased traits is returned as given. Display only: roles themselves are
/// decided by `TypeId`.
pub(crate) fn role_spelling(text: &str) -> String {
    let Some(rest) = text.strip_prefix("dyn ") else { return text.to_owned() };
    let (path, tail) = rest.split_at(rest.find('<').unwrap_or(rest.len()));
    let (prefix, last) = match path.rfind("::") {
        Some(end) => path.split_at(end + 2),
        None => ("", path),
    };
    let alias = match last {
        "ErasedGuard" => "AnyGuard",
        "ErasedInterceptor" => "AnyInterceptor",
        "ErasedErrorHandler" => "AnyErrorHandler",
        _ => return text.to_owned(),
    };
    format!("{prefix}{alias}{tail}")
}
