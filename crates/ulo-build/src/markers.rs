//! The `ServiceGenerator` wrapping tonic's: after tonic writes a service's client, it appends one
//! `ulo_grpc::Method` marker per method under the service's module, and the `ulo_grpc::GrpcClient`
//! impl for the client.

use prost_build::{Service, ServiceGenerator};

/// Tonic's generator with the markers appended.
pub(crate) struct Markers {
    pub(crate) tonic: Box<dyn ServiceGenerator>,
}

impl ServiceGenerator for Markers {
    fn generate(&mut self, service: Service, buf: &mut String) {
        let _ = (service, buf, &self.tonic);
        todo!()
    }

    fn finalize(&mut self, buf: &mut String) {
        self.tonic.finalize(buf);
    }
}
