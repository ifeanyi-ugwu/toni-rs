//! `#[ulo_rpc::message]` and `#[ulo_rpc::event]`, with the parameter kinds and the reply forms.

use std::sync::Arc;

use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use ulo::{ExecutionRef, injectable, routes};
use ulo_rpc::{CallHeaders, Inbound, Payload};

use crate::derives::Refusal;
use crate::enhancers::{Allow, Pass, Relay, Tag};

#[derive(Deserialize, Serialize)]
pub struct Order {
    pub id: u64,
}

#[injectable]
pub struct Orders;

#[routes]
#[guards(rpc = Allow)]
#[interceptors(Pass)]
#[error_handlers(value = Relay)]
#[meta(Tag("orders"))]
impl Orders {
    #[ulo_rpc::message("orders.get")]
    fn get(&self, id: Payload<u64>) -> Order {
        Order { id: id.0 }
    }

    #[ulo_rpc::message("orders.find")]
    async fn find(&self, id: Payload<u64>, headers: CallHeaders, exec: ExecutionRef) -> Result<Order, Refusal> {
        let _ = (headers, exec);
        Ok(Order { id: id.0 })
    }

    #[ulo_rpc::message("orders.list")]
    #[guards(value = Allow)]
    async fn list(&self, n: Payload<u64>) -> impl Stream<Item = Result<Order, Refusal>> {
        stream::iter((1..=n.0).map(|id| Ok(Order { id })))
    }

    #[ulo_rpc::message("orders.total")]
    async fn total(&self, ids: Inbound<u64>) -> Result<u64, Refusal> {
        let mut ids = ids;
        let mut total = 0;
        while let Some(id) = ids.next().await {
            total += id.map_err(|_| Refusal)?;
        }
        Ok(total)
    }

    #[ulo_rpc::message("orders.double")]
    fn double(&self, ids: Inbound<u64>) -> impl Stream<Item = Result<u64, Refusal>> {
        ids.map(|id| id.map(|id| id * 2).map_err(|_| Refusal))
    }

    #[ulo_rpc::message("orders.held")]
    async fn held(self: Arc<Self>, exec: ExecutionRef) -> Result<(), Refusal> {
        exec.cancelled().await;
        Ok(())
    }

    #[ulo_rpc::event("orders.placed")]
    async fn placed(&self, order: Payload<Order>) -> Result<(), Refusal> {
        let _ = order.0.id;
        Ok(())
    }

    #[ulo_rpc::event("orders.seen")]
    fn seen(&self, _order: Payload<Order>) {}
}
