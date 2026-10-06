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
//!
//! The generated code names `tonic` (with its `channel` and `codegen` features), `tonic-prost`
//! and `prost`, and `prost-types` where a proto uses a well-known type other than `Empty`, so the
//! crate running the build step depends on those beside `ulo-grpc`. No server trait is generated:
//! handlers bind to the markers, and the dispatcher calls them by path.

mod markers;

use std::io;
use std::path::{Path, PathBuf};

use crate::markers::Markers;

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
    /// `FILE_DESCRIPTOR_SET` for reflection. A relative path is taken from the directory the build
    /// script runs in, the package's root.
    pub fn file_descriptor_set_path(mut self, path: impl AsRef<Path>) -> Self {
        self.file_descriptor_set_path = Some(path.as_ref().to_owned());
        self
    }

    /// Whether a client is generated per service: `true` unset. A client gets the
    /// `ulo_grpc::GrpcClient` impl `GrpcClientModule` builds it through.
    pub fn build_client(mut self, enabled: bool) -> Self {
        self.build_client = enabled;
        self
    }

    /// Compiles `protos`, each found under one of `includes`, writing the messages, the clients and
    /// the method markers.
    pub fn compile(self, protos: &[impl AsRef<Path>], includes: &[impl AsRef<Path>]) -> io::Result<()> {
        let out_dir = match self.out_dir {
            Some(dir) => dir,
            None => std::env::var_os("OUT_DIR").map(PathBuf::from).ok_or_else(|| {
                io::Error::other("`ulo_build::configure().compile(..)` writes to `OUT_DIR`, which cargo sets only for a build script; set `out_dir` elsewhere")
            })?,
        };
        #[cfg_attr(not(feature = "vendored-protoc"), allow(unused_mut))]
        let mut includes: Vec<PathBuf> = includes.iter().map(|include| include.as_ref().to_owned()).collect();
        let mut config = prost_build::Config::new();
        config.out_dir(&out_dir);

        #[cfg(feature = "vendored-protoc")]
        if std::env::var_os("PROTOC").is_none() {
            let protoc = protoc_bin_vendored::protoc_bin_path().map_err(|error| io::Error::other(format!("vendored protoc: {error}")))?;
            config.protoc_executable(protoc);
            let well_known = protoc_bin_vendored::include_path().map_err(|error| io::Error::other(format!("vendored protoc: {error}")))?;
            includes.push(well_known);
        }

        // `include_bytes!` resolves a relative path against the generated file, in `OUT_DIR`, not
        // against the package, so the path the markers write is made absolute first.
        let descriptor = match self.file_descriptor_set_path {
            Some(path) if path.is_relative() => Some(std::env::current_dir()?.join(path)),
            other => other,
        };
        if let Some(path) = &descriptor {
            config.file_descriptor_set_path(path);
        }

        let tonic = tonic_prost_build::configure().build_client(self.build_client).build_server(false).service_generator();
        config.service_generator(Box::new(Markers { tonic, build_client: self.build_client, descriptor }));
        config.compile_protos(protos, &includes)
    }
}
