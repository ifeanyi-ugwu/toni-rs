//! Capture the message of a panic raised inside an async block.
//!
//! Startup failures unwind rather than exiting, so a test can assert the whole
//! diagnostic in process instead of scraping a subprocess's output.

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};

/// Drives `f` to completion on a current-thread runtime and returns the message
/// of the panic it raised. The application is `!Send`, so the future runs on the
/// calling thread.
///
/// # Panics
///
/// Panics if `f` completes without panicking — the caller expected a failure.
pub fn panic_message<F, Fut>(f: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: Future,
{
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(f())
    }));

    match outcome {
        Ok(_) => panic!("expected a panic, but the call returned"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic payload was not a string".to_owned()),
    }
}
