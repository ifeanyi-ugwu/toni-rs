//! # toni-actix
//!
//! Actix-web adapter for the Toni framework.
//!
//! This crate provides an implementation of Toni's `HttpAdapter` trait for the Actix-web framework,
//! allowing you to use Actix-web as the HTTP server for your Toni applications.
//!
//! ## Usage
//!
//! ```ignore
//! use toni_actix::ActixAdapter;
//!
//! #[actix_web::main]
//! async fn main() {
//!     let mut app = ToniFactory::new()
//!         .create_with(AppModule)
//!         .await
//!         .unwrap();
//!     app.use_http_adapter(ActixAdapter::new(), ("127.0.0.1", 3000)).unwrap();
//!     app.start().await.unwrap();
//! }
//! ```

mod actix_adapter;

pub use actix_adapter::ActixAdapter;

pub use toni::HttpAdapter;
