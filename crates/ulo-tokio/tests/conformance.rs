//! The runtime conformance suite over `ulo_tokio::Tokio`, each scenario on a multi-thread runtime
//! of its own.

use std::future::Future;

use ulo_runtime_conformance::{Harness, runtime_suite};
use ulo_tokio::Tokio;

struct OnTokio;

impl Harness for OnTokio {
    type Runtime = Tokio;

    fn runtime() -> Tokio {
        Tokio
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a tokio runtime for the scenario")
            .block_on(fut)
    }
}

runtime_suite!(OnTokio);
