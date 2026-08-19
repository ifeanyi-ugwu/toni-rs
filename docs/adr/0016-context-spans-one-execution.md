# 0016 — A context spans one execution and is a shared handle

Status: proposed

## Context

A handler context is named for what it is: the execution context — the object that exists for the
duration of one execution. One HTTP request. One RPC call. One WebSocket message. Every question
below is the same question, which is where the code and that definition disagree.

### The context does not last as long as the execution

An HTTP request is not finished when the handler returns. It is finished when the last byte reaches
the client. A handler returning `BodyInner::Streaming` has produced no bytes yet; an SSE endpoint may
stream for hours.

`execute_controller_logic` ends with `context.into_response()`, which consumes the context and hands
the adapter a body. The context is dropped at handler return and the adapter drains the body
afterwards. Everything the execution knows about itself — the extension bag, the cancellation token,
the route metadata — stops being reachable at the point where a streaming response has done none of
its work.

This is the root of the belief that `'static` streams outlive their scope. They do not outlive the
execution; they *are* the tail of it. They outlive an object that ends early.

### Contexts cannot be shared, for a reason confined to two fields

`&T` is `Send` only where `T` is `Sync`. `HttpContext` is `Send` and not `Sync`, and the whole of that
comes from two fields:

```rust
pub type BoxBody        = UnsyncBoxBody<Bytes, Box<dyn Error + Send + Sync>>;
pub type RequestBoxBody = UnsyncBoxBody<Bytes, Box<dyn Error + Send + Sync>>;
// http_body_util:
pub struct UnsyncBoxBody<D, E> { inner: Pin<Box<dyn Body<Data = D, Error = E> + Send + 'static>> }
```

Send, not Sync. Every other field `HttpContext` holds is Sync already.

`Mutex<T>` is `Sync` when `T` is `Send`. `UnsyncBoxBody` is `Send`. A body behind a `Mutex` is
therefore `Sync` with the aliases unchanged and with no bound added to user streams.

Before the Sync-bound realignment the response alias was `BoxBody<..>` — `Send + Sync` — and the
context's slots were Mutexed. Streams paid a Sync bound and the slots paid a lock. Both were removed
together, though only the first was forced: the lock never needed a Sync body, only a Send one. The
remaining combination — Mutexed slots over Send-only bodies — makes a context `Sync` while keeping
everything the realignment bought for streams.

### One execution has phases, and exclusive access is not the same as a phase

Access to an execution changes over its life:

- **Reading** — the request body is available and single-use.
- **Deciding** — enhancers and the handler run; an answer can still be chosen.
- **Emitting** — the answer is committed and the body is streaming; choosing an answer is meaningless.

`&mut C` encodes exactly one of these boundaries, at handler return, and encodes it by destroying the
context rather than by naming the phase. That is simultaneously too strict — it denies a streaming
body access to the bag, which is legal and correct — and too loose, since nothing distinguishes
setting a response during Deciding from setting one during Emitting.

### Three participants answer by mutation

Five kinds of code can answer a request. Two of them return a value: middleware returns
`MiddlewareResult` and an error handler returns `Option<R>`.

The other three reach for the slot. `Interceptor::intercept` and `Pipe::process` both return `()`, so
neither can answer except through `context.set_response(..)` — a pipe pairs it with `abort()` to stop
the chain. A guard returns `bool`, and the dispatcher additionally reads any response the guard left
behind and prefers it over the canonical rejection envelope.

Those three uses are not equally grounded. A pipe rejecting invalid input is a real, exercised
capability. The guard path is reachable but unused: no guard in the repository sets a response, and
`#[catch(GuardRejection)]` covers the same ground at strictly higher precedence, since chain handlers
are consulted before the guard's response is.

### What follows from the definition on each transport

An RPC controller is resolved once at startup and held as `Arc<dyn RpcControllerTrait>`, so nothing on
that transport is built per execution. `ProviderContext::WebSocket` and `ProviderContext::Rpc` are unit
variants that no code constructs. On WebSocket, `WsContext::new` repoints its client clone at a fresh
bag, and `begin_connect` stores the client it was passed rather than the one the context holds — three
bags across one connect, none of which reaches the next message.

## Decision

### The execution is the unit, and it ends when the answer ends

An execution begins when a request, call or message arrives and ends when its answer is complete —
including the drain of a streaming body. The context lives exactly that long.

The response body carries the context to make this mechanical:

```rust
struct ScopedBody { inner: BoxBody, _ctx: HttpContext }
```

The dispatcher wraps rather than consumes. The adapter drains the body; the last frame drops the
handle; the execution ends there. WebSocket does the same for a streaming answer, since a stream that
has emitted nothing is in the same position as a body that has written nothing.

Making the context a handle already covers half of this on its own: a stream that *captures* one keeps
the execution alive through its own `Arc`, with no help from the dispatcher. What the wrap adds is the
other half — the execution's state survives the drain whether or not the answer happens to hold a
reference to it. That is the difference between a property of the framework and a property of how a
particular handler was written, and it is what makes request-scoped state safe to drop at the end of
an execution rather than at the end of a handler.

### A context is a cheap-clone handle

```rust
#[derive(Clone)]                                   // Send + Sync + 'static
pub struct HttpContext { inner: Arc<HttpInner> }

struct HttpInner {
    route:        Option<Arc<RouteMetadata>>,
    extensions:   Extensions,
    cache:        ExecutionCache,
    cancellation: CancellationToken,
    parts:        RequestPart,
    body:         Mutex<Option<RequestBody>>,
}
```

Locks are per slot, not one around the whole inner, so awaiting the body does not hold anything else.
`Extensions` is already this shape and is the existing evidence that it works: a handle whose `Arc`
keeps the state alive across detachment, correct when a stream captures it.

Enhancers take `&C`. A handler that wants to keep the context clones it.

Four handles, not one type. An extractor declares its valid transports through its `FromContext<C>`
impls (ADR-0015), which requires the contexts to be distinct types. Uniformity of shape, not
collapse into one noun.

### Phases need no enforcement, because the answer never lives here

The plan was a `committed` flag and a runtime warning, on the reasoning that a handle cannot express
"the answer is committed" as a bound. Moving the answer off the context removed the state that flag
would have policed: there is nothing on a context to answer with, at any phase, so there is no
precedence and no window to warn about.

What survives of the phase distinction is the request body, and it was already loud — a second read
fails by name rather than handing back an empty one.

So the compile-time check `&mut` provided is not traded for a runtime one. It is dropped, because what
it enforced — exclusivity — was never the property worth having.

### The answer is returned, never written

`Interceptor` and `Pipe` gain the result type middleware and error handlers already have:

```rust
pub trait Interceptor<C: ?Sized + HandlerContext, R>: Send + Sync {
    async fn intercept(&self, ctx: &C, next: Box<dyn InterceptorNext<C, R>>) -> R;
}

pub trait InterceptorNext<C: ?Sized + HandlerContext, R>: Send {
    async fn run(self: Box<Self>, ctx: &C) -> R;
}

pub trait Pipe<C: ?Sized + HandlerContext, R>: Send + Sync {
    fn process(&self, data: &mut C) -> Option<R>;
}
```

For an interceptor, short-circuiting is returning without calling `next` — which is what skipping the
handler already means. For a pipe, `Some` answers and skips the remaining pipes and the handler, which
is what `set_response` plus `abort()` meant when written together.

`abort()` does not survive that. Once every enhancer answers by returning, a flag saying "stop, with
nothing to send" is a third spelling of what `bool` and `Some(R)` say more precisely — and it was
honoured on RPC, WebSocket and gRPC while HTTP's guard loop never read it, so the same guard rejected
on three transports and was ignored on the fourth. It also took its name from the concept
`CancellationToken` implements, which is a different thing entirely: stop because the caller went away,
not stop because this stage decided to. Nest has no `abort` on `ExecutionContext` either. It is removed
along with `should_abort` and the seven checks that read it.

A guard therefore rejects by returning `false`, and a pipe rejects by returning `Some`. A pipe's answer
and a handler's error still leave by different doors, and the difference is observable: an answer a
pipe returns is the reply, while a handler's `Err` renders through the error chain into the
success-frame-carrying-an-error-envelope. A rejected request is not a user error and is not framed as
one; the parallel is guard rejection.

Guards keep returning `bool`, matching Nest, and lose the read-back of a response they may have left
on the context. `#[catch(GuardRejection)]` is the replacement, and already outranked that path.

`R` is concrete per transport — `HttpResponse`, `RpcHandlerResult`, `WsHandlerResult`,
`GrpcHandlerResult` — as `ErrorHandler<C, R>` already is. An associated type on `HandlerContext` would
serve the same purpose and is unavailable: that trait is used as `dyn HandlerContext` by
`ErrorObserver` and the observer fan-out.

WebSocket answers with `WsHandlerResult` rather than the `Result<Option<WsMessage>, WsError>` the
response slot held, because that slot could not hold a stream. A gateway handler returning one reached
the dispatcher through a side channel — an `Arc<Mutex<Option<BoxStream>>>` threaded beside the context
— for no reason other than the slot's shape. Returning the type that can express the whole answer
removes the channel.

`set_response` and the response slot are then removed from all four contexts. The dispatchers stop
carrying a value from handler to exit through a slot and return it instead. Of the two Mutexes a
shared context would need, this leaves one, and the one it leaves is the one whose misuse is already
loud: a second body read fails by name.

Two error branches disappear with the slot rather than being ported. "Request aborted by interceptor
without response" on RPC, and its WebSocket twin, described an interceptor that stopped the chain with
nothing to send. An interceptor that must return `R` cannot reach that state, so the type removes the
condition instead of the code that checked for it.

### The per-execution instance cache belongs to the execution

`RequestCache` is a `TypeId`-keyed store of instances built for one execution, so that a guard factory
and the controller resolving the same request-scoped type build it once. Nothing in it is HTTP. Its
`install(&mut parts)` / `adopt(parts)` carrier rides the request parts only because there was no
execution object to hold it.

It becomes a field on every context and is renamed `ExecutionCache`. The carrier methods are removed.
Callers driving the container outside a request — tests, CLI entry points, background jobs — construct
a context instead of installing a cache on synthetic parts.

Moving it forces the enhancer factory SPI to move with it, and that is the larger half of the change.
A factory received `Option<&RequestPart>`, which is only enough to find a cache that rides on parts;
it now receives the execution itself:

```rust
fn create<'a>(&'a self, ctx: &'a C) -> Pin<Box<dyn Future<Output = Arc<dyn Guard<C>>> + Send + 'a>>;
```

Which in turn forces the dispatcher's order: the context is built before enhancers resolve, rather
than assembled from parts afterwards.

And that removes a rule rather than relocating it. `requires_http_parts()` and the startup refusals on
RPC, WebSocket and gRPC — *"this transport has no HTTP request context"* — were true only while the
scope handle was HTTP-shaped. Every transport carries an execution, so a request-scoped provider is a
usable enhancer dependency on all four. This is the HTTP privilege dissolving, and it is the point of
the ADR rather than a side effect of it.

### Provider contexts stay concrete; universal state goes through the trait

```rust
pub enum ProviderContext {          // no lifetime; Clone, not Copy
    Http(HttpContext),
    Ws(WsContext),
    Rpc(RpcContext),
    Grpc(GrpcContext),
    None,
}
```

The hollow variants are filled rather than deleted, and they carry the handle itself rather than a
selection of fields.

`Option<&dyn HandlerContext>` would replace the enum outright and is rejected. A request-scoped HTTP
provider reads headers, which means an accessor returning `&RequestPart` on `HandlerContext`, which
puts an `http::request::Parts` on the WebSocket and RPC contexts. That is the leak the enum was
introduced to prevent, relocated from one variant onto every implementor.

The line: state every execution has reaches through `HandlerContext` — extensions, cancellation, route
metadata, and now `cache()`. State one transport has reaches through the variant.

The enum stops being `Copy`, which is worth stating because it is load-bearing. Holding a handle rather
than two borrowed references means a construction site that passed the context on by value now has to
clone it, and the compiler names each one. Those sites were correct only because the old enum was
trivially copyable.

### Each transport gets one execution context per execution

**RPC.** A call is an execution, so a controller with request-scoped dependencies is built per call,
gated by the same elevation check controllers already use. A controller rebuilt per execution cannot
be held as a dependency, so `#[rpc_controller]` becomes a dispatch target and stops being registered
as an injectable provider. This is a divergence from the current registration, where every provider
reaches the provider store regardless of role.

The store is not only what dependency resolution reads — the startup and shutdown hook loops read it
too. So a dispatch target is moved to a second collection rather than left out of both: what it loses
is resolvability, not its lifecycle.

**WebSocket.** One context per message, and one per connect, rather than three across a connect. A
connect guard's writes reach the connect hook because there is one execution and one bag, not because
anything is copied between them.

**gRPC.** In. Nothing about tonic obstructs the context model. A call is an execution, so a service
with request-scoped dependencies is built per call and `#[grpc_service]` becomes a dispatch target on
the same terms as `#[rpc_controller]` — the argument is about what a holder could know, and holds
wherever the scope follows the dependencies. The service is asked for inside the wrapper's delegate,
which is where tonic's generated impl hands control back.

gRPC remains outside the extractor model of ADR-0015, because `#[grpc_methods]` re-emits a signature
the tonic trait dictates; that exclusion is about handler signatures and does not extend to the
context those handlers run under.

### A connection is a session, not an execution

A WebSocket connection is a long-lived thing during which many executions happen. State spanning it is
session state and is out of scope here. A session scope — store, lifetime, teardown, API — is its own
decision, to be recorded when something needs one.

The distinction is load-bearing for this ADR rather than adjacent to it: it is why WebSocket gets an
execution context per message instead of per connection. Keying a request scope on the client object
produces a session scope wearing an execution scope's machinery, which is what the per-message
boundary here avoids.

## Consequences

- Breaking for every `Guard`, `Interceptor`, `Pipe` and `ErrorHandler` implementor: `&mut C` becomes
  `&C`. The third change to these signatures, and the first driven by what a context *is* rather than
  by what a bound forces.
- Breaking for `Interceptor` implementors twice over: the signature returns `R`, and short-circuiting
  moves from `set_response` to `return`. An interceptor that only observes drops a semicolon; one that
  works after the call binds the answer and returns it.
- Breaking for `Pipe` implementors: `process` returns `Option<R>`, so every pipe that transforms and
  falls through ends in `None`, and one that rejects returns `Some` instead of writing and aborting.
- `abort()` and `should_abort()` are removed from `HandlerContext`. A guard rejects by returning
  `false`; a pipe rejects by returning `Some`. Code calling either has no replacement, and no silent
  behaviour change: on HTTP a guard's `abort()` was already ignored.
- Breaking for guards only where one set a custom rejection response. Reshape with
  `#[catch(GuardRejection)]`, which already took precedence over that response.
- `set_response`, `response()`, `response_mut()`, `take_response()` and `into_response()` are removed
  from all four contexts. The internal call sites are spread across `instance_wrapper`,
  `rpc_controller_wrapper`, `gateway_wrapper` and `grpc_runtime`, and include the error-handler and
  panic-recovery paths. If this is too large to land with the rest, the response slot stays behind a
  `Mutex` and a superseding ADR records the return form when it arrives.
- A handler can no longer set a response at all, so the precedence rule between a set response and a
  returned one is gone, and with it the warning that announced which had won.
- `RequestCache::install` and `RequestCache::adopt` are removed along with the `RequestCache` name.
  Callers resolving providers outside a request build a context.
- `#[rpc_controller]` types stop being injectable. Any code holding one as a dependency breaks, with
  no replacement — a dispatch target is not a dependency.
- The window in which an execution's state is reachable grows from handler return to body drain. A
  bug that leaked a context previously ended with the handler and now ends with the stream.
- No phase check is added. Answering off-phase stopped being representable when the answer left the
  context, so the warning this ADR planned for is unnecessary rather than deferred.
- Extension-bag reads from inside an SSE stream, a WebSocket stream handler, or any gRPC streaming mode
  become possible. They return the execution's bag rather than nothing.

## Roads not taken

**Require Sync streams.** Reverts the Sync-bound realignment and reimposes `Stream + Send + Sync` on
every user stream. `axum::body::Body` is itself an `UnsyncBoxBody`, so the bound would cost adapter
interoperability to buy a property a `Mutex` provides for free.

**A borrowed `Context<'_>`.** Sync via the same Mutexes, but a `&'a Context` cannot be held by a
`'static` stream. The shape would be available for the sequential part of an execution and absent for
its tail, which is the failure this ADR exists to remove.

**A `Send + Sync` view type beside the concrete context.** Splits on whether a field is Sync, which is
a property of the body's boxing rather than of the execution. The split that carries meaning is by
phase, and the phase split is what the returned answer and the take-once body already express.

**Ambient scoping (task-local, CLS).** Previously rejected on the grounds that an ambient scope wraps a
future and dies when that future resolves, leaving `'static` streams reading nothing. That reasoning
described the early-drop defect rather than a property of ambient: a scope wrapping the response body's
poll would cover the whole execution. Ambient remains unadopted for the reason that survives — it is
the one form with nothing for the compiler to check, where a captured handle is checked and correct.
