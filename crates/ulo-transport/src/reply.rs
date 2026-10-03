//! A handler's answer, converted per transport (transports DESIGN §2.3).

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use ulo::{BoxError, Transport};

use crate::error::{Classify, ErrorKind};

/// A handler's answer on transport `T`: `()`, a value, a stream. A handler returning a `Result`
/// is answered through the reply probe, which sends the error side to the error handlers whatever
/// the return type is spelled as, an alias of `Result` included; `IntoReply` is the value side.
///
/// A transport implements it for its reply types: `Json<T>`, `Sse<S>`, a protobuf message, a
/// stream, and so on.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot answer a `{T}` call",
    note = "a transport implements `IntoReply` for the replies its wire can carry"
)]
pub trait IntoReply<T: Transport>: Send + 'static {
    fn into_reply(self, cx: &T::Cx) -> Result<T::Reply, IntoReplyError>;
}

/// The conversion's own failure: a serializer refusing a value, for example. Classified
/// `Internal`, with the cause kept as its source.
pub struct IntoReplyError {
    source: BoxError,
}

impl IntoReplyError {
    pub fn new(source: impl Into<BoxError>) -> Self {
        IntoReplyError { source: source.into() }
    }
}

impl fmt::Display for IntoReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the reply could not be encoded: {}", self.source)
    }
}

impl fmt::Debug for IntoReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntoReplyError").field("source", &self.source).finish()
    }
}

impl Error for IntoReplyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source as &(dyn Error + 'static))
    }
}

/// `Internal`, with the cause withheld from the caller.
impl Classify for IntoReplyError {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }

    fn public_message(&self) -> Cow<'_, str> {
        Cow::Borrowed("internal error")
    }
}
