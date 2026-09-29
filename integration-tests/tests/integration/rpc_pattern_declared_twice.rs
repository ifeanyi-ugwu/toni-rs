//! Two controllers declaring one RPC pattern fail `create`, naming the pattern and both
//! controllers, in one module or across two, and with either attribute.

use ulo::dispatch::Items;
use ulo::rpc::{RpcData, RpcHandlerResult};
use ulo::{UloFactory, module};
use ulo_macros::{controller, event_pattern, message_pattern, patterns};

macro_rules! answering {
    ($name:ident, $pattern:literal) => {
        #[controller]
        pub struct $name {}

        #[patterns]
        impl $name {
            #[message_pattern($pattern)]
            async fn handle(&self) -> RpcHandlerResult {
                Ok(Items::One(RpcData::text(stringify!($name))))
            }
        }
    };
}

answering!(FirstDupe, "dupe.pattern");
answering!(SecondDupe, "dupe.pattern");
answering!(Elsewhere, "other.pattern");

#[controller]
pub struct EventDupe {}

#[patterns]
impl EventDupe {
    #[event_pattern("dupe.pattern")]
    async fn handle(&self) -> Result<(), ulo::rpc::RpcError> {
        Ok(())
    }
}

async fn refusal(module: impl ulo::di::ModuleMetadata + 'static) -> String {
    match UloFactory::create(module).await {
        Ok(_) => panic!("the module must be refused"),
        Err(err) => err.to_string(),
    }
}

#[module(controllers: [FirstDupe, SecondDupe])]
struct OneModule;

#[module(controllers: [FirstDupe])]
struct FirstModule;

#[module(controllers: [SecondDupe])]
struct SecondModule;

#[module(imports: [FirstModule, SecondModule])]
struct TwoModules;

#[module(controllers: [FirstDupe, Elsewhere])]
struct DistinctPatterns;

#[module(controllers: [FirstDupe, EventDupe])]
struct MessageAndEvent;

#[tokio::test]
async fn two_controllers_declaring_one_pattern_are_refused() {
    let err = refusal(OneModule).await;
    assert!(err.contains("`dupe.pattern`"), "{err}");
    assert!(
        err.contains("FirstDupe") && err.contains("SecondDupe"),
        "{err}"
    );

    let err = refusal(TwoModules).await;
    assert!(err.contains("`dupe.pattern`"), "{err}");
    assert!(
        err.contains("FirstDupe") && err.contains("SecondDupe"),
        "{err}"
    );

    let err = refusal(MessageAndEvent).await;
    assert!(err.contains("`dupe.pattern`"), "{err}");
    assert!(
        err.contains("FirstDupe") && err.contains("EventDupe"),
        "{err}"
    );

    UloFactory::create(DistinctPatterns)
        .await
        .expect("controllers declaring distinct patterns start");
}
