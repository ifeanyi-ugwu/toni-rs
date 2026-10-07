//! `#[ulo_ws::gateway]` and `#[ulo_ws::message]`, with the parameter kinds and the reply forms.

use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize};
use ulo::{Dep, injectable, routes};
use ulo_ws::{Frame, Payload, Rooms, WsCx};

use crate::derives::Refusal;
use crate::enhancers::{Allow, Pass, Relay, Tag};

#[derive(Deserialize)]
pub struct Line {
    pub text: String,
}

#[derive(Serialize)]
pub struct Echoed {
    pub text: String,
}

#[injectable]
pub struct Chat;

#[routes]
#[ulo_ws::gateway(path = "/chat")]
#[guards(ws = Allow)]
#[interceptors(value = Pass)]
#[error_handlers(Relay)]
#[meta(Tag("chat"))]
impl Chat {
    #[ulo_ws::message("echo")]
    fn echo(&self, line: Payload<Line>) -> Echoed {
        Echoed { text: line.0.text }
    }

    #[ulo_ws::message("join")]
    async fn join(&self, cx: WsCx, rooms: Dep<Rooms>) -> Result<(), Refusal> {
        let _ = (cx, rooms);
        Ok(())
    }

    #[ulo_ws::message("count")]
    #[guards(with = || Allow)]
    fn count(&self, up_to: Payload<u32>) -> impl Stream<Item = Result<u32, Refusal>> {
        stream::iter((1..=up_to.0).map(Ok))
    }

    #[ulo_ws::message("raw")]
    fn raw(&self) -> Frame {
        Frame::text("raw")
    }

    #[ulo_ws::message("quiet")]
    fn quiet(&self) {}
}
