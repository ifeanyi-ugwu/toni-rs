//! A gRPC handler reads like a handler on the other three transports.
//!
//! The service writes an inherent impl — `Payload<T>` in, its own reply and
//! its own error out — and `#[grpc_methods(Trait)]` writes the proto trait
//! impl around it: `Request` unwrapped, `Response` wrapped, the error mapped
//! to the code its `kind()` means and parked so the chain sees the type.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serial_test::serial;
use toni::context::{Extensions, GrpcContext, HandlerContext};
use toni::extractors::{Inbound, Payload};
use toni::toni_factory::ToniFactory;
use toni::{async_trait, injectable, module, ErrorKind, GrpcCode, GrpcStatus};
use toni_macros::{controller, grpc_methods, new, use_error_handlers, use_guards};

mod greeter_pb {
    tonic::include_proto!("toni_test.orders");
}

use greeter_pb::greeter_client::GreeterClient;
use greeter_pb::greeter_server::{Greeter, GreeterServer};

static SAW_CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct Seen(String);

#[derive(Debug)]
struct NoName;

impl std::fmt::Display for NoName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a greeting needs a name")
    }
}

impl std::error::Error for NoName {}

impl toni::Error for NoName {
    fn kind(&self) -> ErrorKind {
        ErrorKind::BadRequest
    }
}

/// Claims the domain type, which reaches the chain because the generated
/// method parks it on the execution.
#[injectable]
pub struct NoNameHandler {}

#[async_trait]
impl toni::traits_helpers::ErrorHandler<GrpcContext, GrpcStatus> for NoNameHandler {
    async fn handle_error(
        &self,
        error: toni::traits_helpers::ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcStatus> {
        error.downcast_ref::<NoName>()?;
        Some(GrpcStatus::new(
            GrpcCode::FailedPrecondition,
            "caught:no-name",
        ))
    }
}

/// Writes to the execution's bag, so a handler reading it back proves the bag
/// crossed from the guard rather than being made fresh per parameter.
#[injectable]
pub struct MarkGuard {}

#[async_trait]
impl toni::traits_helpers::Guard<GrpcContext> for MarkGuard {
    async fn can_activate(&self, ctx: &GrpcContext) -> bool {
        ctx.extensions().insert(Seen("from-guard".to_string()));
        true
    }
}

#[controller]
pub struct GreeterService {}

#[grpc_methods(greeter_pb::greeter_server::Greeter)]
#[use_error_handlers(NoNameHandler)]
impl GreeterService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// The whole handler: no `Request`, no `Response`, no `Status`.
    #[grpc_method]
    async fn greet(
        &self,
        Payload(req): Payload<greeter_pb::GreetRequest>,
        ctx: &GrpcContext,
    ) -> Result<greeter_pb::GreetReply, NoName> {
        if req.name.is_empty() {
            return Err(NoName);
        }
        Ok(greeter_pb::GreetReply {
            message: format!("{} on {}", req.name, ctx.method()),
        })
    }

    /// The execution rides the reply: a stream dropped with items still to
    /// come fires the token, which the work feeding it can select on.
    #[grpc_stream]
    async fn greet_forever(
        &self,
        Payload(_req): Payload<greeter_pb::GreetRequest>,
        ctx: &GrpcContext,
    ) -> Result<
        impl futures_util::Stream<Item = Result<greeter_pb::GreetReply, NoName>> + Send + 'static,
        NoName,
    > {
        let context = ctx.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for _ in 0..200 {
                let _ = tx.send(Ok(greeter_pb::GreetReply {
                    message: "tick".to_string(),
                }));
                tokio::select! {
                    _ = context.cancellation().cancelled() => {
                        SAW_CANCEL.store(true, Ordering::SeqCst);
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {}
                }
            }
        });
        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// The request written bare, which is how an RPC handler spells it, with
    /// the execution's bag beside it.
    #[grpc_method]
    #[use_guards(MarkGuard)]
    async fn greet_bare(
        &self,
        req: greeter_pb::GreetRequest,
        extensions: Extensions,
    ) -> Result<greeter_pb::GreetReply, NoName> {
        // The guard wrote this before the handler ran, so finding it here is
        // what says the bag is the execution's rather than one made per param.
        let seen: Option<Seen> = extensions.get();
        Ok(greeter_pb::GreetReply {
            message: format!("{}:{}", req.name, seen.map_or("none", |s| s.0.leak())),
        })
    }

    /// The escape hatch: the whole request, for what the shapes above do not
    /// cover — trailers, peer address, the metadata map as it arrived.
    #[grpc_method]
    async fn greet_raw(
        &self,
        request: tonic::Request<greeter_pb::GreetRequest>,
    ) -> Result<greeter_pb::GreetReply, NoName> {
        let peer = request.remote_addr().is_some();
        Ok(greeter_pb::GreetReply {
            message: format!("{}:{peer}", request.into_inner().name),
        })
    }

    /// The caller's stream arrives as `Inbound<T>`, which yields the message
    /// type rather than tonic's `Streaming`.
    #[grpc_method]
    async fn greet_all(
        &self,
        mut inbound: Inbound<greeter_pb::GreetRequest>,
    ) -> Result<greeter_pb::GreetReply, NoName> {
        use futures_util::StreamExt;

        let mut names = Vec::new();
        while let Some(item) = inbound.next().await {
            let req = item.map_err(|_| NoName)?;
            names.push(req.name);
        }
        if names.is_empty() {
            return Err(NoName);
        }
        Ok(greeter_pb::GreetReply {
            message: names.join(", "),
        })
    }

    /// Both directions at once: an inbound stream and a streaming reply.
    #[grpc_stream]
    async fn converse(
        &self,
        mut inbound: Inbound<greeter_pb::GreetRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<greeter_pb::GreetReply, NoName>> + Send + 'static,
        NoName,
    > {
        use futures_util::StreamExt;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(item) = inbound.next().await {
                let reply = item
                    .map(|req| greeter_pb::GreetReply {
                        message: format!("hi {}", req.name),
                    })
                    .map_err(|_| NoName);
                if tx.send(reply).is_err() {
                    return;
                }
            }
        });
        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// A streaming reply is a stream of the handler's own types. The macro
    /// declares the associated type tonic asks for and boxes this into it.
    #[grpc_stream]
    async fn greet_many(
        &self,
        Payload(req): Payload<greeter_pb::GreetRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<greeter_pb::GreetReply, NoName>> + Send + 'static,
        NoName,
    > {
        if req.name.is_empty() {
            return Err(NoName);
        }
        let name = req.name;
        Ok(futures_util::stream::iter((1..=2).map(move |i| {
            Ok(greeter_pb::GreetReply {
                message: format!("{name} {i}"),
            })
        })))
    }
}

#[module(controllers: [GreeterService], providers: [NoNameHandler, MarkGuard])]
impl GreeterModule {}

async fn boot() -> u16 {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::new().create_with(GreeterModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.unwrap()
}

async fn client(port: u16) -> GreeterClient<tonic::transport::Channel> {
    GreeterClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    )
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_answers_with_its_own_reply_type() {
    let mut client = client(boot().await).await;

    let reply = client
        .greet(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner();

    // The context param arrived too: the method path is what the caller dialled.
    assert_eq!(reply.message, "ada on toni_test.orders.Greeter/Greet");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_streaming_handler_answers_with_its_own_item_type() {
    use futures_util::StreamExt;

    let mut client = client(boot().await).await;

    let messages: Vec<String> = client
        .greet_many(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner()
        .map(|item| item.expect("each item arrives").message)
        .collect()
        .await;

    assert_eq!(messages, vec!["ada 1".to_string(), "ada 2".to_string()]);
}

/// A stream that fails before it opens is an ordinary handler error, so the
/// chain claims it the way it claims a unary one.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_streaming_handler_that_fails_to_open_reaches_the_chain() {
    let mut client = client(boot().await).await;

    let err = client
        .greet_many(greeter_pb::GreetRequest {
            name: String::new(),
        })
        .await
        .expect_err("an empty name fails the call");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "caught:no-name");
}

/// The generated associated type is the scoped one, so a reply the caller
/// abandons tells the work behind it — the guarantee ADR-0033 pins for a
/// hand-written trait impl, reached here without one.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_abandoned_stream_cancels_the_work_feeding_it() {
    use futures_util::StreamExt;

    SAW_CANCEL.store(false, Ordering::SeqCst);
    let mut client = client(boot().await).await;

    let mut stream = client
        .greet_forever(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner();

    stream.next().await.expect("one item").expect("an ok item");
    assert!(
        !SAW_CANCEL.load(Ordering::SeqCst),
        "a stream still being read must not read as abandoned"
    );

    drop(stream);

    let mut fired = false;
    for _ in 0..40 {
        if SAW_CANCEL.load(Ordering::SeqCst) {
            fired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        fired,
        "the work feeding the reply must learn the caller went"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_takes_its_request_bare_and_the_bag_beside_it() {
    let mut client = client(boot().await).await;

    let reply = client
        .greet_bare(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner();

    assert_eq!(reply.message, "ada:from-guard");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_can_take_the_request_whole() {
    let mut client = client(boot().await).await;

    let reply = client
        .greet_raw(greeter_pb::GreetRequest {
            name: "ada".to_string(),
        })
        .await
        .expect("the call succeeds")
        .into_inner();

    // The peer address is on the request and nowhere else, so a handler that
    // reads it has the wire shape rather than a copy of the message.
    assert_eq!(reply.message, "ada:true");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_reads_the_caller_s_stream() {
    let mut client = client(boot().await).await;

    let names = futures_util::stream::iter(["ada", "grace", "edsger"].map(|name| {
        greeter_pb::GreetRequest {
            name: name.to_string(),
        }
    }));

    let reply = client
        .greet_all(names)
        .await
        .expect("the call succeeds")
        .into_inner();

    assert_eq!(reply.message, "ada, grace, edsger");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_answers_each_message_as_it_arrives() {
    use futures_util::StreamExt;

    let mut client = client(boot().await).await;

    let names = futures_util::stream::iter(["ada", "grace"].map(|name| greeter_pb::GreetRequest {
        name: name.to_string(),
    }));

    let messages: Vec<String> = client
        .converse(names)
        .await
        .expect("the call succeeds")
        .into_inner()
        .map(|item| item.expect("each item arrives").message)
        .collect()
        .await;

    assert_eq!(messages, vec!["hi ada".to_string(), "hi grace".to_string()]);
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_error_reaches_the_chain_with_its_type() {
    let mut client = client(boot().await).await;

    let err = client
        .greet(greeter_pb::GreetRequest {
            name: String::new(),
        })
        .await
        .expect_err("an empty name fails the call");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "caught:no-name");
}
