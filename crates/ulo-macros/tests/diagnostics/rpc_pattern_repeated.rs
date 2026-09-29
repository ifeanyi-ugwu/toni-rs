// Two methods of one `#[patterns]` impl declaring one pattern: the later one would never answer, and
// the adapter would be handed the pattern twice.

#[ulo::controller]
pub struct Orders {}

#[ulo::patterns]
impl Orders {
    #[ulo::message_pattern("orders.get")]
    async fn first(&self) -> ulo::rpc::RpcHandlerResult {
        Ok(ulo::dispatch::Items::Empty)
    }

    #[ulo::message_pattern("orders.get")]
    async fn second(&self) -> ulo::rpc::RpcHandlerResult {
        Ok(ulo::dispatch::Items::Empty)
    }
}

fn main() {}
