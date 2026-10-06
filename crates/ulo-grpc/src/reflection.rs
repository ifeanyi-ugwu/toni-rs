//! `grpc.reflection.v1` and `v1alpha` from the generated descriptor sets, through
//! `tonic_reflection::server::Builder` and its `build_v1` and `build_v1alpha` (transports DESIGN
//! §6.2). Added to the path table in debug builds and behind `Server::reflection(true)` in release
//! builds.

use tonic_reflection::server::Builder;

use crate::dispatch::{Builtin, builtin};

/// Both reflection services over `sets`, each an encoded `FileDescriptorSet` as `ulo-build` writes
/// it; `Err` with the reason when a set does not decode, for the server's `prepare` failure.
pub(crate) fn services(sets: &[&'static [u8]]) -> Result<Vec<(&'static str, Builtin)>, String> {
    let mut v1 = Builder::configure();
    let mut v1alpha = Builder::configure();
    for &set in sets {
        v1 = v1.register_encoded_file_descriptor_set(set);
        v1alpha = v1alpha.register_encoded_file_descriptor_set(set);
    }
    let v1 = v1.build_v1().map_err(|error| format!("reflection: a file descriptor set does not decode: {error}"))?;
    let v1alpha = v1alpha.build_v1alpha().map_err(|error| format!("reflection: a file descriptor set does not decode: {error}"))?;
    Ok(vec![builtin(v1), builtin(v1alpha)])
}
