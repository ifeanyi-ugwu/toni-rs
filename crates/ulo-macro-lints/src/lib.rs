//! Every attribute and derive macro the framework exports, expanded as a user's crate expands
//! them, so CI's `cargo clippy -p ulo-macro-lints --all-targets --no-deps -- -D warnings` fails on
//! the first lint generated code trips. Nothing here runs: each item exists to be expanded, and
//! the hand-written bodies are as small as the macros allow, so a warning names generated code.
//!
//! Each transport's handlers cover the forms its attribute expands differently: sync and `async`,
//! a plain value, a `Result` and a stream, the body-consuming parameters, `self: Arc<Self>`, the
//! three enhancer spellings at both tiers, `#[meta]`, and a generic controller.

pub mod derives;
pub mod di;
pub mod enhancers;
pub mod grpc;
pub mod http;
pub mod rpc;
pub mod ws;

/// The package `lints.v1`: one method per call shape. The module is named as the service, so the
/// marker module `ulo-build` writes is `lints::lints`.
pub mod lints {
    ulo_grpc::include_proto!("lints.v1");
}

/// The root module, `#[module]` over every controller here.
#[ulo::module(
    imports = [ulo_ws::WsModule::for_root()],
    controllers = [
        http::Pages,
        http::Generic<u8>,
        ws::Chat,
        rpc::Orders,
        grpc::LintsService,
    ],
    providers = [di::Store, di::Clock, di::Named, di::Defaulted],
    exports = [di::Store],
)]
pub struct AppModule;
