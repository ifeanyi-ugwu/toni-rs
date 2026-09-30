//! What an injection site reads follows its type, not its spelling: an alias of `Arc<T>`, `Arc`
//! imported under another name, and aliases of `Arc<dyn Trait>` and of a collection each read what
//! the spelled-out form reads, in a field, a `#[new]` parameter and under a marker.

use std::sync::Arc;
use std::sync::Arc as Handle;

use ulo::di::Execution;
use ulo::{UloFactory, injectable, key, module, new, provide};

#[injectable]
pub struct Store {}

pub trait Logger: Send + Sync {
    fn name(&self) -> &'static str;
}

pub struct Console;

impl Logger for Console {
    fn name(&self) -> &'static str {
        "console"
    }
}

pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}

pub struct Alpha;

impl Plugin for Alpha {
    fn name(&self) -> &'static str {
        "alpha"
    }
}

type SharedStore = Arc<Store>;
type Log = Arc<dyn Logger>;
type Plugins = Vec<Arc<dyn Plugin>>;
type SharedLimit = Arc<u32>;

key!(pub Limit: u32);

#[injectable]
pub struct Spelled {
    #[inject]
    store: Arc<Store>,
}

#[injectable]
pub struct Aliased {
    #[inject]
    store: SharedStore,
    #[inject]
    renamed: Handle<Store>,
    #[inject]
    log: Log,
    #[inject]
    plugins: Plugins,
    #[inject(Limit)]
    limit: SharedLimit,
}

#[injectable]
pub struct Built {
    store: SharedStore,
}

impl Built {
    #[new]
    fn new(store: SharedStore) -> Self {
        Self { store }
    }
}

/// Built per execution, so nothing reads its field at startup but the loader's check, which must
/// not take an aliased `Arc` over a `provide!` value for a plain read.
#[injectable(scope = "execution")]
pub struct PerCall {
    #[inject(Limit)]
    limit: SharedLimit,
}

#[module(providers: [
    Store,
    Spelled,
    Aliased,
    Built,
    PerCall,
    provide!(dyn Logger => value Console),
    provide!(into dyn Plugin => value Alpha),
    provide!(Limit => 7u32),
])]
struct Sites;

#[tokio::test]
async fn an_alias_or_a_renamed_arc_reads_what_the_spelled_out_arc_reads() {
    let ctx = UloFactory::create_application_context(Sites)
        .await
        .expect("every site reads its binding");

    let spelled = ctx.get::<Spelled>().await.unwrap();
    let aliased = ctx.get::<Aliased>().await.unwrap();
    let built = ctx.get::<Built>().await.unwrap();
    assert!(Arc::ptr_eq(&aliased.store, &spelled.store));
    assert!(Arc::ptr_eq(&aliased.renamed, &spelled.store));
    assert!(Arc::ptr_eq(&built.store, &spelled.store));
    assert_eq!(aliased.log.name(), "console");
    let plugins: Vec<_> = aliased.plugins.iter().map(|plugin| plugin.name()).collect();
    assert_eq!(plugins, ["alpha"]);
    assert_eq!(*aliased.limit, 7);

    let per_call = ctx
        .resolve::<PerCall>(&Execution::standalone())
        .await
        .unwrap();
    assert_eq!(*per_call.limit, 7);
}
