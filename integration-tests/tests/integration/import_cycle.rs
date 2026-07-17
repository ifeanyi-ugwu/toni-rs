use toni::*;

// Two modules that import each other. An import edge is a visibility relationship, not a
// construction dependency, so a mutual import cycle must boot and both modules' providers
// must resolve — not be silently dropped from instantiation.

#[injectable]
pub struct ServiceA {
    pub label: String,
}

impl ServiceA {
    #[new]
    pub fn new() -> Self {
        Self { label: "a".into() }
    }
}

#[injectable]
pub struct ServiceB {
    pub label: String,
}

impl ServiceB {
    #[new]
    pub fn new() -> Self {
        Self { label: "b".into() }
    }
}

#[module(providers: [ServiceA], exports: [ServiceA], imports: [ModuleB])]
impl ModuleA {}

#[module(providers: [ServiceB], exports: [ServiceB], imports: [ModuleA])]
impl ModuleB {}

#[module(imports: [ModuleA, ModuleB])]
impl AppModule {}

#[tokio::test]
async fn mutual_import_cycle_still_instantiates_both_modules() {
    let app = ToniFactory::create(AppModule).await;

    let a = app
        .get::<ServiceA>()
        .await
        .expect("ServiceA must resolve despite the import cycle");
    let b = app
        .get::<ServiceB>()
        .await
        .expect("ServiceB must resolve despite the import cycle");

    assert_eq!(a.label, "a");
    assert_eq!(b.label, "b");
}

// A one-way provider dependency that crosses the import cycle: ServiceC depends on
// ServiceD, which ModuleD exports. The deferred-retry loop must build ServiceD first,
// then ServiceC — the export has to resolve even though the modules import each other.

#[injectable]
pub struct ServiceD {
    pub label: String,
}

impl ServiceD {
    #[new]
    pub fn new() -> Self {
        Self { label: "d".into() }
    }
}

#[injectable]
pub struct ServiceC {
    pub d_label: String,
}

impl ServiceC {
    #[new]
    pub fn new(d: ServiceD) -> Self {
        Self { d_label: d.label }
    }
}

#[module(providers: [ServiceC], exports: [ServiceC], imports: [ModuleD])]
impl ModuleC {}

#[module(providers: [ServiceD], exports: [ServiceD], imports: [ModuleC])]
impl ModuleD {}

#[module(imports: [ModuleC, ModuleD])]
impl AppModuleCd {}

#[tokio::test]
async fn cross_module_dependency_through_import_cycle_resolves() {
    let app = ToniFactory::create(AppModuleCd).await;

    let c = app
        .get::<ServiceC>()
        .await
        .expect("ServiceC must resolve its cross-module dependency through the import cycle");

    assert_eq!(c.d_label, "d");
}
