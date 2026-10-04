//! The payload pump: actix's `!Send` request payload forwarded chunk by chunk through a bounded
//! channel from the worker-local task, and the response body driven the same way, whose failed
//! write is where a disconnect is observed.
