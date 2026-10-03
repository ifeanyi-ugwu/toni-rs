# Responses on the transport design

The design author's answers on the transport layer, in order, each with the user's sign-off when given.

---

## Naming exchange during the review

Received 2026-10-03, while the review was being written. The user asked about GraphQL's positioning
and about trait names; the author answered on naming. Not yet signed off. The GraphQL question has no
answer in this exchange.

dont wrry, code excution is enabled, check.........now, two thigns: graphql, the way it is positioned, does i acknlwoedge tha fact that grphawl can be paired with anyother thign? because as far a si know graphql does not necessarily need http, just that over http has astandardard,  thats why i am asking for this and the param trait, shont it be named extract? ...MiddlewareNext why not it be named next still i knwo itneceptor has next we can add or epxor alias for them if the user dont want to do module naming speicifally you get the point?

On the nested shape: yes, that was its purpose. It avoided repeating param in every variant. Both shapes are defensible. The flat one matches LookupError, which repeats key in every variant for the same reason: you can match ExtractError::Missing { param } in one pattern. I'd keep the flat shape for that consistency, and the param() accessor covers code that only wants the name.

Your naming rule
You're right on both counts, and the rule is better grounded than the symmetry argument I used for FromCall.

A trait is named after its predominant method. That's the dominant pattern in std: Clone::clone, Hash::hash, Drop::drop, Default::default, From::from, FromStr::from_str, IntoIterator::into_iter, Deref::deref, AsRef::as_ref, ToString::to_string. The early Rust naming conventions (RFC 344) point the same way: traits are preferably verbs, and a trait with one primary method shares its name.

When no single method fits, name the concept. That's Iterator (its method is next), Future (poll), Error, Read, and Write. The core's role traits (Guard, Interceptor, ErrorHandler) belong here: they name the role a type plays, not one method.

There's a matching rule for errors, also from std: an error that one operation produces is named after that operation. String::from_utf8 fails with FromUtf8Error, try_from with TryFromIntError, CString::into_string with IntoStringError, and parse with ParseIntError. An error shared by a whole family of operations is named after the domain: io::Error, and the core's LookupError.

Applying it
The inbound trait should be Extract, as you first suggested. Under this rule, FromCall::extract is wrong: the trait and its method disagree. There are two correct options:

FromCall::from_call, with an error named... FromCallError, which is awkward.
Extract::extract, with ExtractError, which aligns completely, like Clone::clone.
The second is clearly right. I chose FromCall for symmetry with FromContainer, but symmetry was a criterion I invented. Yours comes from std.

The outbound error should be IntoReplyError, not ReplyError. By the error rule, a failure of into_reply is IntoReplyError, as into_string fails with IntoStringError.

The core has one violation. FromContainer's method is read. It should be from_container, the way FromStr's is from_str. Its error stays LookupError, because that's a family error: dep, get, by_key and every resolver read produce it too, the way io::Error covers every I/O method.

The rest of what's built passes:

Trait	Method(s)	Verdict
Construct	construct → ConstructError	✓ fully aligned
Module	register, identity	✓ concept; no single method
Guard, Interceptor, ErrorHandler, Middleware	role methods	✓ roles
Timer, Server, Transport, Engine, Link	several methods	✓ concepts
Validate	validate	✓
Meta, Factory, Scope, Role, AllowedIn, HookCapable	markers or concepts	✓
FromContainer	read	✗ should be from_container
Classified	kind, public_message, details	✗ (see below)
Classified needs your decision. It's an adjective, and its predominant method is kind. The rule points to Classify::classify(&self) -> ErrorKind. But that collides with the free function fw_transport::classify(&BoxError) -> CallError, which does something different: it turns any error into a CallError. If the trait takes the name, the free function should move to CallError::from_error(&BoxError). That's a conversion constructor, which is where Rust usually puts such functions. Two things with the same name meaning different things would be worse than either name alone.

One more stutter, found while auditing: fw_http::HttpBackend becomes fw_http::Backend, by the rule you approved.

So the changes are:

Was	Now
FromCall::extract	Extract::extract → ExtractError (flat)
ReplyError	IntoReplyError
FromContainer::read (core, built)	FromContainer::from_container
Classified::kind + free classify(&BoxError)	Classify::classify + CallError::from_error(&BoxError), if you approve
HttpBackend	fw_http::Backend
if you think the Extract shoudl be a from or an inot because it conversta value or rather pull value from feel free to suggest ( i dunno the rule behaind from and into etc and try from and try into, but i guess the try varaints return and error and the others dont, but that makes sense only if the fallibel and non flaible variants exist toghether), otherwise, ..now on the lcassified, the name seems wwird by the way, what is your ecoendtion for bth its name nd the free funcion

From, Into, and the Try variants
You guessed the Try rule correctly. Try only appears where an infallible sibling exists: TryFrom exists because From does. A fallible conversion with no infallible sibling just returns a Result under the plain name. FromStr::from_str returns Result and isn't called TryFromStr. File::open isn't try_open.

The From / Into distinction is about direction:

From* means Self is built from a source: From<T>::from(t), FromStr::from_str(s), FromIterator::from_iter(it). The source is the input.
Into* means Self turns itself into a target, consuming itself: Into<T>::into(self), IntoIterator::into_iter(self).
Applied here:

The return value is Into. The handler's value is consumed and turned into the transport's reply, so IntoReply::into_reply(self) is right.
The parameter is From. The parameter is built from the call, the way a number is built from a string. The call isn't consumed, and only part of it is read. Into would point the wrong way. TryFrom would be wrong too, because std's TryFrom has a fixed, synchronous, by-value signature, and this has no infallible sibling.
That puts me in an awkward spot: it argues for FromCall::from_call, which I talked you out of last time. My objection then was that its error would have to be called FromCallError. Std shows that objection was wrong. FromStr::from_str fails with ParseIntError and ParseFloatError, named after the operation users think of ("parsing"), not after the trait. The core already does the same thing: FromContainer fails with LookupError. So the std-grounded and fully parallel design is:

Trait	Method	Error
FromContainer (core)	from_container	LookupError
FromCall<T>	from_call	ExtractError
IntoReply<T>	into_reply	IntoReplyError
Extract::extract also follows your method rule, so it isn't wrong. But FromCall says where the value comes from, and that matters here because a handler parameter can come from either source, the container or the call. The pair FromContainer / FromCall makes that visible in every diagnostic. I recommend FromCall.

Classified: the trait and the free function
It reads oddly because it's a past-participle adjective. Std mostly uses verbs or nouns for traits, and your method rule points straight at the fix.

The trait: Classify, with the method classify:

rust
pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;
    fn public_message(&self) -> Cow<'_, str> { self.to_string().into() }
    fn details(&self) -> Details { Details::default() }
}

#[derive(Debug, thiserror::Error, Classify)]
pub enum UserError {
    #[error("user not found")] #[classify(not_found)] NotFound,
}
impl Classify for UserError { fn classify(&self) -> ErrorKind { .. } } reads as a sentence, and the derive's helper attribute takes the same word.

The free function becomes conversions on CallError, split by what you're converting from:

From a typed domain error: impl<E: Classify> From<E> for CallError. Then ? works directly: inside a function returning Result<_, CallError>, a UserError converts itself. It's the same pattern anyhow uses. It compiles because CallError itself never implements Classify. If it did, the impl would collide with std's impl<T> From<T> for T. So CallError keeps an inherent kind() instead. That costs nothing, and the rule should be written in the docs.
From an arbitrary BoxError: CallError::from_boxed(err: BoxError) -> CallError. This is the recogniser: it walks the error and maps GuardRejected, PanicRecovered, ExtractError, LookupError::Construct and so on. It's a named constructor, like std's from_utf8 or from_raw, rather than a From impl, for two reasons. First, a From<BoxError> impl next to the blanket impl above would rely on the compiler proving that Box<dyn Error> can never implement Classify, and I wouldn't bet on coherence accepting that. Second, recognising types isn't a plain conversion, and a name that says so is clearer.
Both coherence claims should go into the probe crate before anything is built against them. The anyhow precedent makes me confident about the first, and the second is deliberately avoided.

Was	Now
Classified + #[kind(..)]	Classify::classify + #[derive(Classify)], #[classify(..)]
CallError::classified(e)	CallError::from(e), or just ?
free classify(&BoxError) -> CallError	CallError::from_boxed(BoxError)
Extract::extract (last message's suggestion)	FromCall::from_call

---

## First response: answers to REVIEW.md

Received 2026-10-03, answering the review's 27 refinements, 14 questions, the addendum and §13
decisions 1 and 4. On R6 the user had independently concluded the providers list should inherit the
`with` grammar, which is the fix chosen here. The user decided two of its three open points the same
day: R15 (`Unavailable` with `ErrorInfo.reason`) and the third error-naming branch (A6.4:
`ConnectRefused`, the core's `NoTimer` renamed `TimerMissing`). Decision 1 (per-module middleware)
was put back to the author with the current ulo design's argument (ADR-0056, proposed); see the second
response. The user signed off the rest of this response the same day, decision 4 included (connect
guards after the 101, `refuse = handshake` as a per-gateway opt-in).

I've checked every item on its merits rather than taking the review's word for it. The review is right on all 27 refinements, and the probes settle the questions that matter. For 21 of them I accept the change as written. For six (R4, R6, R7, R11, R14, R15) I accept the problem but choose a different fix, and the review's questions plus its two open decisions get a direct answer each.

### Refinements

**R1: accepted. My arm placement was wrong.** I listed the arms in priority order and put them on the wrong reference depths. Combined with A2: `Into<CallError>` on `&&Probe<Result<V,E>>`, `Into<BoxError>` on `&Probe<Result<V,E>>`, `IntoReply` on the bare probe, with the value in a `Cell<Option<_>>` as the core's factory probe already does.

**R2: accepted as written.** A free `const _` after the impl, naming the controller type. For a generic controller, a named associated const that `mount` reads.

**R3: accepted.** `__FwShared<V0, ..>`, with each `value` entry's position in writing order becoming part of the `__handler` contract.

**R4: accepted, with one addition.** The macro appends `+ use<>` to opaque return types, as the review says. The consequence needs stating: a stream that borrows `self` then fails inside the handler body. The reply must be `'static` anyway, so that's correct, but the error will surprise people. So also support `self: Arc<Self>` as a handler receiver (stable arbitrary-self type). A streaming handler then captures the `Arc` instead of borrowing, which is the natural idiom: `async fn watch(self: Arc<Self>, ..) -> impl Stream<..>`. The generated call already holds the controller in an `Arc`.

**R5: accepted.** `meta` goes into the inert list, is parsed at both tiers, and is carried in `__handler`. A stray `#[meta]` is a compile error. The WebSocket hooks are covered under Q1, and they stop being handlers.

**R6: accepted, and the choice is to extend the grammar.** The providers list gains `with = ..` and `with(scope) = ..`. There's one more inconsistency to fix while doing it: today a *bare* closure in a providers list lowers to `singleton`, an explicit scope, while `with = closure` is `Auto`. Under "scope is its own axis" (core §3.3), the same closure must mean the same thing everywhere, so the bare closure becomes shorthand for `with = closure`, meaning `Auto`. For a provider, `Auto` behaves as a singleton, so nothing breaks. The only differences are the wiring hint and the hooks rule, which already treats `Auto` providers as singletons (D9).

**R7: accepted, and the builder gets a different name.** Handler parameters must reach the wiring pass, so the builder gains `.dependencies(..)` filled from each parameter's `FromCall::dependencies`, and the input walk adds those reads to its roots. For the name, take `HandlerSpec`, which matches the core's `EnhancerSpec` and also settles A6.5.

**R8: accepted, taking the second option (X14).** Add `AppHandle::redact(BoxError) -> Redacted`, which runs the graph's own redaction. Transports hold an `AppHandle` from `Mounted`. One redaction rule is worth one core method, and `Malformed { param, source: Redacted }` stays.

**R9: accepted.** One `impl<T, P: FromCall<T>> FromCall<T> for Option<P>`, forwarding `CONSUMES_BODY`. `Valid<P>` gets the same shape.

**R10: accepted.** The skipping sentence was wrong. Emit every pair.

**R11: accepted, with the core's own shape.** `StartupError::Configure(ConfigureErrors)`, mirroring `Wiring(WiringErrors)`, with one entry per failure carrying `{ transport, source: Redacted }`. That's more cohesive than a bare `Vec`.

**R12: accepted.** `module_meta` yields `(ModuleRef, Arc<T>)`. For resolving inside the request's execution, choose `ModuleRef::with_execution(&ExecutionRef) -> ModuleRef`. A `ModuleRef` already carries an optional execution in the core's model, so this just sets it, which is a smaller addition than a new resolver entry point.

**R13: accepted.** Once X4 exists, transports ship no input module. A declaration arriving by both paths with the same `(key, seeder)` counts as one. `InputDecl::declared_in` gains a transport origin, so reports print "declared by transport `Http`" rather than inventing a module. The consequence the review notes, that an unmounted transport declares nothing, is accepted and documented.

**R14: accepted, with a smaller fix.** `route` takes `impl Into<Cow<'static, str>>`. For configured paths, the common need is a prefix, so `ModuleDef::controller::<C>()` returns a handle with `.at(prefix)`, a runtime value applied to every route and gateway path of that controller (NestJS's controller path). It covers GraphQL and health endpoints without exposing `Mount` closures. `controller_with(closure)` stays possible later if a case appears that a prefix can't express.

**R15: accepted, and this needs a decision.** The review is right that brokers can't produce `unimplemented` server-side, and that NATS can't tell "no handler" from "server down". [32] demands one uniform answer, so I'd choose **`Unavailable` for "no handler reachable for this pattern" on every link**, with `ErrorInfo.reason` saying what the link actually knows: `"pattern_unhandled"` where it's certain (TCP and UDP, answered by the server), and `"no_destination"` where it's ambiguous (NATS no-responders, a Redis `PUBLISH` count of 0, AMQP `basic.return`, a Kafka unknown topic). The conformance suite asserts the kind and accepts either reason. The alternative, `Unimplemented` everywhere, would be a false statement whenever the server is simply down.

**R16: accepted.** Handlers are checked in `prepare` from the recorded shape. A client call fails with `RpcError` before any I/O.

**R17: accepted, in A1's form.** The transport tests `cancel_reason()` before calling `from_boxed`.

**R18: accepted.** Split on CR, LF and CRLF. `Event::id` and `Event::event` take `EventId` and `EventName` newtypes, built fallibly, refusing line terminators and (for the ID) U+0000. A bad value fails where it's built, not mid-stream.

**R19: accepted.** Add `url.scheme` and `http.response.status_code`, with gRPC span names as `$package.$service/$method`.

**R20: accepted.** `type` is `about:blank`, `title` is the status phrase, and `detail` is `public_message`.

**R21: accepted, with `Bearer` as the default** (RFC 6750). It's configurable, and `CallError::unauthorized(challenge)` overrides it per error.

**R22: accepted. Answer the envelope without an `id`.** "Fire-and-forget" means no ack on success, not silence on failure, so [26] holds unamended, and a guard's refusal stays visible to the client.

**R23: accepted.** A write-once reason slot, with `cancel_with` on both `Execution` and `ExecutionRef`. `cancel()` stays as `cancel_with(CancelReason::Explicit)`, a new variant, so a cancelled execution never answers `None`. X10's outcome slot changes shape (Q8).

**R24: accepted.** Both behaviors are stated: resolvers created before `route_to` keep the root module, and one key can produce two instances in one execution across the switch.

**R25: accepted.** `Closed` joins the list.

**R26: accepted.** Both wordings get corrected: close code 1013 lives in the IANA registry that RFC 6455 established, and RabbitMQ's prefetch applies per channel or per consumer, not per connection.

**R27: accepted.** `BackendLimits` gains `upgrades`. actix wires upgrades through its service-level hook if the adapter can, and otherwise declares `upgrades: false`, so `prepare` refuses a gateway on its port.

### Questions

**Q1: WebSocket hooks.** These shouldn't be handlers at all. The core's own principle is that a role comes from the traits a type implements, with no marker attributes. So the hooks become traits on the gateway type: `OnConnect`, `OnDisconnect`, and `AfterInit`. They then don't receive `__handler`, and impl-level enhancers don't reach them:
- **The connect phase** is a `WsConnect` handler that `fw-ws` mounts itself. Its guards are the gateway attribute's `connect_guards(..)`. Its body calls `OnConnect` if the type implements it, which `fw-ws` detects by probing the concrete type.
- **Impl-level `#[guards]`** apply to message handlers (`Ws`) only.
- **`on_disconnect`** runs as a terminal execution without guards. Its failures and panics are logged.
- **`after_init`** is a once-per-gateway lifecycle call, not an execution.

There's one protocol need. `#[routes]` doesn't know the impl is a gateway, so each `fw_ws::message` mount function calls `<Self as GatewayConfig>::mount_gateway(m)`. That needs **X15, `Mount::once::<K>(..)`**, which makes the call idempotent per controller type. A gateway with no message handlers mounts nothing, and the gateway attribute refuses that case at compile time.

**Q2: `open` refused during the drain.** No execution means no pipeline. The transport answers directly:
- HTTP: 503 with `Retry-After` and `Connection: close`.
- gRPC: UNAVAILABLE.
- TCP RPC: an `err` frame of kind `unavailable`.
- WebSocket: nothing, because busy connections have already stopped reading.

Global middleware doesn't run for these requests, and that should be documented. §10's table gains a row for it.

**Q3: where the first item is pulled.** Not inside `dispatch`. The sentence about the first item goes: every item error, the first included, takes the late path, so SSE headers are never held back.

On `Ok` during the late path, the review is right that suppressing the error was the wrong reading. An `Ok` from a handler written for the pre-stream case is a failure rendering, and turning it into a clean end would tell the client "complete" when the stream failed. So the late-path rules are:
- `Ok(_)` is ignored and logged, and the original error renders canonically.
- `Err(e2)` reshapes the error.
- A clean end requires an explicit sentinel, `Err(fw::EndStream)`.

**Q4: the GraphQL endpoint is a mounted handler.** It's a controller in `fw-graphql-http` mounted `.at(config.path)` (R14), so role keys and inputs work as for any route.

**Q5: forwarding deadlines from inside an execution.** There's no ambient execution, so make it explicit. `RpcClient` calls take `.within(&exec)`, which forwards the remaining deadline and also cancels the call when the execution is cancelled. For tonic, `fw_grpc::outgoing(&exec, request)` sets `grpc-timeout`. The claim that this happens automatically comes out of the design.

**Q6: sessions.** `Session<T>::describe` declares a third WebSocket input, `SessionHandle`, seeded into every connection-scoped execution: the connect phase, every message, and `on_disconnect`. The session lives until the last execution holding it ends, so it *is* readable in `on_disconnect`.

**Q7: a `Path<T>` with no names.** The check follows what the probe can record:
- A struct is checked by its field names.
- A tuple is checked by count.
- A scalar or newtype requires exactly one parameter.
- A map, a flattened struct, or a `deserialize_any` impl is skipped and documented as unchecked.

§12's row is reworded to match.

**Q8: who awaits the stream outcome.** Nobody should have to. Replace the future with a callback: `exec.on_stream_end(|outcome| ..)`, called synchronously when the reply stream finishes. That's runtime-free, needs no spawned task, and works from an interceptor. The outcome is the *reply* stream's. An inbound stream's end is already visible to the handler that reads it. This also folds X10 into X5, as §11 suggested.

**Q9: dropping an unread body.** That's not a disconnect. `Disconnected` fires only when the backend observes the peer close the connection or reset the stream. The documentation for the `body` field gets corrected.

**Q10: TLS on the standalone WebSocket server and the TCP link.** Yes for both. The standalone WebSocket server (`wss`) and the TCP link both take `fw_net::Tls`.

**Q11: `Retry-After` values.** Load shedding sends a configured value, `.shed_retry_after(..)`, defaulting to 1 second. An `Unavailable` error without a `RetryAfter` detail sends no header, since RFC 9110 makes it optional.

**Q12: Kafka ordering.** Keying by correlation ID was a mistake. The partition key becomes a caller-supplied ordering key, defaulting to the client instance's ID, so one caller's requests stay ordered. The correlation ID moves to a header.

**Q13: socket-activation environment.** Set `FD_CLOEXEC` on every inherited socket: yes. But *don't* unset the environment variables. In edition 2024, `std::env::remove_var` is `unsafe` because it races with other threads, and under `#[tokio::main]` the runtime's threads already exist when user code runs. The systemd protocol already covers stale variables: `LISTEN_PID` must equal the reading process's ID, so a child spawned later rejects them. `fw-net` enforces that check, and the documentation says why the variables stay.

**Q14: non-object `Detail::Json` on gRPC.** Use `google.protobuf.Value`, which holds any JSON value, packed in `Any`, for every `Detail::Json`, not only non-objects.

### Decisions 1 and 4

**Decision 1: I now recommend reversing it, to module-local.** The review's evidence shows the two models cost the same and both have precedent. What decides it is the observation that under app-wide matching, the middleware stack for a route depends on *import order*, which nothing at the route declares. Under module-local matching, a route's middleware comes from its own module, which is explicit and local. App-wide needs are still covered: the global pre-routing stage applies to every request, and guards are the better tool for authentication anyway. It's your call, but I'd take module-local.

**Decision 4: keep connect guards after the 101.** The review's point is decisive here: a browser can only learn *why* it was refused through a close code after the handshake. Refusing the handshake itself becomes an opt-in per gateway (`refuse = handshake`), for non-browser clients and proxy logs.

### The addendum

**A4: yes, that split is the intent.** Use separate crates, as the review proposes, matching the rest of the layout: `fw-graphql` (neutral), `fw-graphql-http`, and `fw-graphql-ws`. The neutral crate exports the engine under `dyn Engine`, so any handler can take `Dep<dyn Engine>`. That drops the `Graphql` wrapper service I proposed last time, which added nothing over the binding itself. `GraphqlModule` becomes the HTTP binding's module.

**A5: already settled.** You approved `fw_http::middleware::Next`, and no alias ships. Users who import both `Next` types write `use .. as ..` themselves.

**A6, the five items to settle:**
1. **`ExtractError` beside `from_call`: confirmed.** The rule is written as "an error is named after the operation as users understand it", as `FromStr` fails with `ParseIntError`. The rule's text cites that example, so it doesn't get reopened.
2. **`Answer::from_result` is dropped.** `r.map_err(CallError::from)` covers it.
3. **`CallError::from_boxed(BoxError)`, by value.** P27b shows coherence *would* accept `From<BoxError>`, so my coherence reason no longer holds. The remaining reason does: a recognizer shouldn't run invisibly on every `?`.
4. **Condition-named errors are the rule's missing third branch.** The new branch: "an error that reports a condition rather than an operation's failure is named for the condition", as std does with `PoisonError`. Within that branch, use the past-participle form the core already uses (`GuardRejected`, `PanicRecovered`), so `Refusal` becomes **`ConnectRefused`** and `NoTimer` becomes **`TimerMissing`**. `Closed` stays: the subject (the app) is implied wherever it appears. `Redacted` isn't an error at all. It's a wrapper type, named for what was done to its contents, which is correct.
5. **Abbreviations go.** `HandlerDecl` becomes `HandlerSpec` (R7), and `InputDecls` becomes `Inputs`.

### New SPI items from this round

- **X14:** `AppHandle::redact` (R8).
- **X15:** `Mount::once::<K>()` (Q1).
- **X5 extended:** `CancelReason::Explicit` and `on_stream_end` (R23, Q8). X10 is retired.

What needs your answer: R15 (`Unavailable` for unhandled patterns), decision 1 (module-local middleware), and the third error-naming branch (A6.4).

---

## Second response: per-module middleware and tower

Received 2026-10-03. The user asked the author to weigh the current ulo design's proposed position
(ADR-0056: retire per-module middleware, keep one path-scoped pre-dispatch stage on every transport)
against app-wide-by-pattern and module-local, and whether both modes could be kept; then where tower
fits. The user signed it off the same day: option 3 (no per-module middleware; one pre-dispatch stage
in two sub-steps) and the tower answer. The module tier for the enhancer stack waits until a case needs it.

### Per-module middleware

Short version: **option 3, no per-module middleware.** The pre-dispatch stage covers the app-wide case, controller interceptors cover the local case, and nothing real falls between them.

| | App-wide by pattern | Module-local | No per-module middleware |
|---|---|---|---|
| Shared auth module protecting others | ✓ | ✗ | ✓ through pre-dispatch, path-scoped |
| Failures inside the error chain | ✗ | ✗ | ✓ in both places |
| Order decided by something visible | ✗ (import order) | ✓ | ✓ (declaration order) |
| Works on all four transports | ✗ (HTTP only) | ✗ | ✓ |
| Concepts a user learns | two stages + interceptors | two stages + interceptors | one stage + interceptors |

**Do you need both modes?** Only if some behavior must run *after routing but before guards*, scoped to *one module*, and can't be expressed as a path scope. I can't find one:
- **Auth for a group of routes** is pre-dispatch scoped by pattern. It runs before guards, so it can set `CurrentUser`.
- **Logging, metrics, or transforming a module's responses** is an interceptor.
- **Rejecting a request** is a guard.

Two details to settle so option 3 holds up:

1. **Pre-dispatch has two sub-steps.** Path rewriting has to happen before route matching, while scoping by route pattern needs the match. So: unscoped entries run first, and can rewrite and see misses; then the route is matched; then pattern-scoped entries run; then dispatch. It's still one stage to the user, in declaration order, and both sub-steps are inside the error chain. Only global error handlers apply to a miss, since no handler matched.

2. **"Every controller in this module" is the one gap**, since interceptors are per controller. If that need appears, don't bring middleware back. Add a **module tier to the enhancer stack** (global, module, controller, method). It's inside the error chain, ordered explicitly, and has the same semantics as the tiers that already exist. Until someone needs it, `.at(prefix)` (R14) plus a pattern-scoped pre-dispatch entry covers it.

This also supports the default you liked: "app-wide by pattern" survives intact as pre-dispatch's scoping. What's retired is the second stage that sat outside the error chain.

### Tower

#### Where tower fits in each option

| Option | Where layers run |
|---|---|
| App-wide by pattern | the global pre-routing stage only, unscoped, HTTP only |
| Module-local | the same; the after-routing stage has no tower position |
| No per-module middleware (pre-dispatch) | both pre-dispatch sub-steps, so layers can be unscoped or path-scoped |

#### Path-scoped layers under pre-dispatch

Yes, path-scoped layers work. Tower layers are composed once, not per request, and the route table is known in `prepare`. So:

- **Unscoped layers** wrap the whole stage, before matching. They can rewrite and they see misses.
- **Scoped layers**: for each route, `prepare` builds that route's own stack from the layers whose pattern matches it, in declaration order. A request is matched, then runs its route's prebuilt stack. There's no per-request composition, and routes with the same set of layers share one stack.

```rust
m.meta::<fw_http::PreDispatch>()
    .layer(TraceLayer::new_for_http())                          // unscoped
    .layer_for(["/files/*"], RequestBodyLimitLayer::new(50 * MB))
    .apply_for::<ApiKeyAuth>(["/admin/*"]);
```

Being in the error chain needs two clarifications:
- **A layer's `Err` and its panics** go to the error handlers, as `BoxError` and `PanicRecovered` respectively.
- **A response a layer builds itself**, such as tower-http's auth answering 401, is a response, not an error, so error handlers don't see it. That's inherent to tower, and the documentation should say so.

#### gRPC

Yes. gRPC is HTTP/2, so `http::Request` and `http::Response` are already its types, and tonic is built on tower. Give gRPC the same pre-dispatch stage with `.layer` and `.layer_for`, scoped by gRPC method paths (`/users.v1.UserService/*`).

One spec point decides how layer responses work there. A generic HTTP layer that answers with a non-200 HTTP status isn't a valid gRPC response. The gRPC transport translates such a response using the gRPC spec's HTTP-to-status mapping (401 to UNAUTHENTICATED, 403 to PERMISSION_DENIED, 429 and 502–504 to UNAVAILABLE, and so on), so clients still receive a proper `grpc-status`.

#### WebSocket and RPC

Their pre-dispatch stage takes `Middleware<T>` only, not tower. Their requests aren't HTTP, and the tower-http ecosystem doesn't apply to them. That's enforced by the type system: `.layer` exists on the HTTP and gRPC pipelines alone, so trying it on RPC is a compile error, not a runtime surprise.

---

## Third response: the two choices made in the fold

Received 2026-10-03, answering the fold's two questions. The user signed it off the same day.

### 1. WebSocket keeps `unimplemented`

Leave it as written. [32] is about RPC: "a pattern nothing handles is reported the same way on every transport" means every RPC *link*, because a caller using `RpcClient` shouldn't see different answers depending on which broker sits underneath. RPC chose `Unavailable` because most links *can't* know whether a handler exists, so the uniform answer had to be the one that's true on all of them.

A WebSocket gateway isn't one of those links, and it does know for certain. The rule behind both decisions is "answer what's true", and for WebSocket that's `unimplemented`.

### 2. Keep two entries for GraphQL, but add a form for the real gap

**For GraphQL, the two-entry form is right.** `AsyncGraphql<S>` is the adapter crate's own `Construct` type, so `AsyncGraphql<ApiSchema> as dyn Engine` is the natural spelling. Each entry does one job: the closure builds the schema, and the type binds the adapter under the trait.

**But the same split leaves a real gap elsewhere.** `X as dyn T` lowers to `provide::<X>().also_as(..)`, which requires `X: Construct`. A third-party type built by a closure, such as `RedisCache::new(cfg)` that should be read as `dyn Cache`, has no macro spelling today. The user has to implement `Module` by hand. That's a gap, so it's worth a spelling.

**Don't spell it `with = .. as dyn T`.** A closure's body extends as far as it can, so in `with = |cfg| RedisCache::new(cfg) as dyn Cache`, the `as dyn Cache` parses as a cast *inside the body*. The user then gets "non-primitive cast", which says nothing about bindings. Parentheses would fix the parse, but forgetting them would be a common mistake with a confusing error.

**Use a key-first form instead, mirroring `into K: [..]`:**

```rust
providers = [
    dyn Cache: with = |cfg: Dep<RedisConfig>| RedisCache::new(cfg),
    dyn Cache: with(execution) = |..| ..,          // scope axis, as everywhere
]
```

This lowers to `.with(f).also_as::<dyn Cache>(|a| a)`, the closure's handle plus a coercion, which the value API already supports. As with `X as dyn T`, the concrete type stays bound in the module too, and exports decide what leaves the module. It's unambiguous to parse, it reads as "this key is provided by this closure", and its shape matches the `into` lists users already know: `K: [..]` contributes to a collection, while `K: with = ..` provides a single binding.
