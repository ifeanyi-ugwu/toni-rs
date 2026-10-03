//! HTTP extractors (transports DESIGN §3.1): `Path<T>`, `Query<T>`, `Json<T>`, `Form<T>`,
//! `Bytes`, `BodyStream`, `Multipart`, `Header<H>`, `HeaderMap`, `LastEventId` and `HttpCx`, each a
//! `FromCall<Http>`; every container type is accepted beside them.
//!
//! `Json`, `Form`, `Bytes`, `BodyStream` and `Multipart` consume the body, so a handler declares at
//! most one of them, checked at compile time. A body over the route's limit fails with 413 before
//! deserialization. Every failure is an `ExtractError`: 400, 413, 415 or 422 by variant, field
//! violations in the problem document's `details`.

mod body;
mod head;
mod path;
mod query;

pub use body::{BodyStream, Form, Json, Multipart};
pub use head::{Header, LastEventId};
pub(crate) use path::check_path;
pub use path::{Path, PathCheck};
pub use query::Query;
