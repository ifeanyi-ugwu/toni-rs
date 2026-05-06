use crate::context::HandlerContext;

/// A pipe transforms or validates the request before the handler runs.
pub trait Pipe<C: ?Sized + HandlerContext>: Send + Sync {
    fn process(&self, data: &mut C);
}
