//! The catch-all routes: one `rocket::route::Handler` per method, ranked so rocket's own routes
//! answer first, building the app's `Request` and buffering the body once the app reads it, and
//! forwarding a `Forwardable` miss with the request's original `Data`.

use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use rocket::data::ByteUnit;
use rocket::http::{Method, Status};
use rocket::route::{Handler, Outcome, Route};
use rocket::{Data, Request};
use tokio::sync::oneshot;
use ulo::BoxError;
use ulo_http::embed::Forwardable;
use ulo_http::{ConnInfo, HttpBody, Routing};

use crate::{Handle, convert, upgrade};

/// The rank of every catch-all route: above rocket's default ranks, which run from -12 to -1, so a
/// route of the host's own on a colliding path answers first.
const RANK: isize = 100;

/// Every method rocket routes.
const METHODS: [Method; 9] = [
    Method::Get,
    Method::Put,
    Method::Post,
    Method::Delete,
    Method::Options,
    Method::Head,
    Method::Trace,
    Method::Connect,
    Method::Patch,
];

/// The handler every catch-all route carries.
#[derive(Clone)]
pub struct RocketHandler {
    pub(crate) handle: Handle,
}

/// One catch-all route per method over `handle`, for `rocket.mount("/api", routes(&embedded))`.
/// Each is `/<path..>` at rank 100, so a route the host mounts itself on a colliding path answers
/// first, and under `Miss::Forward` a request the app misses goes on to the host's lower-ranked
/// routes.
pub fn routes(handle: &Handle) -> Vec<Route> {
    let handler = RocketHandler { handle: handle.clone() };
    METHODS.iter().map(|method| Route::ranked(RANK, *method, "/<path..>", handler.clone())).collect()
}

#[rocket::async_trait]
impl Handler for RocketHandler {
    async fn handle<'r>(&self, request: &'r Request<'_>, data: Data<'r>) -> Outcome<'r> {
        let Ok(head) = convert::head(request) else {
            return Outcome::Error(Status::BadRequest);
        };
        let (upgrade, handoff) = upgrade::pending(request);
        let conn = ConnInfo::new(http::Version::HTTP_11);
        let conn = match request.remote() {
            Some(peer) => conn.peer(peer),
            None => conn,
        };
        let (body, demand, fill) = BufferedBody::new();
        let app_req = ulo_http::Request { head, body: HttpBody::new(body), conn, upgrade };
        let answer = self.handle.service().respond(request, app_req);
        let (response, data) = answer_reading(answer, data, demand, fill, self.handle.body_limit()).await;
        if response.extensions().get::<Forwardable>().is_some()
            && let Some(data) = data
        {
            return Outcome::Forward((data, Status::NotFound));
        }
        if let Some(routing) = response.extensions().get::<Routing>().cloned() {
            request.local_cache(move || Some(routing));
        }
        Outcome::Success(convert::response(response, handoff, request.method() == Method::Head).await)
    }
}

/// Awaits the app's answer, reading `data` only once the app first polls its body. A request whose
/// body the app never touched keeps its `Data` unread, which a forwarded miss hands back to
/// rocket. Read, the body is buffered whole under `limit` plus one byte.
async fn answer_reading<'r, F>(
    answer: F,
    data: Data<'r>,
    demand: oneshot::Receiver<()>,
    fill: oneshot::Sender<Result<Buffered, String>>,
    limit: u64,
) -> (ulo_http::Response, Option<Data<'r>>)
where
    F: Future<Output = ulo_http::Response> + Send,
{
    let mut answer = pin!(answer);
    let mut demand = Some(demand);
    let mut source = Some((data, fill));
    let mut reading: Option<Pin<Box<dyn Future<Output = ()> + Send + 'r>>> = None;
    let response = poll_fn(|cx| {
        if let Poll::Ready(response) = answer.as_mut().poll(cx) {
            return Poll::Ready(response);
        }
        if let Some(asked) = demand.as_mut()
            && let Poll::Ready(asked) = Pin::new(asked).poll(cx)
        {
            demand = None;
            // `Err`: the app dropped the body without polling it, and nothing is read.
            if asked.is_ok()
                && let Some((data, fill)) = source.take()
            {
                reading = Some(Box::pin(read(data, fill, limit)));
            }
        }
        if let Some(read) = reading.as_mut()
            && read.as_mut().poll(cx).is_ready()
        {
            reading = None;
        }
        Poll::Pending
    })
    .await;
    (response, source.map(|(data, _)| data))
}

/// Reads the whole body, at most `limit + 1` bytes of it, into the app's body.
async fn read(data: Data<'_>, fill: oneshot::Sender<Result<Buffered, String>>, limit: u64) {
    let read = data.open(ByteUnit::from(limit.saturating_add(1))).into_bytes().await;
    let buffered = read
        .map(|capped| Buffered { cut: !capped.is_complete(), bytes: Bytes::from(capped.value) })
        .map_err(|error| error.to_string());
    let _ = fill.send(buffered);
}

/// What [`read`] buffered: the bytes, and whether the body went on past them.
struct Buffered {
    bytes: Bytes,
    cut: bool,
}

/// The app's body on rocket: it asks for the read on its first poll, then yields the buffered bytes
/// as one frame.
///
/// A body longer than the embedding's `body_limit` arrives as `body_limit + 1` bytes, which a route
/// on the default limit answers with the same 413 as on every host; on a route whose own limit is
/// higher, the frame after them is an error, so the app never takes the truncated body for the
/// whole one.
struct BufferedBody {
    state: State,
}

enum State {
    Waiting { demand: Option<oneshot::Sender<()>>, fill: oneshot::Receiver<Result<Buffered, String>> },
    Cut,
    Done,
}

impl BufferedBody {
    fn new() -> (Self, oneshot::Receiver<()>, oneshot::Sender<Result<Buffered, String>>) {
        let (demand, asked) = oneshot::channel();
        let (fill, filled) = oneshot::channel();
        (BufferedBody { state: State::Waiting { demand: Some(demand), fill: filled } }, asked, fill)
    }
}

impl http_body::Body for BufferedBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        match &mut this.state {
            State::Waiting { demand, fill } => {
                if let Some(demand) = demand.take() {
                    let _ = demand.send(());
                }
                let filled = match Pin::new(fill).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(filled) => filled,
                };
                match filled {
                    Ok(Ok(Buffered { bytes, cut })) => {
                        this.state = if cut { State::Cut } else { State::Done };
                        if bytes.is_empty() {
                            return Pin::new(this).poll_frame(cx);
                        }
                        Poll::Ready(Some(Ok(Frame::data(bytes))))
                    }
                    Ok(Err(error)) => {
                        this.state = State::Done;
                        Poll::Ready(Some(Err(BoxError::from(format!("reading the request body failed: {error}")))))
                    }
                    Err(_) => {
                        this.state = State::Done;
                        Poll::Ready(Some(Err(BoxError::from("the request body was read after the rocket handler returned"))))
                    }
                }
            }
            State::Cut => {
                this.state = State::Done;
                Poll::Ready(Some(Err(BoxError::from("the request body is longer than the rocket embedding buffers"))))
            }
            State::Done => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.state, State::Done)
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}
