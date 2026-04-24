#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "http")]
pub use self::http::HttpHealthIndicator;

#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "memory")]
pub use self::memory::MemoryHealthIndicator;

#[cfg(feature = "disk")]
pub mod disk;
#[cfg(feature = "disk")]
pub use self::disk::DiskHealthIndicator;
