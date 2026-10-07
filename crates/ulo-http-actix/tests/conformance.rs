//! The embedding conformance suite against an actix-web host, as a scope at `PREFIX` and as the
//! default service. The host listens with `listen`, which serves HTTP/1.1 alone, so the HTTP/2
//! drain scenarios are declared not applicable here; `conformance_http2.rs` runs them against an
//! h2c host.

mod common;

ulo_http_conformance::http_conformance_suite!(common::ActixHost<false>; not_applicable {
    drain_http2: "the host listens with `listen`, which serves HTTP/1.1 alone; `conformance_http2` runs it",
    drain_goaway: "the host listens with `listen`, which serves HTTP/1.1 alone; `conformance_http2` runs it",
});
