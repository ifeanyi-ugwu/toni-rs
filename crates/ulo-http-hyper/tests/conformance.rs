//! The embedding conformance suite against its reference host, the hyper backend on tokio, in both
//! modes.

ulo_http_conformance::http_conformance_suite!(ulo_http_conformance::HyperHost<ulo_http_conformance::OnTokio>; not_applicable {
    routing_extension: "the reference has no host around the app to read `Routing`",
});
