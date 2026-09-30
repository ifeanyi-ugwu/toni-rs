//! A field or parameter written as a plain type over a binding that hands out one shared instance
//! is refused: at `create` for a declaration the loader can read, and at resolution for a provider
//! written by hand that answers an `Arc` without saying so.
//!
//! Each consumer here is built per execution or per call, so nothing resolves it at startup and
//! only the loader's check refuses it at `create`.

use std::any::Any;
use std::sync::Arc;

use ulo::di::{DeclaresProvider, DynamicModule, Execution, ModuleMetadata, ResolutionError};
use ulo::spi::{BuildResult, Provider, ProviderFactory, Registration};
use ulo::{
    FxHashMap, StartupError, UloFactory, async_trait, controller, injectable, key, module, new,
    provide,
};

key!(pub Limit: u32);

/// The consumer `create` refused and the refusal's key and written type.
async fn refusal(module: impl ModuleMetadata + 'static) -> (String, String, String) {
    match UloFactory::create_application_context(module).await {
        Err(StartupError::BuildFailed { token, source, .. }) => {
            match source.downcast_ref::<ResolutionError>() {
                Some(ResolutionError::SharedByValue { token: key, wrote }) => {
                    (token, key.clone(), wrote.clone())
                }
                _ => panic!("unexpected build failure: {source}"),
            }
        }
        Err(other) => panic!("unexpected startup error: {other}"),
        Ok(_) => panic!("the module must be refused"),
    }
}

#[injectable(scope = "execution")]
pub struct ReadsAField {
    #[inject(Limit)]
    limit: u32,
}

#[module(providers: [provide!(Limit => 10u32), ReadsAField])]
struct PlainField;

#[tokio::test]
async fn a_plain_field_over_a_provided_value_fails_create() {
    let (consumer, key, wrote) = refusal(PlainField).await;
    assert!(consumer.ends_with("::ReadsAField"), "{consumer}");
    assert!(key.ends_with("::Limit"), "{key}");
    assert_eq!(wrote, "u32");
}

#[injectable(scope = "execution")]
pub struct ReadsAParameter {
    limit: u32,
}

impl ReadsAParameter {
    #[new]
    fn new(#[inject(Limit)] limit: u32) -> Self {
        Self { limit }
    }
}

#[module(providers: [provide!(Limit => 10u32), ReadsAParameter])]
struct PlainParameter;

#[tokio::test]
async fn a_plain_parameter_over_a_provided_value_fails_create() {
    let (consumer, key, wrote) = refusal(PlainParameter).await;
    assert!(consumer.ends_with("::ReadsAParameter"), "{consumer}");
    assert!(key.ends_with("::Limit"), "{key}");
    assert_eq!(wrote, "u32");
}

#[controller("/limits", scope = "execution")]
pub struct LimitsController {
    #[inject(Limit)]
    limit: u32,
}

#[module(providers: [provide!(Limit => 10u32)], controllers: [LimitsController])]
struct PlainControllerField;

#[tokio::test]
async fn a_plain_controller_field_over_a_provided_value_fails_create() {
    let (consumer, key, wrote) = refusal(PlainControllerField).await;
    assert_eq!(consumer, "LimitsController");
    assert!(key.ends_with("::Limit"), "{key}");
    assert_eq!(wrote, "u32");
}

#[derive(Debug)]
pub struct Gauge(u32);

/// Answers `Arc<Gauge>` and leaves `shape` at its default, a value.
struct SharedGaugeFactory;

#[async_trait]
impl ProviderFactory for SharedGaugeFactory {
    fn token(&self) -> String {
        ulo::di::token_of::<Gauge>()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        Ok(Registration::new(Arc::new(SharedGaugeProvider), vec![]))
    }
}

struct SharedGaugeProvider;

#[async_trait]
impl Provider for SharedGaugeProvider {
    fn token(&self) -> String {
        ulo::di::token_of::<Gauge>()
    }

    async fn resolve(&self, _ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        Ok(Box::new(Arc::new(Gauge(1))))
    }
}

#[injectable(scope = "execution")]
pub struct ReadsAGauge {
    #[inject]
    gauge: Gauge,
}

#[tokio::test]
async fn a_plain_field_over_an_unannounced_shared_answer_is_refused_where_it_resolves() {
    let module = DynamicModule::builder("Gauges")
        .provider(SharedGaugeFactory)
        .provider(ReadsAGauge::provide())
        .build();
    let ctx = UloFactory::create_application_context(module)
        .await
        .expect("the provider says it hands out a value, so the loader passes it");

    match ctx.resolve::<ReadsAGauge>(&Execution::standalone()).await {
        Err(ResolutionError::SharedByValue { token, wrote }) => {
            assert!(token.ends_with("::Gauge"), "{token}");
            assert!(wrote.ends_with("::Gauge"), "{wrote}");
        }
        Err(other) => panic!("unexpected refusal: {other}"),
        Ok(reads) => panic!("a plain field took a shared answer: {:?}", reads.gauge),
    }
}
