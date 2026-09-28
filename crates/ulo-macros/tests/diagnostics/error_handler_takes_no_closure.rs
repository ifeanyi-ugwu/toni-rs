// An error handler is built once and shared by every execution, so the closure
// spelling — which builds per execution — has nothing to land on. It is refused
// at the argument, naming the spellings that are accepted.
//
// Paths are written in full and the handler type is well-typed, so the
// recorded output is this diagnostic and nothing else.

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

#[ulo::controller("/p")]
pub struct P {}

#[ulo::routes]
#[ulo::use_error_handlers(|_ctx| Handler)]
impl P {
    #[ulo::get("/")]
    fn get(&self) -> ulo::http::Body {
        ulo::http::Body::text("ok".to_string())
    }
}

fn main() {}
