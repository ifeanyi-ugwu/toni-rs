//! `build.rs` code generation for `ulo-grpc` (transports DESIGN §6.1): tonic-prost-build over a
//! vendored `protoc`, writing the messages (prost), a client per service (tonic), and, through a
//! `ServiceGenerator` wrapping tonic's, one marker type per RPC method under the service's module,
//! `pub struct GetUser; impl ulo_grpc::Method for GetUser { .. }`, where
//! `ulo_grpc::include_proto!` finds them. The file descriptor set for reflection is written when
//! asked for.
//!
//! ```ignore
//! // build.rs
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     ulo_build::configure().compile(&["proto/users.proto"], &["proto"])?;
//!     Ok(())
//! }
//! ```
//!
//! The vendored `protoc` is the default-on `vendored-protoc` feature, and a `PROTOC` environment
//! variable wins over it.

mod markers;

use std::io;
use std::path::{Path, PathBuf};

/// The build step's configuration, from [`configure`].
#[derive(Clone, Debug)]
pub struct Builder {
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) file_descriptor_set_path: Option<PathBuf>,
    pub(crate) build_client: bool,
}

/// The build step with every setting at its default: clients generated, written to `OUT_DIR`, no
/// descriptor set.
pub fn configure() -> Builder {
    Builder { out_dir: None, file_descriptor_set_path: None, build_client: true }
}

impl Builder {
    /// Where the generated files go: `OUT_DIR` unset.
    pub fn out_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.out_dir = Some(dir.as_ref().to_owned());
        self
    }

    /// Writes the encoded file descriptor set here, which `include_proto!` exposes as
    /// `FILE_DESCRIPTOR_SET` for reflection.
    pub fn file_descriptor_set_path(mut self, path: impl AsRef<Path>) -> Self {
        self.file_descriptor_set_path = Some(path.as_ref().to_owned());
        self
    }

    /// Whether a client is generated per service: `true` unset.
    pub fn build_client(mut self, enabled: bool) -> Self {
        self.build_client = enabled;
        self
    }

    /// Compiles `protos`, each found under one of `includes`, writing the messages, the clients and
    /// the method markers.
    pub fn compile(self, protos: &[impl AsRef<Path>], includes: &[impl AsRef<Path>]) -> io::Result<()> {
        let _ = (protos.len(), includes.len());
        todo!()
    }
}
