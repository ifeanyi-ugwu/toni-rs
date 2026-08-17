use crate::context::HandlerContext;

/// A pipe transforms or validates the request before the handler runs.
///
/// `None` continues to the next pipe and then the handler. `Some` answers the
/// request there and then: the remaining pipes and the handler are skipped, and
/// the returned value is what goes out. Rejecting invalid input is the usual
/// reason to answer from a pipe.
///
/// `R` is what the transport answers with, matching
/// [`Interceptor`](super::Interceptor).
pub trait Pipe<C: ?Sized + HandlerContext, R>: Send + Sync {
    fn process(&self, data: &C) -> Option<R>;
}
