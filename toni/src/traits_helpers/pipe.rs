use crate::context::HandlerContext;
use crate::injector::Context;

/// A pipe transforms or validates the request before the handler runs.
// TODO: drop `= Context` default once the legacy `Context` is removed.
pub trait Pipe<C: ?Sized + HandlerContext = Context>: Send + Sync {
    fn process(&self, data: &mut C);
}
