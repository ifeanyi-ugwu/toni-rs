//! Modules: Rust types with an identity that declare imports, bindings, controllers and exports
//! (§3.6, §8).

pub(crate) mod def;
pub(crate) mod dynamic;
pub(crate) mod handle;
pub(crate) mod keyed;
pub(crate) mod meta;

use std::any::{Any, TypeId, type_name};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::binding::Qualifier;
use crate::key::short_type_name;
use crate::module::def::ModuleDef;
use crate::module::keyed::Keyed;

/// A module: an identity and a synchronous, I/O-free registration.
///
/// `register` returns `()` and never returns early. A value that may fail to build goes through
/// [`ModuleDef::try_value`], which records the `Err` for `wire()` to report beside every other
/// wiring error.
pub trait Module: Send + Sync + 'static {
    /// Identity for deduplication and diagnostics.
    fn identity(&self) -> ModuleIdentity;

    /// Declare everything. Synchronous and free of I/O.
    fn register(&self, m: &mut ModuleDef<'_>);

    /// This module as a keyed instance: inside it injection points stay unqualified, and at its
    /// export boundary every unqualified export, re-exports included, is requalified as `T @ Q`.
    /// `DbModule::for_root(url).keyed::<Primary>()`.
    fn keyed<Q: 'static>(self) -> Keyed<Q, Self>
    where
        Self: Sized,
    {
        Keyed::new(self)
    }
}

/// Who a module is: its type, plus its configuration value if it has one, plus a qualifier if
/// it is keyed. Two configurations of one type are two modules; the same configuration imported
/// twice is one.
///
/// Equality and hashing ignore the label. The configuration's `Debug` output is never printed,
/// because it may hold credentials.
#[derive(Clone)]
pub struct ModuleIdentity {
    ty: TypeId,
    ty_name: &'static str,
    config: Option<ConfigKey>,
    qualifier: Option<Qualifier>,
    label: Option<&'static str>,
}

impl ModuleIdentity {
    /// A unit module: its type alone.
    pub fn of_type<M: 'static>() -> Self {
        ModuleIdentity { ty: TypeId::of::<M>(), ty_name: type_name::<M>(), config: None, qualifier: None, label: None }
    }

    /// A configured module: its type and its value.
    pub fn of_value<M: Eq + Hash + Clone + Send + Sync + 'static>(m: &M) -> Self {
        ModuleIdentity::of_owner::<M, M>(m)
    }

    /// The name diagnostics print in place of the type name.
    pub fn label(self, name: &'static str) -> Self {
        ModuleIdentity { label: Some(name), ..self }
    }

    /// The owner type `O` with configuration `C`, which is how a `DynamicModule` is identified.
    pub(crate) fn of_owner<O: 'static, C: Eq + Hash + Clone + Send + Sync + 'static>(config: &C) -> Self {
        // `DefaultHasher::new` is unkeyed: equal configurations hash alike in every map holding
        // the identity. The configuration's type is hashed too, which keeps two configurations
        // of different types apart before the value comparison runs.
        let mut hasher = DefaultHasher::new();
        TypeId::of::<C>().hash(&mut hasher);
        config.hash(&mut hasher);
        ModuleIdentity {
            ty: TypeId::of::<O>(),
            ty_name: type_name::<O>(),
            config: Some(ConfigKey { hash: hasher.finish(), value: Arc::new(config.clone()) }),
            qualifier: None,
            label: None,
        }
    }

    pub(crate) fn keyed_by(self, qualifier: Qualifier) -> Self {
        ModuleIdentity { qualifier: Some(qualifier), ..self }
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.ty
    }

    pub(crate) fn type_name(&self) -> &'static str {
        self.ty_name
    }

    pub(crate) fn qualifier(&self) -> Option<Qualifier> {
        self.qualifier
    }

    pub(crate) fn label_text(&self) -> Option<&'static str> {
        self.label
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.config.is_some()
    }
}

impl PartialEq for ModuleIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty && self.qualifier == other.qualifier && self.config == other.config
    }
}

impl Eq for ModuleIdentity {}

impl Hash for ModuleIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ty.hash(state);
        self.qualifier.hash(state);
        self.config.as_ref().map(|c| c.hash).hash(state);
    }
}

/// A configuration value erased for identity: its precomputed hash and a comparison by value.
#[derive(Clone)]
pub(crate) struct ConfigKey {
    pub(crate) hash: u64,
    pub(crate) value: Arc<dyn DynKey>,
}

impl PartialEq for ConfigKey {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.value.eq_dyn(&*other.value)
    }
}

pub(crate) trait DynKey: Any + Send + Sync {
    fn eq_dyn(&self, other: &dyn DynKey) -> bool;
    fn as_any(&self) -> &dyn Any;
}

impl<C: Eq + Send + Sync + 'static> DynKey for C {
    fn eq_dyn(&self, other: &dyn DynKey) -> bool {
        other.as_any().downcast_ref::<C>() == Some(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A module as the errors name it: the type name or its label, the qualifier, and a position for
/// a second configuration of one type, as in `DbModule @ Replica` or `DbModule #2`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ModuleName {
    text: Arc<str>,
}

impl ModuleName {
    /// Names `identity`. `instance` is its zero-based position, in collection order, among the
    /// registered modules of its type and qualifier; from the second on it is printed as `#n`,
    /// labelled or not, since several configurations commonly share one label.
    pub(crate) fn of(identity: &ModuleIdentity, instance: usize) -> Self {
        let mut text = match identity.label {
            Some(label) => label.to_owned(),
            None => short_type_name(identity.ty_name),
        };
        if let Some(qualifier) = identity.qualifier {
            text.push_str(" @ ");
            text.push_str(&short_type_name(qualifier.name));
        }
        if instance > 0 {
            text.push_str(&format!(" #{}", instance + 1));
        }
        ModuleName { text: Arc::from(text) }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
