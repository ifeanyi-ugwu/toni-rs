// An error handler has one instance, shared by every execution, so a factory building one per
// execution cannot be declared under a slot holding an error handler.

struct Handler;

#[ulo::async_trait]
impl ulo::enhancer::ErrorHandler<ulo::http::HttpContext, ulo::http::HttpHandlerResult> for Handler {
    async fn handle_error(
        &self,
        _error: ulo::enhancer::ChainError<'_>,
        _ctx: &ulo::http::HttpContext,
    ) -> Option<ulo::http::HttpHandlerResult> {
        None
    }
}

ulo::key!(OnError: dyn ulo::enhancer::ErrorHandler<ulo::http::HttpContext, ulo::http::HttpHandlerResult>);

fn main() {
    let _ = ulo::provide!(OnError => async || Handler).per_execution();
}
