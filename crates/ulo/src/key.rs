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

    /// A key rebuilt from ids held as runtime values, as a module's identity holds its type and
    /// a keyed module its qualifier.
    pub(crate) fn from_parts(
        ty: TypeId,
        ty_name: &'static str,
        qualifier: TypeId,
        q_name: &'static str,
    ) -> Key {
        Key { ty, qualifier, ty_name, q_name }
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

    /// Built whole so `Display` can pad it as one string. `full` keeps every path, for a report
    /// where two keys would otherwise print alike.
    fn text(&self, full: bool) -> String {
        let name = |n: &'static str| if full { n.to_owned() } else { short_type_name(n) };
        let mut text = role_spelling(&name(self.ty_name));
        if let Some(q) = self.qualifier_name() {
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

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = self.key.text(f.alternate());
        if self.kind == BindingKind::Collection {
            text.push_str(" (collection)");
        }
        f.pad(&text)
    }
}

impl fmt::Debug for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The diagnostic spelling of a `type_name`: module paths stripped from every segment, so
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

/// `alloc::sync::Arc<my_app::db::PgPool>` reads `Arc<PgPool>`. Shared by `Key`, `KeyName`,
/// `ModuleName` and every wiring report.
///
/// Only the paths change: `dyn my_app::Repo + core::marker::Send` reads `dyn Repo + Send`, and the
/// punctuation of references, slices, tuples, function pointers and qualified paths is copied as
/// written. Two types whose last segments match print alike.
pub(crate) fn short_type_name(full: &'static str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut rest = full;
    while let Some(c) = rest.chars().next() {
        if is_path_char(c) {
            let end = rest.find(|c: char| !is_path_char(c)).unwrap_or(rest.len());
            push_last_segment(&mut out, &rest[..end]);
            rest = &rest[end..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The characters of one `a::b::{{closure}}` run. `type_name` writes `:` only inside `::`.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '{' | '}')
}

/// Writes the last segment of `path`. A closure segment such as `{{closure}}` keeps the item
/// that encloses it, which is the part that names it. A leading `::`, as after the `>` of
/// `<A as Tr>::Out`, is kept.
fn push_last_segment(out: &mut String, path: &str) {
    let path = match path.strip_prefix("::") {
        Some(tail) => {
            out.push_str("::");
            tail
        }
        None => path,
    };
    let mut segments = path.rsplit("::");
    let last = segments.next().unwrap_or("");
    if last.starts_with('{') {
        if let Some(enclosing) = segments.next() {
            out.push_str(enclosing);
            out.push_str("::");
        }
    }
    out.push_str(last);
}
