//! The embedding conformance suite against an actix-web host serving HTTP/2 without TLS beside
//! HTTP/1.1 (`listen_auto_h2c`), built with actix-web's `http2` feature through this crate's
//! `conformance-http2`. Any crate in an application can turn that feature on, Cargo unifying
//! features, so the HTTP/2 drain the adapter declares (`DrainHttp2::Reset`) is asserted here.
//!
//! ```text
//! cargo test -p ulo-http-actix --features conformance-http2 --test conformance_http2
//! ```

mod common;

ulo_http_conformance::http_conformance_suite!(common::ActixHost<true>);
