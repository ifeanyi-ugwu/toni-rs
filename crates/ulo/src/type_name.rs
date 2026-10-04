use std::any::{TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};

/// A type as an error report names it: `TypeName::of::<my_app::User>()` prints `User`, and with
/// `{:#}` prints `my_app::User`.
///
/// Two names compare on their `TypeId`s, so a type alias or a renamed import names the same type.
/// A report decides which names it writes with `{:#}` by running [`TypeName::colliding`] over
/// every name it holds: the wiring report over each [`Key`](crate::Key)'s type and qualifier and
/// the transports it names, the startup report over each failing transport and the names of each
/// [`PrepareError`](crate::PrepareError), and the shutdown report over its keys and transports.
/// A transport is named by its marker type, `TypeName::of::<Http>()`.
///
/// A report entry stores each name it prints as a `TypeName` or a [`Key`](crate::Key), the one a
/// [`KeyName`](crate::KeyName) carries, and renders it when the report is formatted. Only text that
/// never contains a type name is stored already rendered.
#[derive(Clone, Copy)]
pub struct TypeName {
    id: TypeId,
    name: &'static str,
}

impl TypeName {
    pub fn of<T: ?Sized + 'static>() -> TypeName {
        TypeName { id: TypeId::of::<T>(), name: type_name::<T>() }
    }

    /// The names among `names` whose short form another, different type among them shares,
    /// `a::Config` beside `b::Config`: the ones a report writes with `{:#}`. Run it over every
    /// name one report prints, so a name colliding with one in another entry prints in full too.
    pub fn colliding(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName> {
        let mut by_text: HashMap<String, Vec<TypeName>> = HashMap::new();
        for name in names {
            let alike = by_text.entry(name.to_string()).or_default();
            if !alike.contains(&name) {
                alike.push(name);
            }
        }
        by_text.into_values().filter(|alike| alike.len() > 1).flatten().collect()
    }

    /// A name rebuilt from an id and a `type_name` held as runtime values, as a module's identity
    /// holds its type.
    pub(crate) fn from_parts(id: TypeId, name: &'static str) -> TypeName {
        TypeName { id, name }
    }

    pub(crate) fn id(&self) -> TypeId {
        self.id
    }

    /// The full `type_name`, as `{:#}` writes it.
    pub(crate) fn full(&self) -> &'static str {
        self.name
    }

    /// `{:#}` when `full` holds this name, `{}` otherwise.
    pub(crate) fn written(&self, full: &HashSet<TypeName>) -> String {
        if full.contains(self) { self.name.to_owned() } else { short_type_name(self.name) }
    }
}

impl PartialEq for TypeName {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeName {}

impl Hash for TypeName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// `Arc<User>` for `alloc::sync::Arc<my_app::User>`: every path cut to its last segment, generic
/// arguments included. Two types whose last segments match print alike. The alternate form,
/// `{:#}`, writes the `type_name` whole.
impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() { f.pad(self.name) } else { f.pad(&short_type_name(self.name)) }
    }
}

impl fmt::Debug for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// `alloc::sync::Arc<my_app::db::PgPool>` reads `Arc<PgPool>`. [`TypeName`]'s `Display`, and
/// through it `Key`, `KeyName` and `ModuleName`; the reports that hold a bare `type_name` call it
/// directly.
///
/// Only the paths change: `dyn my_app::Repo + core::marker::Send` reads `dyn Repo + Send`, and the
/// punctuation of references, slices, tuples, function pointers and qualified paths is copied as
/// written.
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
