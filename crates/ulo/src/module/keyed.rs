use std::marker::PhantomData;

use crate::binding::Qualifier;
use crate::module::def::ModuleDef;
use crate::module::{Module, ModuleIdentity};

/// `M` as the keyed instance `Q`: itself a module, built by [`Module::keyed`].
///
/// Inside `M`, sites stay unqualified (`Dep<PgPool>`). At the export boundary, unqualified
/// exports are requalified as `T @ Q`, re-exports included. Contributions are not exports and
/// keep their key. An execution input declared inside is a wiring error. The identity includes
/// `Q`, so `DbModule::for_root(url)` beside `DbModule::for_root(url).keyed::<Primary>()` is two
/// modules; one pool under two keys is an alias, not a second import.
pub struct Keyed<Q, M> {
    inner: M,
    _q: PhantomData<fn() -> Q>,
}

impl<Q: 'static, M: Module> Keyed<Q, M> {
    pub(crate) fn new(inner: M) -> Self {
        Keyed { inner, _q: PhantomData }
    }
}

impl<Q: 'static, M: Module> Module for Keyed<Q, M> {
    fn identity(&self) -> ModuleIdentity {
        self.inner.identity().keyed_by(Qualifier::of::<Q>())
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.keyed_by(Qualifier::of::<Q>());
        self.inner.register(m);
    }
}
