//! The embedding conformance suite against the hyper backend on smol, in both modes: the server on
//! `ulo-listen-smol`'s listener, the app on `ulo_smol::Smol`, the client over `async-net`. No tokio
//! runtime runs; every scenario the tokio reference passes, this one passes.

ulo_http_conformance::http_conformance_suite!(ulo_http_conformance::HyperHost<ulo_http_conformance::OnSmol>; not_applicable {
    routing_extension: "the reference has no host around the app to read `Routing`",
});
