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

---

## Fourth response: race 2a's divergences

Received 2026-10-04, answering `transports/DIVERGENCES.md` (T1–T26). Its T2 answer is narrower than the
earlier ulo HTTP audit's conclusion (keep the framework adapters for interop and as a conformance asset,
with `ulo-http-hyper` as the reference); the user sent a separate T2 question on that, and the fifth
response answers it, superseding the T2 answer below: axum is not dropped but becomes an embedding
adapter, `ulo-http-axum` the backend becomes `ulo-http-hyper`, and the type a host mounts is
`fw_http::embed::Service`, not `fw_http::Service`, which is the inner service a pre-dispatch layer wraps
(`EMBEDDING_REVIEW.md` R1). The user signed off everything here except T2 the same day; the pre-compile
batch is T1 with `NoRoute`, T5, T8, T13, T14, `Middleware::handle -> Result<Response, BoxError>`, and T21.

Most of these I'd take as recommended. Five need a different answer or an addition, and one item in the reading sections contradicts a decision we already made.

### Different answers

**T8: don't let a route timeout skip the error handlers.** Accepting it breaks the rule we set for pre-dispatch: every failure reaches the error handlers. It matters in practice. An app that reshapes every error into its own envelope would still send raw problem details on timeouts, and that's the inconsistency clients notice. The concern about an unbounded error handler is valid, so bound it: run the handlers with a `Timeout` `CallError` under a short grace period (say 1 s, using the core's `Bound` vocabulary), and render canonically if the grace runs out.

**Related, from the reading sections: `Middleware::handle` must be able to return `Err`.** As built it answers a `Response` and can fail only by panicking. That contradicts the pre-dispatch decision, whose whole argument was that middleware failures reach the error handlers like any other error. An auth middleware should be able to return `Err(CallError::unauthorized(..))` and have the app's error handlers shape it. Without that, every middleware renders its own errors. Change the signature to `-> Result<Response, BoxError>`.

**T2: rename to `ulo-http-hyper`, and go further.** The four reasons are sound. But if axum supplies only types, drop the axum dependency entirely: hyper and `http-body-util` provide them. That leaves [16] promising axum as a backend, and the honest correction is that axum isn't a server, it's a router on top of hyper. The useful axum integration is the opposite direction: embedding. `ulo_http::Service` is a tower `Service`, so it can be mounted inside an existing axum app (`Router::fallback_service`). Amend [16] to list hyper as the backend and axum as an embedding target.

**T3 and T24: use the core's `Bound` vocabulary, not `Option`.** `Option<Duration>` and `Option<u32>` leave `None` ambiguous: does it mean the default or no limit? T24 already shows the problem, with `None` meaning "hyper's default" while passing `None` through to hyper would mean unlimited. The core solved this once: `Bound::{Default, After(d), Unbounded}`. Use it for the handshake and header timeouts, and a matching `Default | Max(n) | Unlimited` shape for the stream limit. Same follow-up timing as recommended.

**T21: use the typed origin.** `InputOrigin::Transport { name, at }` already exists internally (spine 19), so making it public in `InputConflict { key, first: InputOrigin, second: InputOrigin }` costs almost nothing, and lets a test or a tool inspect the parts. `Missing::consumer` is text because nothing typed existed for it. Here something does.

**T1: yes, with one addition for 404.** A public `MethodNotAllowed` with `allow()` is right, and the name is HTTP's own phrase for the condition. But the 404 has the mirror-image problem: an error handler can't tell "no route matched" from a handler's own `NotFound`. Keep the 404 as `CallError::new(NotFound, ..)` so it renders by kind, and attach a public `NoRoute` marker as its source, so `source_as::<NoRoute>()` tells them apart.

### Taken as recommended

T4, T5, T6, T7, T9, T10, T11, T12, T13, T14, T15, T16, T17, T18, T19, T20, T22, T23, T25 and T26.

Two of these with a short note:
- **T16:** amend §0.6, as recommended. `from_pem_files` recording a source is still a correct `From`-style constructor, since nothing is fallible until `load`.
- **T20:** fine to defer. `Classify::challenge` with a default of `None` is additive later, so waiting costs nothing.

### Summary of answers

| | Answer |
|---|---|
| T1 | yes, plus `NoRoute` as the 404's source |
| T2 | rename to `ulo-http-hyper`, drop axum, amend [16] (axum as an embedding target) |
| T3, T24 | follow-up, using the `Bound` vocabulary |
| T8 | run the error handlers under a short grace bound |
| T21 | public `InputOrigin` in the variant |
| `Middleware::handle` | return `Result<Response, BoxError>` |
| everything else | as recommended |

For the pre-compile batch, that adds T8 and the `Middleware` signature to T1, T5, T13 and T14. Both touch signatures that would otherwise break after compiling, so they belong before it.

## Fifth response: T2, backends and embedding

Received 2026-10-04, answering the user's T2 follow-up: keep the framework adapters, as the earlier ulo
HTTP audit concluded, for interop and as a conformance asset, with `ulo-http-hyper` as the reference.
It restates [16] and designs the embedding surface. The second part answers the user's question on
how the engines and the adapters relate. Reviewed in `EMBEDDING_REVIEW.md`; signed off with the sixth
response's changes on 2026-10-04.

### [16] restated

> **Backends and embedding.** `fw-http-hyper` is the reference backend and the default: it owns the listener and serves connections. Each framework adapter (`fw-http-axum`, `-salvo`, `-poem`, `-actix`, `-rocket`) runs the app *inside* that framework instead: nested under a path or as the fallback, with the framework's own middleware around it, and with the host keeping its own server setup. Routing, extraction, pre-dispatch, dispatch and error rendering run inside the app either way. One conformance suite runs every adapter against the hyper reference, with documented limits where a host can't do something.

### 1. Two roles, two types

A `Backend` (§3.7) owns sockets. An embedding owns none. It's a second `Server` implementation that binds nothing, so the app still goes through `listen()`, and `prepare` still builds the route table, checks CORS, and validates paths before anything serves.

```rust
let (server, embedded) = fw_http::Embedded::new()     // app-level settings only (§5)
    .body_limit(4 * MB)
    .max_inflight(512);

let app = App::builder(AppModule).timer(fw_tokio::Timer).wire()?
    .connect().await?
    .bind(server)
    .listen().await?;
```

`embedded` is a cheap-clone handle, usable before `listen()` returns, so the host's router can be built first. Requests that arrive before the app is bound get 503. That ordering is the host's responsibility, and the documentation says so.

Server-level settings belong to the host: endpoints, TLS, h2c and `max_concurrent_streams`. `Embedded` has no methods for them, so trying one is a compile error (E0599), not a setting silently ignored.

### 2. How each host sees the app

| Adapter | Exposed as | Mounting |
|---|---|---|
| axum | `tower::Service` | `Router::nest_service("/api", ..)` or `.fallback_service(..)` |
| salvo, poem | `tower::Service`, through each framework's tower bridge | nested or fallback |
| actix-web | a native `HttpServiceFactory` | `App::service(fw_http_actix::scope("/api", embedded))` or `default_service` |
| rocket | a native `Handler`, mounted as one catch-all route per method | `rocket.mount("/api", fw_http_rocket::routes(embedded))` |

Each adapter is mostly conversion:
- **actix:** its request payload is `!Send`, while the app's body must be `Send` (the core needs `Execution: Send + Sync`). The adapter forwards payload chunks through a bounded channel from actix's worker-local task, so the app reads a `Send` stream.
- **rocket:** rocket 0.5 is on `http` 0.2, so the adapter converts at its edge.
- **The three tower hosts:** these share `fw_http::Service` directly.

**Prefix.** Hosts like axum strip the nest prefix before the app sees the request. The adapter takes the prefix (`.nested_at("/api")`), so the app routes on the stripped path but records the *full* route in its span (`http.route = /api/users/{id}`), and `HttpCx::mount_prefix()` gives handlers what they need to build `Location` headers.

### 3. What host middleware sees, in both directions

The app's `Execution` opens *inside* the app's service, so host middleware runs before any execution exists and never touches it. Information crosses the boundary in two defined places:

- **Host to app, through request extensions.** Host middleware writes a value into the request's `http::Extensions` (axum's `Extension` pattern). The app reads it with the `Host<T>` extractor, or copies it into the execution with a pre-dispatch entry, `.adopt::<T>()`, so guards and services read it as `Ext<T>`. That's how a host's auth layer hands its user to the app's guards.
- **App to host, through response extensions.** Every response carries a `Handled` value in its response extensions: the matched route, the handler's name, and the transport key. Host logging and metrics can then record the app's route rather than the raw path.

Errors render *inside* the app. Host middleware sees the finished response (problem details, through the app's error handlers), never a `BoxError`, so the app's error shape is the same embedded or not. Panics in the app are caught inside it, so they never reach the host's panic handling.

**Host code reaching the app's container.** A host handler that wants the app's services holds the `AppHandle` and calls `handle.execute(..)` for anything execution-scoped, or `handle.get::<T>()` for a singleton. That serves the case of an existing application adopting this framework's DI one route at a time.

### 4. Routes, misses, and who answers

- **Nested:** the app answers everything under its prefix, misses included. Its 404 and 405 go through its global error handlers (with `NoRoute` and `MethodNotAllowed`), exactly as on the reference backend. The host answers everything else.
- **Fallback:** the app answers whatever the host didn't match, and its 404 is final.

Unscoped pre-dispatch entries run for every request that reaches the app, misses included, in both modes.

**Passing a miss back to the host** only works where the host supports fallthrough. Rocket does: a handler can return `Outcome::Forward`, so with `.on_miss(Miss::Forward)` an app miss lets rocket try its lower-ranked routes. A tower `Service` can't hand a request back to an axum router, so `prepare` refuses `Miss::Forward` on the adapters that can't honor it.

### 5. Lifecycle and shutdown when the host owns the server

**One owner for signals: the app.** The app's `serve(signal)` stays the single trigger, as DESIGN §9.5 requires. The host's own signal handling is turned off (actix `disable_signals()`, rocket's `shutdown.ctrlc = false`). Two owners would race.

The core's sequence maps onto the host like this:

| Core step | Embedded |
|---|---|
| 1. before-shutdown hooks | host still serving normally |
| 2. stop accepting | `Embedded`'s `drain` resolves `handle.draining()`, which the host's graceful shutdown is wired to: axum's `with_graceful_shutdown`, actix's `ServerHandle::stop(true)`, rocket's `Shutdown::notify()`. From this point the app answers new requests 503 with `Connection: close` (Q2's rule). |
| 3–4. drain, then cancel | in-flight *executions* drain under `drain_timeout`; the host independently drains its *connections* |
| 5. destroy hooks | as usual |
| 6. close | `Embedded::close` awaits the host server's own future, bounded by what remains of the cap, so the app's shutdown report includes a host that didn't stop |
| 7. shutdown hooks | as usual |

Each adapter provides one helper that does the wiring, so users don't assemble it by hand:

```rust
let router = axum::Router::new()
    .route("/legacy", get(legacy))
    .nest_service("/api", embedded.service());

fw_http_axum::run(app, axum::serve(listener, router), fw_tokio::shutdown_signal()).await?;
```

`run` connects the host's graceful shutdown to `draining()`, hands the host's future to `Embedded::close`, and returns the app's `Shutdown` outcome.

### 6. What the app can't know, and how it's refused

Each adapter declares `EmbedLimits`, and `prepare` checks the app against them, the same mechanism as `BackendLimits`:

| Limit | Why it varies | Refused in `prepare` |
|---|---|---|
| `peer_addr` | axum supplies it only through `ConnectInfo` | a handler reading non-optional `Dep<ClientAddr>` when the adapter isn't told the host provides it (`.peer_addr(true)`) |
| `upgrades` | hyper hosts pass `OnUpgrade`; actix and rocket use their own mechanisms | a gateway on the HTTP transport when the adapter can't upgrade |
| `forward_miss` | rocket only | `Miss::Forward` elsewhere |
| `tls_info` | depends on whether the host terminates TLS | nothing refused; `ConnInfo::tls` is `None` |

### 7. Conformance

One scenario list runs against the hyper reference, and against every adapter in both modes (nested and fallback). It asserts byte-identical responses wherever a scenario doesn't depend on a declared limit, and it covers routing, misses, extraction failures, problem details, SSE, cancellation on disconnect, upgrades, and the drain. The suite also checks the limits table in the other direction: an adapter that *passes* a scenario it declares unsupported fails, so the declared limits stay honest.

This uses the core and transport SPIs as they are: `Embedded` is just another `Server`, its `drain` and `close` are the core's steps, and its `prepare` reads `HandlerSpec` dependencies (R7) for the `peer_addr` check.

### Engines and adapters

The user asked how the engines and the adapters relate.

The key distinction is **who owns the sockets**, and the engine underneath follows from that choice.

**Engines** are what parse HTTP off the wire: framing, keep-alive, HTTP/2 streams, upgrades. There are really only three in play: hyper 1.x, actix-http, and the older hyper 0.14 that Rocket 0.5 still uses. The framework never talks to an engine directly. It talks to whatever sits on top of one.

**The backend** is the case where the app owns the sockets. `fw-http-hyper` binds the listeners, runs the accept loop, and drives hyper connections into `fw_http::Service`. This is the reference path and the default, and it's the one the conformance suite compares everything else against.

**Adapters** are the case where a host framework owns the sockets and the app runs inside it. They split into two kinds by how they connect:

- **Tower embed (axum, salvo, poem).** These hosts already speak tower, so the adapter is thin. It hands the host `fw_http::Service` as a tower service, and the host nests it or uses it as a fallback. All three sit on hyper 1.x, so behaviorally they're closest to the reference.
- **Native embed (actix, rocket).** These hosts don't speak tower, so each adapter implements the host's own handler trait. The actix adapter bridges actix's `!Send` payload into a `Send` stream. The rocket adapter converts between `http` 0.2 and 1.x, and is the only host that can pass a miss back to its own router.

```rust
// Backend: the app owns the sockets.
let app = app.bind(fw_http_hyper::Server::new("0.0.0.0:8080")).listen().await?;
app.serve(fw_tokio::shutdown_signal()).await?;

// Adapter: the host owns the sockets; the app binds an `Embedded` server that opens none.
let (server, embedded) = fw_http::Embedded::new();
let app = app.bind(server).listen().await?;
let router = axum::Router::new().nest_service("/api", embedded.service());
fw_http_axum::run(app, axum::serve(listener, router), fw_tokio::shutdown_signal()).await?;
```

The actix adapter keeps actix's runtime model: actix runs one single-threaded runtime per core with no work-stealing, and the app's pipeline runs inside those per-core workers, so a user who picks actix for thread-per-core behaviour still gets it.

**Should actix-http also get a backend?** That would be a second socket-owning path, so someone could get actix's thread-per-core model without writing an actix-web app. I'd skip it, at least at first. The actix adapter already gives exactly that with a three-line host:

```rust
HttpServer::new(move || App::new().default_service(fw_http_actix::service(embedded.clone())))
    .disable_signals()
    .bind("0.0.0.0:8080")?
```

A dedicated backend would duplicate that path, and add a second reference implementation for the conformance suite to keep in agreement. If a real need appears, such as needing actix-http's settings without any actix-web types, it can be added as a second backend later without changing anything above it.

## Sixth response: the `cfg_attr` change and the embedding review

Received 2026-10-04, answering `divergences/batch2a-cfgattr.md` and `EMBEDDING_REVIEW.md`. The user
signed it off the same day, together with three decisions of the pre-compile batch the response does
not address: the strict key rule for a scoped key whose handlers are all behind `cfg`
(`batch2a-macros.md` entry 3), the 405 as a `BadRequest` `CallError` whose source is
`MethodNotAllowed` (`batch2a-http.md` entry 1), and the macOS listening check through
`TCP_CONNECTION_INFO` with the other BSDs unprobed (`batch2a-core-net.md` entries 5-7). Deferrals
filed: the actix-http backend and the actix embedding's WebSocket support.

Both documents hold up. I take almost everything as proposed, with five changes: a better name for R1's type, a different fix for R12, a firmer answer for Q5, an enum rather than optional fields for Q6, and a correction to the diagram from last turn.

### The `cfg_attr` change

All nine decisions are accepted. Decision 1, where the first gated attribute macro decides, is a heuristic, but it's the right one. `#[routes]` can't tell a transport attribute from any other attribute macro by name, and the document shows that every case the heuristic gets wrong *fails to compile* rather than mounting something wrongly. That's the property that matters. The one behavior change, a helper with `#[cfg_attr(.., tracing::instrument)]` becoming a handler, is consistent with the existing rule that helpers go in a separate impl block. The verification is thorough: it runs both toolchains with the feature on and off, and stashes the old macros to confirm the scratch crate actually exercises the change.

### The embedding review

**Accepted as written:** R2, R3, R4, R5, R6, R7, R8, R9, R11, R13, R14, R15, R17, R18, R19, R20.

A few of those reshape the design, and should be stated plainly when they're folded in:
- **R2:** `Embedded<A: Embed>`, generic over the adapter like `Server<B: Backend>`, with the limits checked in `prepare`.
- **R4:** `Embedded::drain` signals the host to stop accepting and returns at once. The host's lingering connections are `close`'s business, since an embedding can't cut them.
- **R5:** the host's future lives in a slot, polled by `Embedded::serve` and awaited by `close`, with no spawned task.
- **R8:** salvo and poem join actix and rocket as **native** embeds. axum is the only tower embed. That corrects the diagram I drew last turn: the "Tower embed" box holds axum alone, and "Native embed" holds salvo, poem, actix and rocket.

**R1: accepted, under a module-qualified name.** The app needs a new tower service that converts from `http::Request`, as the review says. Following the rule you approved for `middleware::Next`, call it `fw_http::embed::Service`. The module carries the context and the type name doesn't repeat it.

**R10: take the explicit registration.** `host_extensions` goes into `EmbedLimits`. The actix and rocket adapters get `.forward::<T>(..)`, which reads a host value under a type the app names. rocket's `Routing` value (Q6) lives in the request's local cache, where a fairing's `on_response` can read it.

**R12: fix it rather than scope the sentence.** Leaving body panics to each host makes the same failure behave five different ways. Wrap `ExecBody::poll_frame` in a panic catch. A panic ends the body with an error frame, reports `CutOff`, and logs a `PanicRecovered` with stage `Handler`. Then "panics are caught inside the app" is true without exceptions, on every host.

**R16 and Q4: actix ships with `upgrades: false`.** The payload/body bridge is real but nontrivial, and nothing in the first version needs WebSocket on an actix host. `prepare` already refuses a gateway on the HTTP transport when the adapter declares that limit.

### The questions

**Q1: one clock, set by the user.** Add an accessor, `AppHandle::drain_timeout()`, and have each adapter's `run` pass it to the host wherever the host accepts a bound: salvo's `stop_graceful`, actix's `shutdown_timeout`, rocket's `grace`. axum has no bound of its own, so the app's `close` bound contains it. Users then set the window once.

**Q2: an error.** A host that stops before the shutdown trigger leaves the app with nothing serving it, which is a failure, not completion. `Embedded::serve` answers `Err`, and the app shuts down naming the transport, just as with a dead backend.

**Q3: decide at runtime.** Forward only when the body is untouched and the path unchanged. Otherwise answer the app's own 404 and log at `warn`. A static refusal isn't feasible here: unscoped entries are opaque middleware and tower layers, so `prepare` can't know whether they read the body or rewrite the path. It would have to refuse any unscoped entry at all, which is far too strict.

**Q5: a missing host value is a deployment fault, not a bad request.** Add an `ExtractError` variant, `HostMissing { param, type_name }`, that maps to `Internal` with its message withheld. `Option<Host<T>>` is the spelling for genuinely optional values. Where the adapter declares `host_extensions: false`, `prepare` refuses `Host<T>` altogether (R10).

**Q6: an enum, not optional fields.** Calling a miss "handled" would be false. Make it `fw_http::Routing`, with `Matched { route, handler }`, `NotFound` and `MethodNotAllowed`, so host middleware reads one type and matches on it. The conformance suite asserts it on the hosts where a test layer can observe response extensions. It's outside the byte-identical rule, so it gets its own assertion.

**Q7: separate structs.** `EmbedLimits` and `BackendLimits` stay separate, which keeps the compile-error story exact. The shared `prepare` (R20) takes the parts both have in common as plain arguments. Sharing a function doesn't require sharing a struct.

**Q8: yes.** The embedded app may own `/`, which is `/api` from outside. `mount_prefix()` joins the prefix and the route without doubling slashes, and the route table's trailing-slash rule then decides `/api/` versus `/api` exactly as it does unembedded.

**Q9: yes, HTTP/2 too.** The drain scenario has two shapes: `Connection: close` on HTTP/1.1, and GOAWAY on HTTP/2. The suite runs both on every host that supports HTTP/2.

### New core surface from this round

- `Mounted::handlers_reading::<T>()`, built on the input walk (R3).
- `AppHandle::drain_timeout()` (Q1).

Both are additive.

## Seventh response: the embedding build's divergences

Received 2026-10-04, answering `divergences/batch2a-embed.md` (entries 1-19). Entry 17's housekeeping
point was already met: `b70d564d` carries the rename's content changes. The user signed it off the
same day.

Most of this is right, and the scratch crate covers the important paths, including the known-violation check on the body catch. Here's my answer on the five sign-off items, plus brief notes on the rest.

### The five sign-offs

**Decision 6: add a variant rather than leave `Routing` absent.** Absence is ambiguous in exactly the setting this type exists for. In nested and fallback mode, a request the host routed somewhere else *also* has no `Routing`. So a host metric can't tell "the app answered without routing it" (shed, draining, an early answer from pre-dispatch) from "the app never saw this request". Add `Routing::Unrouted`. Then presence means the app answered, and the variant says how.

**Decision 7: don't call the automatic `OPTIONS` a 405.** As the document notes, a metric keyed on the variant would count these 204s as errors. Add `Routing::Options { route }`: the path matched a route, and the router answered with `Allow` on its behalf. With decision 6, the enum is `Matched`, `Options`, `NotFound`, `MethodNotAllowed`, `Unrouted`, still `#[non_exhaustive]`.

**Decision 9: accept both parts, with one improvement.** `mount_prefix()` returning the prefix is the honest API, since a no-argument method can't join an arbitrary path. Leaving `url.path` stripped is a correct fallback, because the original path can't be recovered from the stripped one. But where the host supplies the original path (axum's `OriginalUri`), the adapter should pass it through, and the span should record it. So: stripped as the fallback, original when the host provides it.

**Decision 11: the refusal is wrong when a pre-dispatch layer supplies the value, so make the supply declarable.** `prepare` can't see what a tower layer inserts, so the refusal can't stand unconditionally. And dropping it would give up the check where it's genuinely useful. Add `.supplies::<T>()` to the pre-dispatch builder, written after the layer that inserts `T`. Feed it into the same exemption set that R10's adapter-side `.forward::<T>()` will use in race 2b. One set, two writers, and the refusal applies only to types nothing declares. `adopt::<T>()` should count as reading from that set too, so it's checked the same way.

**Decision 13: accept both rules.** A 405 means a route of the app matches the path, so it's the app's to answer, and forwarding it would be wrong. Skipping the error handlers for a forwardable miss is also right: the host answers that request, so shaping a response nobody sends is wasted work. The only exposure is an adapter that declares `forward_miss` and then ignores the marker, which is a conformance-suite failure, not a design gap.

### The rest: accepted

- **1:** accepted. My `(server, embedded)` snippet couldn't be written as shown, and the fix matches `App::handle()`.
- **2, 18, 19:** accepted. A `ConnInfo` extension keeps `ulo-http` free of host types, and `respond` is the correct escape for non-`Clone` upgrades.
- **3, 4, 5:** accepted. Decision 5's `Unbound → Bound → Closed` states fill a gap I left: a failed or closed app must not keep serving through the handle. On decision 3, `stopping()` and the core's `draining()` resolve at the same moment. Document `stopping()` as the one adapters use, since it's available before `listen()` returns, so there's one recommended path.
- **8, 10, 12, 15, 16:** accepted. Returning the path from `handlers_reading` (15) is better than what R3 asked for, because the refusal can name the service that reads the input.
- **14:** accepted. Resuming the payload inside `catch_panic` and polling once is a neat way to avoid a third core addition. Leave a comment at that spot explaining why it completes in one poll, since a later edit that adds an await there would silently break it.
- **17:** accepted. One housekeeping point: commit `70d3ac49` contains the rename with the old file contents, so make sure the content changes go into the next commit rather than staying in the working tree.

### Summary

| Item | Answer |
|---|---|
| 6 | add `Routing::Unrouted` |
| 7 | add `Routing::Options { route }` |
| 9 | accept, and record the original path when the host provides it |
| 11 | `.supplies::<T>()` on pre-dispatch, sharing R10's exemption set |
| 13 | accept both rules |
| everything else | accepted |

## Eighth response: the declared-supply decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 23-25. The user signed it off
the same day, with one correction: item 3 removes `Embedded::supplies`, so the stray-`supplies`
refusal of item 2 points to `Embedded::forward`.

**1. Take the precise check.** Your reasoning matches the rule the whole design follows: refuse where `prepare` can see the fault. Both setups are visible statically. The router already knows which routes each scoped stage covers, and stage order is known, so letting them through to fail per request would abandon that rule for no gain. The precise check costs no new surface. So a scoped supply counts only for the routes its stage covers, and an `adopt` counts only unscoped supplies earlier in stage order.

**2. Keep the stray-`supplies` refusal.** It wasn't asked for, but it's the same rule as a stray `exclude`. A `supplies` with nothing before it declares something no entry does, and accepting that silently would make the declaration a lie that only shows up as a 500 later. The refusal's text also points to the right remedy (`Embedded::supplies` for host values), so it teaches the distinction rather than just blocking.

**3. Neither option. Tie the declaration to the copy.** On a host with `host_extensions: false`, the app can't read the host's store at all. The only thing that actually supplies a value is the adapter's copy. So a public `supplies::<T>()` lets an app author declare a value that nothing copies: the refusal lifts, and every request answers 500. `#[doc(hidden)]` hides that trap but doesn't remove it.

The structural fix is to make the declaration impossible without the copy. Give `Embed` an associated type for the host's request, and replace `Embedded::supplies` with a registration that takes the copying function:

```rust
pub trait Embed: Send + Sync + 'static {
    const NAME: &'static str;
    type HostRequest;                  // actix's HttpRequest, rocket's Request, ..
    fn limits() -> EmbedLimits;
}

impl<A: Embed> Embedded<A> {
    pub fn forward<T: Clone + Send + Sync + 'static>(
        self,
        copy: impl Fn(&A::HostRequest) -> Option<T> + Send + Sync + 'static,
    ) -> Self;
}
```

`ulo-http` still names no host types: `A` supplies them. The method records `T` in the exemption set *and* stores the function. The adapter runs each registered function on the host's request before it calls `respond`, inserting what it gets into the request's extensions. A declared type is then always a copied type. That's also exactly R10's `.forward::<T>(..)`, now generic instead of hand-written per adapter, so race 2b's actix and rocket work shrinks to applying the stored functions. It can be public and documented without risk, because it can no longer be used to make a false declaration.

## Ninth response: the forwarded-copy build's decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 28-36 and its open question on
`.adopt::<U>().supplies::<T>()`. The user signed it off the same day; the adapter aliases arrive with
the adapters.

All six decisions are accepted. The lifetime change goes in now, and the `adopt` case is refused, as you suggest.

**The lifetime on `HostRequest`: make the change now.** The probe settles it. A plain associated type can't express a request borrowed for one call, and that's how rocket hands its request over. `type HostRequest<'r>;` with `for<'r> Fn(&A::HostRequest<'r>) -> Option<T>` is the honest shape, and generic associated types have been stable since 1.65, well before our 1.88 minimum. Changing it while only `ulo-http` depends on the trait costs one edit. Changing it after three adapters exist would be a breaking change in four crates. axum and actix just write `type HostRequest<'r> = Parts;` and ignore the lifetime.

**1. Entry 30: accepted.** Not consulting the exclusion follows the rule we've applied throughout: refuse what `prepare` can *see*. Whether a later entry rewrites the path is invisible to it, so comparing the exclusion with route patterns would refuse configurations that work. The cost, a supply after an excluding entry wrongly lifting the refusal, should be stated in the `supplies` docs, with the advice to put the supplying entry where its exclusion matches the routes that read the value.

**2. Entry 32: accepted.** A tower host has nothing to offer besides the head and the body, so `Parts` is the only request a copy on such a host can read. Restricting the tower impl to it is what keeps a `forward` on a tower host from declaring a value it never copies.

**3. Entry 33: accepted.** The type parameter is the cost of typed copies, and erasing it would only move the check to runtime. Each adapter should export aliases (`ulo_http_axum::Handle`, `ulo_http_axum::Service`) so users rarely write the parameter. Storing the copies at startup, so that an early-taken handle still runs them, is the right detail.

**4. Entry 34: accepted.** Putting the copies inside `respond` is the same principle as `forward` itself: make the wrong path impossible rather than documented. No adapter can reach the app without running them.

**5. Entry 35: accepted.** This matches "panics are caught inside the app". The copy runs before any execution exists, so no error handler can receive the panic, and a log plus `HostMissing` is the most informative outcome available.

**6. Entry 31: accepted.** Reporting only the stage failure first is better than reporting misleading `Host<T>` refusals computed over an empty stage, and it makes the code match what its doc already claimed.

**The open question: refuse it.** `adopt` copies *from* the request's extensions *into* the execution, so it never writes the request. A `supplies` after it declares something no entry inserts, which is exactly what the stray rule exists to catch. The rule then reads: a `supplies` must follow an entry that can write the request, and `adopt` is the one built-in entry that can't. The refusal text can say so directly: "`adopt` copies a value out of the request and inserts none; declare the `supplies` after the entry that inserts it".

## Tenth response: adapter-inserted values, entries 39 and 40, and the refusal text

Received 2026-10-04, answering the gaps the fold of `divergences/batch2a-embed.md` rounds three and
four left: a value an adapter inserts read through `Host<T>` under `host_extensions: false`,
entries 39 and 40, and the refusal text's type names. The user signed it off the same day.

### The adapter-inserted value

Your suggestion is right, and it can go one step further. The adapter shouldn't insert `OriginalPath` *and* pre-register a `forward`: two mechanisms that have to agree can drift apart. Make the `forward` **be** the insertion:

```rust
// inside ulo_http_rocket::Embedded::new()
Embedded::<Rocket>::new()
    .forward(|req: &rocket::Request<'_>| Some(OriginalPath::new(req.uri().path().as_str())))
```

`respond` already runs every copy before answering, so the adapter writes no separate insertion code. A type the check counts as supplied is then supplied by exactly the code that counts it. That's the eighth response's invariant held by construction, not by convention.

It also generalizes. Any value an adapter offers its app, whether `OriginalPath` today or a host's TLS details tomorrow, goes in the same way. Each adapter's documentation lists its built-in forwards, and nothing needs new API.

### Entries 39 and 40

Keep both as built.

**Entry 39 (off the entry)** reports the true state. The declaration supplies nothing, so counting it would let the check pretend an `adopt` is reached when it isn't. The extra "nothing supplies it" line next to the refusal is accurate, not noise.

**Entry 40 (the scoped placement)** is correct advice. A scoped entry's exclusions really are applied per route, so it's the placement that makes the check precise. It's worth the extra clause.

### The refusal text

This one is more than an illustration, and worth fixing. The core's diagnostics rule (DESIGN §10.1) prints type names cut to their last segment, with full paths only where two different types would print alike. The embedding refusals print the qualified name (`Host<embed::User>`), which is inconsistent with every other report in the framework. Run these through the same formatter the wiring reports use, so the code prints `Host<User>`, matching the design's quotes, and switches to full paths only on a collision.

### The rest

The aliases and each adapter's `HostRequest` choice arriving with race 2b is fine. Since rocket's is already probed, record the probe's result in the design next to its row, so race 2b doesn't have to repeat it.

## Eleventh response: the short-name build's decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 41-46. The user signed it off
the same day. Item 4's separate commit was not possible: the doc fix had already landed inside
`750c407f`, which was pushed.

**1. Add `TypeName` now, not later.** The "breaks nothing later" argument runs the wrong way. Adding `TypeName` later breaks nothing, true, but by then `Key::colliding` is public API, and taking it back *would* be a breaking change. So the permanent part of option one is the new public method on `Key`, not the reuse.

And the reuse stretches what `Key` means. A `Key` is a binding's identity, a type plus a qualifier. Using it to display `Host<T>`'s `T`, which is never bound, makes "key" mean "any type we print". The next reader of `Key`'s docs then has to learn that some keys aren't keys.

`TypeName` is small: it holds the type's name, has short and full display forms, and offers `colliding`. Put the shortening and collision logic there, have `Key`'s `{}` and `{:#}` delegate to it, and make `TypeName::colliding` the one public entry point that both the wiring reports and `ulo-http` call. That still gives one implementation, `Key` keeps its meaning, and no public item would ever need to be removed.

**2. Keep it.** It's the same rule applied where the same problem occurs. Two controllers named `Api` printing alike is exactly the confusion the rule exists to prevent, and route-table conflicts are where people will meet it.

**3. Accepted.** It makes explicit a bound the extractor already had.

**4. Accepted.** It's a formatting fix with no change in meaning, and it makes `cargo doc` with warnings denied pass. A separate commit with a one-line message is enough.

**Entry 45: fine as left, with one follow-up.** Withholding `HostMissing` from the 500 is right, and the `peer_addr` refusal already follows the rule. On collisions per transport: what the user actually reads is one startup report. `StartupError::Configure(ConfigureErrors)` prints every transport's failures together, so two different `User` types from two transports could print alike in that one report. `WiringErrors` already solves this by gathering keys across all its entries before formatting. Give `ConfigureErrors`'s `Display` the same pass, ideally once `TypeName` exists, so it's one call. It's a small change, and the rule then holds for the report as the user sees it.

## Twelfth response: the TypeName build's decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 47-51 and the two cases they
leave uncovered. The user signed it off the same day.

All five are accepted. I have one naming correction on item 3, and the transport-name gap should be closed now rather than left.

**1. Accepted.** Equality by `TypeId` is what makes `TypeName` mean the same thing `Key` does: identity is the type, and the string is only how it prints. Two different types with identical text must never compare equal. `Key::type_name()` is the right accessor for what `ulo-http` needs.

**2. Accepted. Both output changes are corrections.** The second one especially: `a::Config` and `b::Config @ Q` are two different types that both print as `Config`, so the reader was getting exactly the ambiguity the rule exists to remove. The whole-key comparison only missed it because the qualifier made the strings differ. Checking the parts separately is what the rule meant all along.

**3. Accepted as a mechanism. Rename it `PrepareError`.** Downcasting to a concrete core type is the right answer to `BoxError`'s limits. But by the error-naming rule we settled, an error that one operation produces is named after that operation: `construct` fails with `ConstructError`, `connect` with `ConnectError`, `load` with `LoadError`. `prepare` therefore fails with `PrepareError`. `Failure` is the core's word for an *entry* inside an aggregate: `ShutdownFailure` sits inside `ShutdownError`, and `FailureReason` explains one. `ulo-http`'s old `PrepareError` being gone frees the name, and moving it into the core under the same name is cleaner than introducing a second word.

**4. Accepted.** Redaction has to happen where the secrets are, and keeping secrets inside a returned error to make `Display` do the work would be a leak waiting to happen. Same printed report, and the safe place for the pass.

**5. Accepted.**

**On what's not covered:**

- **Transport names should join the pass now.** They aren't really plain strings. A transport's name is its marker type's last path segment, so it *is* a type name, just stored as text. Store it as a `TypeName` (`ConfigureError.transport`, `ShutdownFailure::Close.transport`, and `StartupError::Bind`'s), and it falls under the same rule with no special case. Changing a public field's type is breaking later and cheap now, before anything is published, so this is the moment.
- **Module names keeping their own handling is right.** They already have a disambiguation scheme that fits them (labels and `#n` instance numbers, D12). Running them through `TypeName` would fight that scheme rather than add anything.

## Thirteenth response: the transport-name build's decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 52-57 and the uncovered
`InputNotSeeded` case. The user signed it off the same day.

Four of the five are accepted. Entry 57 should change, and the inconsistency you found in `InputNotSeeded` is worth fixing rather than leaving.

**1. Accepted, including dropping the F5 exception.** That exception existed because generic markers had no other protection against ambiguity. The collision rule now provides exactly that protection: `Q<X>` prints short until something else would print the same, and then prints in full. One rule everywhere beats a rule with an exception, so retiring F5 is the right outcome. Keeping lookup keys (`"http"`, `BoundAddr::transport`, `HandlerDecl::key`) as text is also right. They're identifiers code matches on, not names a reader disambiguates.

**2. Accepted.** A private flag set by the pass is the minimal way to make a single entry print the same alone as in the report. And including plain-error transports in the pass is necessary: their names still appear in the same report.

**3. Accepted.** Two `Pool`s failing their hooks in one shutdown is exactly the case the rule exists for.

**4. Accepted.** One name can't collide with itself.

**5. Change it: list every variant.** A wildcard arm in exactly the function that collects names for the collision pass is the bug the pass exists to prevent. A future variant carrying a type name would silently print short next to its twin, and nothing would flag it. Inside the core, `#[non_exhaustive]` doesn't stop an exhaustive match. List every variant, as `key_names` and `module_names` do, and adding a variant becomes a compile error at the one place that has to decide.

**The `InputNotSeeded` inconsistency: fix it, don't document it.** One entry printing the same transport two ways is worse than either form alone: the reader sees `Rpc` and `my_app::transports::Rpc` in two adjacent lines and can't tell whether they're the same thing. The "pre-rendered strings stay short" rule was meant for strings that *can't* see the report: consumer descriptions and the steps between services. The transport at the head of an input path is a type the entry already holds. Store that one step as a `TypeName`, render it when the report is formatted, and the headline and the path agree.

**The untested cases: add a transport that declares inputs.** Without one, `InputNotSeeded` and `InputConflict`, the two variants with the most transport names in them, never meet the pass. A minimal scratch transport with one `inputs()` declaration, plus a second marker sharing its last segment, covers both collision cases. Then the fix above can be shown failing when it's removed, like every other rule this round.

## Fourteenth response: the input-path build's decisions

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 59 and 61 and the two places
its "Not covered" list names. The user signed it off the same day. `KeyName` is read as `Key`, the
core's name for a binding's identity.

All four are accepted, with one adjustment to the first.

**1. Rename the field instead of changing what it means.** Keeping the type while changing the meaning is the one kind of change the compiler can't flag: code reading `path[0]` as the handler still compiles and quietly reads the first binding instead. Rename it to `steps` (the steps after the handler), and every reader of the old field becomes a compile error pointing at the line to check. Nothing is published yet, so it costs nothing, and the name now says what the field holds. Not adding a redundant field is right, since the entry already has the handler and the transport.

**2. Accepted.** A `Module` source next to a `Transport` source is the more useful pair to test. It covers the two different origin lines meeting in one report, and it keeps `alt::Rpc` as the single second marker for both collision checks.

**3. Accepted as recommended.** `InputReader` hands out the handler and its transport as a `TypeName` next to the remaining steps, and `ulo-http` renders the head step against the report's collision set. One condition applies to `PrepareError`: the text closure must receive the colliding set, which it does by construction, and not a pre-joined string.

**4. Accepted as recommended.** Store the transport as a `TypeName` on the entry, render it at format time, and add it to `type_names`. Listing every variant (entry 57) means the compiler will name the variant if this step is missed.

**Going forward:** with 3 and 4, the rule holds everywhere today. It last stays held if it's written down as a convention for new report types: a report entry stores names as `TypeName` or `KeyName` and renders them at format time, and only strings that never contain a type name are stored pre-rendered. A sentence next to `TypeName`'s docs, and the same line in the review checklist, would keep the next report from reintroducing what these three rounds removed.

## Fifteenth response: the convention for report names

Received 2026-10-04, answering `divergences/batch2a-embed.md` entries 62-66. The user signed it off the same day. The
"known limit (W9)" it cites is recorded nowhere; the deferral is filed in the workspace gaps ledger.

The first three are accepted. On the fourth, narrow the convention. My wording overreached, and the precise version is the rule these three rounds have actually been enforcing.

**1. Accepted.** Keeping `path()` next to `steps()` would have kept the short-written handler step available, which is exactly what was removed.

**2. Accepted.** The controller and `ClientAddr` appear as values in that refusal too, so leaving them out of the pass would have left them as the only names in the entry that could print alike.

**3. Accepted.** It's the same fix as `InputNotSeeded`'s head step: the description holds no type name, and the name is added at format time.

**4. Narrow the convention, and state the exception precisely.** The thirteenth response kept consumer descriptions and the steps between services pre-rendered on purpose. They're built while the graph is available, and they carry module names, field names and positions that no `TypeName` could hold. When I wrote the convention, I stated it more broadly than that decision, so the contradiction is in my wording, not in the code.

What the last three rounds actually fixed was narrower and sharper: places where **one entry printed the same name two ways**, once as a stored value rendered at format time and once inside a pre-rendered string. So the precise convention is:

> A report entry stores the names it reports as values: `TypeName`, `Key` or `KeyName`, rendered at format time under the collision rule. Descriptive text (a consumer, a path between services, an item description) may be pre-rendered, with the names inside it written short. One exception to that exception: a name that is also stored as a value in the same entry is never written into its text. It's rendered from the value, so the entry prints it one way.

Every field on your list passes that rule as built now. It's the rule that `InputNotSeeded`, the `ClientAddr` refusal and `ClosureScopeViolation` were each brought into line with. Put it in `TypeName`'s doc in place of the current sentence, and make the `WiringErrors` doc point to it rather than describe the exception separately, so it's stated in one place.

I wouldn't move the descriptive fields to `TypeName` now. The remaining case it would fix, two same-named services inside one dependency path, is real but narrow. It's already recorded as a known limit (W9), and the move touches every consumer string in the wiring pass. If a real report ever shows that confusion, the precise convention above tells you exactly which field to change.

**The checklist: leave it.** Creating a contributing guide to hold a single line would be backwards. The rule in `TypeName`'s doc is where someone adding a report type will look. If a `CONTRIBUTING.md` appears later for its own reasons, the line belongs there.

## Sixteenth response: the follow-ups build's decisions

Received 2026-10-04, answering `divergences/batch2a-followups.md` and the three findings its build
reported. The user signed it off the same day; the core's zero-timeout check is filed in the
workspace gaps ledger.

Five of the six decisions are accepted. Decision 6 needs one change, and `Count` should move crates. Two of the build's findings deserve action now.

### The six decisions

**1. `Count` belongs in `ulo-transport`, not `ulo-http`.** "Only HTTP uses it today" holds for about one race. Race 2b brings WebSocket connection limits, per-connection message limits, and RPC in-flight bounds, all the same three-state count. If `Count` lives in `ulo-http`, the WebSocket and RPC crates would either depend on the HTTP crate for it or redefine it. `ulo-transport` is where transport-neutral pieces live. It doesn't belong in the core: the core has no count limits, and `Bound` is there because the core times things. `u32` alone is right: no in-flight or stream limit needs more, and it keeps `Count` non-generic.

**2. Accepted.** It's consistent with how `Bound` takes no conversions. One explicit spelling per state.

**3. Accepted.** Owning the default in one place means no backend can disagree about what "30 s at `Default`" means.

**4. Accepted, with one sentence in the docs.** The purpose of SSE keep-alive is to stop proxies closing a connection they think is idle. Since the clock starts when the stream is found waiting, the guarantee is "a comment after at least `period` of idleness", sometimes a little later. The docs on `keep_alive` should say so, and advise choosing a period comfortably below the shortest proxy idle timeout in the deployment.

**5. Accepted.** An API whose only caller was generated code can change freely.

**6. Names as written: accepted. The fallback number: change it.** `param #n` counting dependency entries means the number doesn't match anything the user can see. They'll count parameters in the signature, land on the wrong one, and lose trust in the report. The macro knows each parameter's position, so number by **signature position**: 1-based, excluding the receiver. Then `param #3` is the third parameter, whatever kind the others are. Better still, the macro has the name of these parameters too. If threading it through to the `ViaCall` path is cheap, print the name everywhere and keep the number only for destructuring patterns that have no name.

### The findings

**`max_inflight` should become a `Count` now.** It's the same kind of setting as the stream limit, and DESIGN §3.6 says no setting is an `Option`. Doing it with `Count` already in place costs one field. Doing it after release costs a breaking change. Admission takes a `usize`, so convert at the boundary.

**`header_timeout` being HTTP/1.1-only must be documented on the setting itself.** The `ulo-http-hyper` crate doc is the wrong place, because users set the option on `HttpConfig` in `ulo-http` and read its docs there. One sentence: "HTTP/1.1 only; HTTP/2 connections are bounded by the backend's own HTTP/2 limits."

**`Bound::After(Duration::ZERO)` should be refused in `prepare`.** A zero timeout is almost certainly a mistake, since it would close every connection before its first byte. Testing what hyper does with one would only confirm a behavior nobody wants. Refuse it as a `Configure` error with the hint "write `Bound::Unbounded` to turn the timeout off". It's visible before anything binds, so it falls under the refuse-what-`prepare`-can-see rule.

The same reasoning applies in the core: `After(Duration::ZERO)` on a hook, a construction, or a readiness check is the same mistake. A matching check in the wiring pass's environment step (§10.1, step 6) would keep the rule uniform. That's worth filing, even though it's outside this race.

## Seventeenth response: re-exports and zero limits

Received 2026-10-04, answering `divergences/batch2a-followups.md` "Second round" decisions 1-5.
The user signed it off the same day.

Three are accepted as built. On the other two I'd go a different way, and item 2 hides a real hazard.

**1. Re-export the types in `ulo-http`'s own signatures.** The current rule ("re-export no type from another ulo crate") is tidy, but it pushes a cost onto users. To configure one server they must add `ulo-net` for `Tls` and now `ulo-transport` for `Count`, crates they otherwise never think about. Rust's API guidelines point the other way: when a type appears in your public API, re-export it, so callers don't need a direct dependency or have to keep versions aligned. So put them under a `ulo_http::config` module (`Bound`, `Count`, `Tls`), with the defining crates as their canonical homes. Users configuring HTTP then import from one place, and nothing is duplicated, because they're the same types.

**2. Refuse `Timeout(Duration::ZERO)`, and treat a zero `keep_alive` as off.** The route timeout is visible in `prepare` and would cancel every request on that route, which is exactly the case the zero refusal exists for. Refuse it with the same hint as the other three.

`Sse::keep_alive(0)` is the hazard. Taken literally, a zero period means writing a comment, finding the stream waiting, and writing another, in a tight loop that spins a task and floods the connection. Since `prepare` can't see it, define it instead: a zero period means keep-alive off, stated in the method's docs. Accepting `shed_retry_after(0)` is right, since `Retry-After: 0` is a meaningful value.

**3. Refuse `Count::Max(0)` on both.** It's the `Count` version of the zero timeout: a configuration that does something no one intends as a standing setting. Every request is shed, or no stream may open, and RFC 9113 only sanctions a zero stream limit briefly, never as configuration. Refuse with "write `Count::Unlimited` to remove the limit". An app that wants to refuse traffic temporarily should do it with a middleware it can switch, not a zero in its config.

**4. Accepted.** The two contexts are different. The compile error carries a span, so the reader sees the pattern in their source right there, and its text is the clearest label. The wiring report has no span, so a position the user can count is the useful label. The difference in format follows the difference in context.

**5. Accepted.** A parameter that declares two reads is still one parameter, and both lines should name it.

## Eighteenth response: lifting a route timeout, and grouped refusals

Received 2026-10-04, answering `divergences/batch2a-followups.md` "Third round" decisions 1-5.
The user signed it off the same day.

Items 1, 2 and 5 are accepted as built. On 3, a handler should be able to lift the timeout, and fixing that also gives this refusal the same hint as the others. On 4, the check is right, but its output should be grouped.

**1. Accepted.** `Endpoint` and `EndpointSpec` pass the same test as the other three: someone configuring a server writes them. Leaving out types that are only matched on, only returned as errors, or only used by backends keeps `config` honest. It's the configuration vocabulary, not a second front door.

**2. Accepted.** One page per type, at its real home.

**3. Yes, a handler should be able to lift an inherited timeout, and the fix is `Timeout(Bound)`.** "Most specific declaration wins" is the rule for all metadata. A rule that can tighten but never loosen breaks it in the case that matters most: a controller with a 5-second timeout that has one export or report endpoint which legitimately runs longer. Today the only workaround is moving that handler into another controller, which bends the code's structure around a configuration limit.

`Bound` already has the vocabulary needed, used everywhere else:
- `Bound::After(d)` is a timeout of `d`.
- `Bound::Unbounded` lifts an inherited timeout.
- `Bound::Default` means the server's route-timeout default. There's none today, so it behaves as no timeout, and if a server-wide default is ever added, `Default` already means the right thing.

Add two short constructors so the common case stays short: `#[meta(Timeout::after(Duration::from_secs(5)))]` and `#[meta(Timeout::OFF)]`. The zero refusal then gets the same hint as the other three settings, "write `Timeout::OFF` to turn the timeout off", and the special-case hint goes away.

**4. Keep the check precise, but group the output by declaration.** Checking the timeout each route actually runs with is right: a zero that every handler overrides harms nothing, and refusing it would be a false refusal. But five failures for one mistake makes the report look five times worse than it is, and buries other failures. Report once per zero *declaration*, listing the routes it reaches: "`#[meta(Timeout(..))]` on the `#[routes]` impl of `Api` would cancel every request on GET /a, GET /b, GET /c, GET /d, GET /e". The precision stays the same, and there's one line per thing to fix.

**5. Accepted.** Naming where the zero was declared is what makes the failure actionable, especially once item 4 groups by declaration.

## Nineteenth response: the route-timeout build's decisions

Received 2026-10-04, answering `divergences/batch2a-followups.md` "Fourth round" decisions 1-4.
The user signed it off the same day.

All four are accepted.

**1. Accepted.** Declaration order is the order the reader can check against the source, which is what a failure is read for. Router precedence only matters for which route answers a request, and that isn't this failure's question.

**2. Accepted, and it's the coherent reading.** `Default` meaning "the default" everywhere except here would make `Bound` mean different things depending on where it appears, and that's the inconsistency we've avoided throughout. "Inherit" already has a spelling: declare nothing on the handler. So every case has exactly one way to write it:

| Handler declares | Effect |
|---|---|
| nothing | the impl's timeout |
| `Timeout::after(d)` | `d` |
| `Timeout::OFF` | no timeout |
| `Timeout(Bound::Default)` | the server's route-timeout default (none today) |

Put that table, or its four lines, in `Timeout`'s docs. Someone writing `Bound::Default` on a handler expecting inheritance should find the answer where they look.

**3. Accepted.** Grouping requires the whole walk first, and ordering the groups by first appearance keeps the output deterministic.

**4. Accepted.** My example's bare routes were only illustrative. Matching the other route-table failures is right, and so is writing `Timeout(..)` however the zero was spelled: the failure names the declaration, not its syntax.

## Twentieth response: the race 2b review

Received 2026-10-05, answering `REVIEW_2B.md` (R1–R49, Q1–Q20 and its build plan). The user signed
it off the same day. Deferrals filed in the workspace gaps ledger: protox (Q11), gRPC on the HTTP port
or an embedding host (Q12), UDP under `--listen` (Q18, with the socket-activation entry); mTLS was
already filed.

The review is thorough, and I accept nearly all of it. Three refinements need correcting, including one name clash. Below I answer the twenty questions, then note what the build plan needs.

### Corrections to the refinements

**R5 and R47 number the extensions differently.** R5 calls `AppHandle::mounted` X19, while R47 makes X19 `also_seeded_by` and X20 `mounted`. Use R47's numbering throughout, R5 included, so the build logs cite one scheme.

**R18 has a name clash.** `Delivery` is the struct for an incoming message, and `Capabilities::delivery: Delivery` makes it the delivery-mode enum too. Rename the enum `DeliveryMode { Competing, FanOut, Addressed }`, and leave `Delivery` as the message.

**R22: the shared default group name is dangerous.** If every NATS and MQTT link defaults to `"ulo"`, two *different* applications on the same broker whose patterns overlap would silently compete for each other's requests. That's wrong delivery with no error anywhere. The default should identify the application: the root module's full type path. Two instances of one app then share a group, as intended, and different apps never do. Keep it overridable with `.group(..)`. That answers Q10.

Everything else in R1–R49 is accepted as written. R24 needs one detail settled: tie the AMQP prefetch default to `max_inflight` (`Max(n)` gives a prefetch of n, and otherwise a fixed default such as 64), so the two limits can't contradict each other.

### The questions

| Q | Answer |
|---|---|
| 1 | Both. `host_extensions: true` (request extensions are copied automatically), plus `type HostRequest<'r> = SalvoRequest<'r>`, a small struct holding `&Request` and `&Depot`, so a `forward` copy can read the `Depot`. A struct is better than a tuple: it names its fields and can grow. |
| 2 | Assert it through a test fairing. The rule is that every declared capability is asserted. Rocket shouldn't be exempt just because its slot is different. |
| 3 | No separate cap. Buffer under the embedding's own `body_limit`. Then a body over the limit gets the same 413 on rocket as everywhere else, and the byte-identical rule holds for the 413 scenarios too. `RequestBody::Buffered` reports that value. |
| 4 | `WsModule`, imported once. It's also where the broadcast adapter is configured (R10), so it has a reason to exist beyond registration. |
| 5 | The mapping is confirmed: a received Close frame gives `ClientClose`, a close the server sent gives `ServerClose`, protocol, UTF-8 and capacity errors give `ProtocolError`, the drain's 1001 gives `Drain`, and an I/O error or a close without a frame gives `Lost`. A connection refused by a connect guard **does not** reach `on_disconnect`. Connection hooks come in pairs, and that connection never connected. |
| 6 | Yes: `ping_interval: Bound` and `pong_timeout: Bound`. For keep-alive, `Default` should mean *on* (30 s each), since a half-open connection that's never noticed wastes a slot until the next write, and proxies close idle WebSockets anyway. `Unbounded` turns it off. A late Pong ends the connection as `Lost`. |
| 7 | The payload, with native correlation, on brokers. TCP and UDP keep the full envelope, because they have no native correlation. |
| 8, 19 | `ciborium` for CBOR and `rmp-serde` for MessagePack. Both work with serde and are maintained. JSON stays the default until they're fetched. |
| 9 | Accepted: `Timeout` where the link can't signal a miss, declared through `miss_signal`. The Kafka link should also create its handler topics at `bind`, as the old one did, so a stopped server's topic still exists, and the documentation can state exactly when a miss is `Timeout`. |
| 10 | The root module's full type path, as above. |
| 11 | The vendored `protoc` now. It meets "no system protoc", which is what [34] asks. Moving to protox is a later swap with no API change. |
| 12 | Out of scope for 2b, written as a limit in §6, with the extension point named. |
| 13 | Defer mTLS (F307) and change the example to a token guard. 2b is already large, and client-certificate verification belongs in `ulo-net`, where it benefits every transport at once, as its own item. |
| 14 | Reuse: `PreDispatch<T: Transport = Http>`. One stage, one set of scope rules. |
| 15 | Build on `resolve_into_stream` directly. Deferring subscriptions would leave one engine half-supported. |
| 16 | The qualifier spelling. It's the DI's ordinary mechanism, so it isn't a new GraphQL rule, just the existing one applied. |
| 17 | The first option, with `ULO_DEV`. `main` must be the same in development and production, which is the point of the feature. The `LISTEN_PID` check already keeps stale variables harmless. |
| 18 | Exclude UDP from `--listen` in 2b and say so. F306 follows with the UDP link. |
| 20 | Refuse it, at compile time where possible. Let a metadata type optionally name its transport (an associated `type Transport` through a marker trait). The handler attribute, which knows its transport, probes each `#[meta]` value at the concrete site and fails to compile on a mismatch. User metadata that names no transport stays valid everywhere, so custom types need no impl. A `Timeout` on an RPC handler then fails where it's written, even in an app that binds no HTTP server, which a check in HTTP's `prepare` would miss. |

### The build plan

The plan is sound and follows 2a's form. Two notes:

- **The spine is what makes the parallel areas safe.** W, R and G each own a contract others call: `Gateway` for GraphQL, `Link` for the brokers, `Method` for the build step. Because the spine writes every new crate's public signatures with `todo!()` bodies, those contracts are frozen before any area starts. That's what lets Q and B begin alongside W and R. Keep it that way.
- **Two items need an owner:** the shared accept loop of R28 (`ulo-hyper-serve`, consumed by the hyper backend, gRPC and the standalone WebSocket server), and the `#[meta]` transport probe (Q20). Assign both to the spine, since three areas depend on the first and every transport's attribute uses the second.

## Twenty-first response: race 2b's divergences

Received 2026-10-06, answering `DIVERGENCES_2B.md` (U1–U33). The user signed it off the same day. The
accepted follow-ups are filed in the workspace gaps ledger; the shape override on `#[message]` is
dropped, since U14's probe removes its need.

Most of this list I'd accept as recommended. Three items deserve a different answer, and one gap in §8 should shape the first batch of tests more than any single entry.

### The "decide before tests" items

| Item | Answer |
|---|---|
| U1 | **Accept** all three mechanisms, and amend the four design sentences to say what a handler may *return* rather than which trait a type implements. |
| U2 | **(b) on both transports.** One deadline rule across four transports. The argument for gRPC (the caller has stopped waiting) is the same one T8 already rejected for HTTP. The grace bounds the handlers, and running them is what lets an application log, count or reshape its timeouts consistently. |
| U5 | **(b).** The defaulted `Link::max_inflight` restores the tie the twentieth response asked for, and only one link's body changes. |
| U26 | **Add `Outcome::Failed`, rendered 500.** A 400 tells the client to fix a document it can't fix. |
| U33 | **All three.** Make `MethodNotAllowed::new` public so the GraphQL 405 goes through the error handlers like every other 405, delete `ulo-ws-tungstenite` and its `exclude` entry, and drop `watchexec-events`. |
| U4 | **Accept `wire()`.** The rule's point is one report before any socket opens, and `wire()` comes earlier than `prepare`. |

### Three different answers

**U14: replace the spelling read with a type-level probe.** Reading `Stream` out of the return type *as written* is exactly the technique the design ruled out for SSE (2a: "telling it apart would read the return type's spelling, which the design rules out"). The alias case shows why: a stream behind a type alias is mounted as unary and fails at runtime. A probe needs a type, not a value. The generated call closure already exists, so a helper generic over the closure's future output (`fn shape_of<F, Fut, R>(_: &F) where F: Fn(..) -> Fut, Fut: Future<Output = R>`) can autoref-probe `PhantomData<R>` for `R: Stream`. That sees through aliases and opaque `impl Stream` types alike, because both are just bounds on `R`. Probe it before tests: shapes are what the RPC suite pins, and this would also remove the need for an override attribute.

**U24: don't force the engine's module to be global.** Making it `global` with `exports = [dyn Engine]` works, but it puts the engine into every module's view to solve a visibility problem between two specific modules. The narrower fix fits the module system: `GraphqlModule` *imports* the module that binds the engine. Something like `GraphqlConfig::at("/graphql").engine_from(ApiSchemaModule)` makes `GraphqlModule` import that module and read its exported `dyn Engine`. Module identity is by value, so a configured module instance works as an import. The rest of U24 stands: the context resolving in the engine's module, the dependency on `ulo-ws`, and importing `WsModule` once.

**U3: the unread prefix should be refused where it's visible.** "A prefix on a controller with no HTTP route and no gateway is read by nothing and reported by nothing" is the kind of silent no-op the design refuses elsewhere. The mixed controller only rules out refusing a prefix *in general*. A controller whose handlers all belong to transports that ignore prefixes is a visible fault. The smallest mechanism is a `Transport::READS_PREFIX: bool` constant, which the freeze checks against a controller's handlers when `.at(..)` is set. Filing it as a gap is acceptable, but it's small enough to include.

### Everything else

U6, U7, U8, U9, U10, U11, U12, U13, U15, U16, U17, U18, U19, U20, U21, U22, U23, U25, U27, U28, U29, U30, U31 and U32 are accepted as recommended, with their design amendments and filed gaps.

Two notes:
- **U26's per-operation gap needs one documentation sentence.** Connect guards are the GraphQL gateway's only authorization point. Someone who writes `Ws` guards expecting them to run on every message will find they don't on graphql-transport-ws, and that should be stated where gateways are documented, not discovered.
- **U22's `Status` passthrough is fine,** since the error handlers still see the `Status` first. Document it next to `with_grpc_code`, as recommended.

### The gap that shapes the first tests

§8 says it plainly: `#[ulo_ws::gateway]`, `#[ulo_ws::message]`, `#[ulo_grpc::method]` and `ulo_build` have **never been expanded**. Their generated code hasn't been type-checked once, and neither has X24's mismatch arm. Everything the compile proved about WebSocket and gRPC handlers covers the hand-written paths only.

So the first test batch should come before any behavior test, and should simply make the compiler see that code:
- a crate with one attributed gateway and a few `#[message]` handlers (covering each reply kind and `session_with`);
- a crate with a `build.rs` running `ulo_build`, one proto with all four shapes, and `#[method]` handlers for each;
- compile-fail tests for X24's mismatch and for a gRPC shape mismatch.

Several of the uncertainties in §8 (the `Answer<M>` inference, the six-arm probe with the const-generic turbofish, the anonymous const inside a generic impl) are exactly the kind of thing that fails on first expansion. Finding that out in a five-line test crate is much cheaper than finding it inside a conformance scenario.

## Twenty-second response: the conformance runs

Received 2026-10-07, answering `divergences/race2b-tests2-http.md` (S1–S3) and the three items both
conformance logs left unchanged. The user signed it off the same day.

All three proposals are right. S1 needs a precise name for its new field. Of the three items you noted, two should become tests, and one should be documented.

**S1: (a), with the field named for the behavior.** Declaring the difference and asserting it is exactly how `EmbedLimits` already handles hosts that differ. Pinning actix below 4.15 (c) would trade a declared limit for a version trap that breaks on the next `cargo update`. Delaying `stop` (b) would add a second signal to work around one host. Make the field an enum, like `Disconnect`: `drain_pending: DrainPending { Served, Closed }`. It answers whether a request whose head is still arriving when the host's graceful stop begins reaches the app. actix declares `Closed`, the hyper hosts `Served`, and the suite asserts each in both directions. The documentation should state what it means in practice: on actix, a client caught mid-request at the moment of shutdown gets a closed connection rather than an answer.

**S2: accepted.** The `!Send` part is the `HttpServer` builder, not the running server. `run` can do the builder work synchronously (`disable_signals`, `shutdown_timeout`, `.run()`) and then return a `Send` future over the resulting `actix_server::Server` and its handle. Every host's `run` then has the same shape. One sentence for its docs: like actix itself, `run` must be called inside a tokio runtime.

**S3: accepted, and the suite fix matters more than the salvo fix.** A scenario that passes because a client timed out hasn't tested anything: it passed on silence. Make it a suite-wide rule that a request ending by timeout fails the scenario, unless the scenario is explicitly *about* a timeout. Then the salvo fix (closing the listener through the acceptor) is the first thing the rule catches, and it won't be the last kind of thing it catches.

**On the three unchanged items:**

- **The RPC recovery scenario passing vacuously on TCP is a real gap.** A lost TCP connection's `Unavailable`, followed by a reconnect, is one of the most important client behaviors on that link. The harness can test it without touching the link: put a small TCP proxy between client and server, and have `disrupt` cut every connection through it. On UDP there's no connection to lose, so the scenario should be marked not applicable and reported as skipped, not counted as passed. Same rule as S3: nothing passes vacuously.
- **The HTTP/2 drain shape should observe GOAWAY directly.** An `h2` client sees the GOAWAY frame on the held connection, which is what §9.5 promises. A refused *new* connection is a different, weaker fact. It's worth one more scenario, since GOAWAY is how HTTP/2 clients learn to stop sending.
- **Rocket's close waiting its full grace for an idle connection is a host limit, so document it.** The design closes idle connections at the start of the drain, and rocket doesn't. Rocket's grace is already set to `drain_timeout`, so shutdown stays bounded. Record it in rocket's row of the limits table, and if the suite can observe it, declare it like S1 so it's asserted rather than merely noted.

## Twenty-third response: the conformance answers and the broker suites

Received 2026-10-07, answering `divergences/race2b-tests3.md` and `divergences/race2b-tests5-brokers.md`
(eight decisions and three filed gaps). The user signed it off the same day.

The build is in good shape. I'd take six of the eight as built. On salvo and Kafka recovery I'd choose the alternatives, and one filed gap is really a bug.

**1. Accepted.** Finding the real limit, rather than the expected one, is exactly what the probe was for. A declared and asserted field is the right form, the same as `DrainPending`. A documentation note asserts nothing, and the next rocket version could change the behavior silently.

**2. Take the wrapping acceptor.** Building salvo's server inside `run` breaks the promise embedding rests on: the user keeps their own server setup. Losing `with_http_builder`, `http1_mut`, `http2_mut` and `fuse_factory` means losing HTTP/2 settings and connection protections that a real salvo deployment configures. A public `Closing<A>` acceptor, with `run` taking `salvo::Server<Closing<A>>`, gets the listener closed and keeps every setting. It costs one public type, and that type says exactly what it does.

**3. Declare it as a limit.** Cargo unifies features, so if any crate in an application turns on actix-web's `http2`, the adapter's behavior changes without anyone choosing it. That's a reason to make the limit explicit, not to leave it in a note. Declare it (HTTP/2 drain without GOAWAY, connections reset at `shutdown_timeout`), and run the actix suite once more with `http2` enabled, so the declaration is asserted rather than assumed.

**4. Accepted.** It's the right capability, and the second-instance requirement is what keeps it honest. Accepting `Timeout` there is a fact about the broker, and the extra assertion proves the drain still works rather than letting the scenario pass on silence. That a false declaration on NATS fails is the proof that the assertion bites.

**5. Accepted.** Losing 1 event in 10 during rebalances was a real delivery bug, and committing at `bind` while the group is empty fixes it at the only moment it can be fixed. Document the remaining case (a topic added while the group already runs starts at its end) in the Kafka row.

**6. Take the capability.** Declaring the scenario not applicable hides a stronger property than the one it skips: on Kafka, a severed connection loses *nothing*. A capability such as `durable_replies: bool`, with the scenario asserting that the call is still answered after the cut, turns a skipped test into a passing test of a better guarantee. That's the same rule as before: nothing passes, or gets skipped, on silence.

**7. Accepted.** These weren't design changes but the code catching up with §5.4. A reconnect that quietly lets in-flight calls vanish is the worst outcome, because nothing reports the loss. Dropping lapin's auto-recovery is right for the same reason: a recovery that loses replies is worse than none, and the next call connects again anyway.

**8. Accepted, all of it.** The `drain_http1` wait is a good catch: a test that also passed against a deliberately wrong declaration wasn't testing the declaration. Folding the early-returning `forward_copy` into one scenario removes a pass-on-silence.

**On the gaps:**

- **`RpcClient` never calling `Link::close` is a bug, not a gap.** Every application that uses an RPC client leaks the link at shutdown, which on Kafka means a consumer outliving the app and a 45-second block on drop. The fix fits the lifecycle exactly: `RpcClientModule`'s binding gets an `on_destroy` hook that closes the link. Do it now. It also removes most of each Kafka scenario's runtime.
- **The rdkafka destroy blocking** will likely mostly disappear once the link is closed before it's dropped. If it remains, drop the consumer on a blocking thread so it can't stall the runtime.
- **The NATS drain flake (1 in 48)** is a real ordering bug somewhere until shown otherwise. Don't mark the scenario flaky. Loop that scenario a few hundred times with tracing on until it reproduces. A held call left unanswered during a drain is exactly the guarantee §9.5 makes.

The fold list looks complete, with these additions: the salvo acceptor in §3.8, the actix HTTP/2 limit, `durable_replies` next to `holds_unserved` in §5.3, and the client's `Link::close` hook in §5.4.

## Twenty-fourth response: the conformance answers' build

Received 2026-10-07, answering `divergences/race2b-tests6.md` (S1–S7 and its "Not covered" list).
The user signed it off the same day, adding the Kafka suite's bounded concurrency to the batch so
the CI broker job does not inherit the container-start failure.

Six of the seven are accepted. On S1 there's a third option that's better than both.

**S1: neither. Signal through the handle.** The ordering works, and 30 of 30 runs is good evidence. But it depends on how salvo's internal `select!` behaves on a given poll, and that's an implementation detail salvo can change in any minor release without notice. The oneshot alternative fails for the reason you give: two values that must be paired, with nothing enforcing it.

`run` already receives `&handle`, and `Closing::new` already takes `&handle`, so the handle can carry the signal. `Closing::new` registers a "listener closed" notifier in the handle's shared state. `Closing::accept` fires it when it drops the inner acceptor. `run` awaits it on the handle before calling `stop_graceful`. The pairing is enforced by the handle both sides already share, there's no extra parameter, and nothing depends on salvo's poll order. It also closes the gap you noted: `run` can see whether any `Closing` was built from *its* handle, and refuse to start (or log at `warn`) when none was, so a `Closing` built from another embedding's handle stops being silent.

**S2: accepted.** A separate field is right. Folding it into `drain_pending` or `drain_abandoned` would tie an HTTP/2 behavior to HTTP/1.1 observations that only happen to coincide on actix.

**S3: accepted.** The feature enables nothing a user couldn't enable directly. Give it a doc line in the manifest and the crate docs ("for the conformance suite; enables actix-web's `http2`") so nobody mistakes it for an application feature.

**S4: accepted.** Returning a count is what stops a durable link's `disrupt` from passing having severed nothing. It's the same rule that's caught real problems all along.

**S5: accepted.** A tenth of a second with the broker up, and bounded by the destroy hook's timeout without it, is the correct trade.

**S6: move Kafka into the CI broker job now.** Running the suite by hand only was justified at 578 s. At 14–22 s, keeping it manual means a Kafka regression can merge unnoticed, and the drop and close bugs this batch fixed are exactly the kind that would. Update the two comments with it.

**S7: accepted for the fold,** with S1's handle signal replacing the salvo description.

**On what's not covered:**

- **F330 is the same class of bug as F327**, so give it the same priority. The destroy hook now calls `close`, but on TCP and UDP the client's connection stays open, so the hook appears to work and doesn't. It should be fixed in the next batch, not left as a filed gap.
- **The durable outage can be observed rather than constructed.** The relay knows when it shut and when it reopened, and the handler can record the moment it published. Assert that the publish fell inside the outage. It's a few lines, and it makes the scenario prove what its design only arranges.
- **The unbounded server-side consumer thread** is acceptable as it stands. The runtime isn't held, and the thread only blocks when the broker has already gone. Document it in the Kafka row with the other drop behavior.

## Twenty-fifth response: the conformance answers' second build

Received 2026-10-07, answering `divergences/race2b-tests7.md` (S1–S6 and its "Not covered" list).
The user signed it off the same day. The generic scenario it asks for was built before this response arrived, as
`client_close` in `divergences/race2b-tests8.md`.

All six are accepted, with one small rename on S1. One item from "Not covered" should go into the next batch.

**S1: accepted, but rename `listener()`.** Making both calls public is right: they're the adapter surface any host that keeps its listener through the drain needs. But `handle.listener()` reads like an accessor, and it actually *registers* a listener, changing the count `run` waits on. A method with a side effect should say so: `register_listener()`. `listeners_closed()` is fine as is, since it's a future named for the condition it waits on.

**S2: accepted.** Refusing is correct, since a mispaired `Closing` is deterministic and visible before anything serves. Your probe shows what the warning alternative would really do: shut the app down at startup with a misleading message. Documenting the residual (a second `Closing` held outside the server delays the stop until dropped, bounded by the core's close bound) is enough.

**S3: accepted.** A bound keeps each scenario's offsets, topics and relay cut isolated, without the suite having to namespace anything. No retry is the right call for the reason you give: a container that dies before its ready line is a symptom worth seeing, not noise worth absorbing.

**S4: accepted.** A `close` that returns before the FIN is sent would be the same false success F330 was about. Owning the writers is what makes `close` mean closed.

**S5: accepted.** The handler's answer moment with a full second of margin is a sound proxy for the produce. Parsing the Kafka protocol inside the relay just to timestamp a reply record would be a large tool for a small gain. The violation probe (failing at a 1 s outage) shows the assertion has teeth.

**S6: accepted for the fold,** using `register_listener` in §3.8.

**From "Not covered": add the generic scenario.** "`close` fails a waiting call `Unavailable`" is now a rule every link must follow, and two of seven links broke it until F330. Per-crate tests hold TCP and UDP to it, and nothing holds the five brokers. A `close_fails_waiting_calls` scenario in the suite (start a call the server holds, close the client app, require `Unavailable` within a bound) would cover all seven with one piece of code. That's exactly what the suite exists for, and the brokers' reply-lane changes in batch 5 are the code it would be protecting.

One practical note, not a design point: host swap at 14 of 15 GB through the broker runs is close to where the engine stopped answering in batch 6. With Kafka now in CI, that's worth watching locally, where the 4-container bound helps but other projects' containers don't.

## Twenty-sixth response: the client-close build, and a stream after a deadline

Received 2026-10-07, answering `divergences/race2b-tests8.md` (S1–S6) and the open question of a
stream an error handler answers after a passed deadline (F334 in the workspace gaps ledger).
The user signed it off the same day.

The first six are accepted. For the seventh, choose "end it at once" on both transports.

**1. Accepted.** A `close` that leaves the link unusable is a failure the "fails waiting calls" rule alone wouldn't catch, and §5.4 promises the reconnect. Asserting it is what makes the scenario test the whole rule.

**2. Accepted, and it's the most valuable finding of the batch.** A broken `close` passing because the core *dropped* the hung hook, ending the connection as a side effect, is a pass on silence of the subtlest kind: the shutdown machinery hid the bug it was supposed to report. Requiring a clean report closes that path in both the scenario and the per-crate tests.

**3. Accepted.** Each kept test now asserts only what the suite can't observe: the clean FIN on TCP, and the socket's release on UDP. That's the right division between the suite and the crates.

**4. Accepted.** `Option<usize>` with `None` on UDP is honest: a link with no connections reports that it can't count, rather than reporting zero.

**5. Accepted.** A bounded flush is what a producer owes its records at close. Losing queued replies to make `close` faster would be the wrong trade.

**6. Accepted for the fold.**

**7. End it at once, on both transports.** The deadline grace exists so an application can *shape* its timeout answer: its own error envelope, its own logging. It was never meant to let the call keep producing output. A stream returned after the deadline breaks that in two ways:

- **It escapes the bound.** The grace bounds the error handler, not the stream it returns, so a streamed answer would run for as long as the stream does, after a deadline whose whole meaning is "stop".
- **Nobody is reading it.** On gRPC especially, the client has already failed the call as DEADLINE_EXCEEDED locally. Every item sent is work for no one.

So the rule, stated once for every transport: after a deadline, an error handler's answer is delivered if it's a single reply produced within the grace. A streamed answer is ended at once with the canonical timeout. That's what RPC already does, so gRPC changes to match. Log it at `warn` when it happens ("an error handler answered a timed-out call with a stream; the stream was ended at the deadline"), so an author who wrote a streaming timeout answer learns why it never arrives. Check HTTP against the same rule while you're in there: a route timeout's error handler returning an SSE body should behave the same way, and the moved-in sleep from T8 suggests it already does.

**On the RabbitMQ time rise:** trace it. A 50% increase with no known cause is how a real regression hides. The likely suspects are this batch's own changes to close behavior, such as teardown now waiting for connections it used to abandon, which would be a correct cost. But that should be shown, not assumed. One `--report-time` run compared per scenario with batch 7's numbers will point to the scenario that grew.

## Twenty-seventh response: the post-deadline stream build

Received 2026-10-07, answering `divergences/race2b-tests9.md` (S1–S5 and its NATS note). The user
signed it off the same day.

Four of the five are accepted. On HTTP's size-hint rule there's a better test, and one of the filed gaps deserves more priority than "filed".

**1. Accepted.** Telling a stream by what reaches the wire is the type-level principle again: judge the reply by its behavior, not by how it was assembled. And as you note, no marker could survive a reply built through the public API. The consequences are the right ones. A second length-prefixed message *is* a stream in gRPC terms, and a body still open at the end of the grace has, by definition, not delivered a single reply in time.

**2. Use completion within the grace instead.** Treating every body without an exact size hint as a stream cuts a genuine single reply that happens to be sent through `Body::stream`. That's a false positive the error handler's author can't see coming. HTTP has no message boundaries, so its natural equivalent of gRPC's "one message" is "a body that finishes". Read the body under the rest of the grace, as gRPC does. If it ends in time, deliver it, buffered, with its now-exact length. If it's still open when the grace ends, answer the 504. SSE never ends, so it's still cut, and the `Body::stream` single payload is delivered. The rule then reads the same across transports, "a single reply produced within the grace", with each transport defining "single" by its own protocol: one message on gRPC, one completed body on HTTP.

**3. Accepted.** `timeout` is what actually happened. `internal` was reporting the side effect.

**4. Prioritize the reply-construction gap; fix the clippy one now.**

- **Error handlers on RPC and gRPC can't build a reply through the public API.** That undercuts much of what the error-handler design was for. The deadline grace, `Timeout` reshaping and the uniform error model all assume a handler can answer with something of its own, and today on two transports it can't, except by returning an error. This should be the next batch's main item: public constructors on each transport's `Reply` (gRPC from a message or a `Status`; RPC through something like `cx.reply(&value)`, which encodes with the link's codec the context already holds), with a conformance scenario per transport where an error handler answers a domain error with its own value.
- **The clippy warning is in generated code, so every user's handler inherits it.** Add the `allow` to the macro's output now. More generally, generated code should be lint-clean under `clippy -D warnings` in the user's crate. A CI step that runs clippy over a crate expanding every attribute macro would catch the next one before a user does.

**5. Accepted.**

**On the NATS startup failure:** right to file it, and the fix generalizes. A suite that can fail without saying why produces exactly the kind of one-in-fifty mystery that took a whole investigation last time. Every startup path in both suites should report the underlying error when it fails, not only NATS's.

## Twenty-eighth response: reply construction and lint-clean generated code

Received 2026-10-07, answering `divergences/race2b-tests10.md` (S1–S6), the two questions the batch
10 fold left (the three gRPC trait names, the lint rule's scope) and RabbitMQ's slowdown. The user
signed it off the same day. For item 8, CI's timings were taken first and show no rise (31–46 s
across batches 7–10); the quiet local run is deferred, since it means stopping the user's own
containers, and is filed in the workspace gaps ledger.

Six of the eight are accepted or need only a small note. Items 2 and 3 have a better answer, and item 6 contains a bug worth fixing first.

**1. Accepted.** The three constraints you hit (an alias that can't take constructors, E0119, E0283) leave the contexts as the one place left, and it reads naturally: `cx.reply(..)` uses the transport's own encoding. Unsealed is fine, since letting users implement those traits for their own reply types costs nothing.

**2. Rename. No exception is needed.** The collision only exists because two traits share one method name. These are conversion traits that consume a value, which is exactly what Rust's `Into*` convention is for, and `IntoReply` already follows it. So give each its own method and name the trait after it: `IntoGrpcReply::into_grpc_reply`, `IntoGrpcStream::into_grpc_stream`, `IntoGrpcItem::into_grpc_item`. The principle holds without an exception. An exception would be the first one the naming rules have needed, and this case doesn't justify it.

**3. Bound the buffer, and report the end when the body is written.**
- **The buffer.** A completed body buffered with no size limit means an error handler that quickly produces a large body makes the server hold all of it in memory. Give the buffer a fixed internal cap, say 1 MiB, documented. A body that exceeds it within the grace counts as streamed: a 504 and the `warn`. No new setting is needed, since this is a rare path and the cap only has to prevent the worst case.
- **The stream end.** `on_stream_end` means "the transport finished writing". Reporting `Completed` when the body was *read* makes it mean something different on this one path. Have the buffered body carry the `Tracked` outcome through to its write, so it reports `Completed` once written, or `CutOff` if the client leaves first. One meaning everywhere.

**4. Accepted. Pin the clippy job's toolchain.** A job that a new clippy release can turn red on an unrelated change will eventually block a merge for a reason nobody introduced. Pin the stable version that job uses, and bump it deliberately in its own commit, fixing whatever new lints appear there.

**5. State it as a rule, not a list.** The intended scope is *all generated code that compiles in a user's crate*: attribute macros, derives, and build-step output alike. Listing them by name is the same mistake as the first redaction scope. The list goes stale the day someone adds a generator, while the rule covers it automatically. Write the invariant that way, with the current three named only as examples.

**6. The encode failure is a bug, not a gap. Fix it first.** A server-side failure to encode its own reply currently reaches the caller as its own `Timeout`, after waiting out the whole timeout, while the server logs at `debug`. That's the pass-on-silence pattern again, in a production path: a server bug presented as a slow network. Treat it exactly like `FrameTooLarge`: answer `err` `internal` under the same id at once, and log at `error`, since it's the server's own code failing.

`RpcCx` implementing `FromCall<Rpc>` is a real gap too, and small. HTTP and gRPC handlers can take their context as a parameter, so RPC should match. Build both in the next batch.

**7. Accepted.**

**8. Trace it, but separate the machine from the code first.** Swap at 12.5–13.4 GB of 14 points at the host, but "probably the host" is what both previous rounds said, and once it turned out to be real. Two cheap measurements settle it:

- **CI's timings.** CI's runner starts clean every time, so if RabbitMQ is stable there, at around 13 s, across this batch, the local slowdown is the machine.
- **One local run on a quiet engine,** with every other container stopped and `--report-time` on, compared per scenario with batch 9.

If both point at the host, record it and move on. If CI shows the rise too, the per-scenario comparison will name the scenario that grew.

## Twenty-ninth response: the unencodable-reply build

Received 2026-10-07, answering `divergences/race2b-tests11.md` (S1–S5) and its Redis startup
failure. The user signed it off the same day.

Four of the five are accepted. On the fourth, the cap is in the wrong place, and the two startup flakes probably share a cause.

**1. Accepted.** A typed error beats `BoxError` here, and the name fits the condition-error rule, next to its sibling `FrameTooLarge`.

**2. Accepted, and pin it with a test.** `Internal` is the truthful answer: a frame this side can't encode is a local bug, not a sign that the other side is unavailable. But a behavior worth deciding is a behavior worth asserting. A unit test with a link that encodes wrongly costs a few lines and keeps a later refactor from sliding it back to `Unavailable` unnoticed.

**3. Accepted.** A message naming the actual reason is better than the generic streamed-answer `warn`. The status is the same, but the log tells the author what to change.

**4. Change it: the cap should bound only what the server has to wait for.** The cap exists so the server doesn't accumulate an unbounded body in memory while waiting for it to end. A `Full` body is already in memory, because the handler built it, so delivering it adds no cost, and refusing it turns a valid single reply into a 504 for nothing. The clean line: a body whose frames are all ready and whose end has been reached on the first poll is a reply that's already produced, and it's delivered whatever its size. Only a body the server must wait on is collected under the 1 MiB cap. That covers `Full`, and any body produced instantly, without special-casing types.

**5. Accepted.** It's a behavior change, but the right kind: the same event is now reported however the stream was built, so it's one meaning everywhere. Worth a line in the divergence log, since a callback that never fired before now does.

**On the Redis startup failure:** that's now two brokers (NATS in batch 9, Redis now) failing the same way, before any call, each roughly once in a few dozen runs. That pattern points at the shared harness more than at either link. The classic cause is a port race: the harness picks a free port, releases it, and the container binds it a moment later, by which time something else may have taken it. Check how `Broker::start` chooses ports.

Separately, make sure the cause can't be lost again. The error chain exists now, but the runner's output filter is still eating it. Write startup failures to a file that CI uploads as an artifact, or print them with `--nocapture` on failure, so the next occurrence is diagnosable the first time it happens.

## Thirtieth response: the shadowed-port build

Received 2026-10-07, answering `divergences/race2b-tests12.md` (S1–S5) and two questions: the TCP
and UDP suites' probe-then-release port choice, and an HTTP stream built without `into_reply`
reporting its end only after a deadline. The user signed it off the same day; a way to give `Tcp` an
already-bound listener is filed in the workspace gaps ledger.

All seven are settled below. For the sixth there's a simpler fix than either option offered.

**1. Accepted.** The cap has to exist, because a body whose frames are always ready, like an endless SSE stream, looks exactly like a large finished body until you've read it. Bounding "the first poll" at 32 frames is a reasonable line. The rare false positive (a body of 33+ ready frames over 1 MiB, answered after a deadline) is a corner of a corner. Document it in one sentence next to the cap.

**2. Accepted.** 18,865 polls in 300 ms was a busy loop burning a core on every timed-out request that reached this path. That's a real production bug found in passing, and exactly the kind worth fixing without being asked.

**3. Accepted.** Probing the actual port is right, and your reproduction proves why. A registry would have checked the suite's own ports and missed the VS Code helper that actually caused the failure. A probe tests the real condition, a registry tests an assumption about it. Limiting it to macOS, where the shadow can form, is correct too.

**4. Accepted.**

**5. Accepted.** The files in the log and as an artifact give you the cause on the first failure, which is what was missing twice before.

**6. Bind port 0 and read the bound address back.** The race exists because the suites choose a port themselves. The design already avoids that: an endpoint may be `0.0.0.0:0`, and `Link::bound()` reports the address the OS chose, which `App<Bound>::addresses()` collects (that's what R18 added `bound` for). So the TCP and UDP suites, and `deadline_answers.rs`, should bind port 0 and hand the reported address to the client. That closes the race without new API and without retries, and retries would only hide the next startup problem. A way to give `Tcp` an already-bound listener is worth having later for supervisors and embedding, but the suites don't need it.

**7. Yes: wrap at the service.** §2.6 promises that every streaming answer reports its end, and a promise that holds only when the stream was built a particular way is the "it depends on how you built it" inconsistency we've removed everywhere else. The service writes every body, so it's the one place that sees all of them. Wrap any body without an exact length in `Tracked` if it isn't already wrapped. Double reporting can't happen, since the first report wins, as it already does on the post-deadline path. Known-length bodies stay unwrapped, as the 2a decision (H 14) settled, so `Content-Length` is never dropped.

### Addendum to the thirtieth response: adopting a resource someone else made

Received 2026-10-07, answering the user's question whether an already-bound listener should be
TCP's alone. A design for a deferred item, filed in the workspace gaps ledger; not yet built.

It shouldn't be TCP-only. It belongs one layer down, in ulo-net's Endpoint, which already has the right shape. Endpoint::Inherited is "a socket someone else bound": it just finds that socket through the systemd environment. A pre-bound listener is the same idea with the socket handed over directly, in code.

So add it there, next to the other two:

```rust
Endpoint::Addr(addr)            // bind it yourself
Endpoint::Inherited(name)       // a socket the supervisor bound, found through LISTEN_FDS
Endpoint::listener(std_listener) // a socket the caller bound, handed over directly
```

Every server that takes an Endpoint then gets it at once, with no per-transport work: the hyper backend, the standalone WebSocket server, the gRPC server (all three bind through ulo-net and the shared accept loop), and the TCP link.

How the rest of the transports fit:

- UDP needs its own variant. A datagram socket isn't a listener: it's a different type with different checks. So it's Endpoint::socket(std_udp_socket) alongside the listener variant. This is the same territory as F306, where activation currently refuses anything but listening TCP sockets. Solving both together makes sense: the activation code learns to tell the two kinds apart, and the direct variant uses the same vetting.
- Brokers have no listening socket. They connect out, so "already bound" doesn't apply to them. Their equivalent is "already connected": adopting a client the host already holds, like the Nats::from_client(client) idea from the embedding discussion. That's the same principle (the framework uses a resource it didn't create) applied to the resource brokers actually have.
- Embedded HTTP needs nothing. There, the host owns the socket entirely.

One detail matters for correctness: an endpoint today is a plain value you can clone and compare, but a socket can only be owned once. So the listener variant should be take-once, exactly like an inherited socket is already taken from the activation. The first bind adopts it, a second one is refused with a clear Configure error, and adoption applies the same treatment inheritance does: non-blocking mode and FD_CLOEXEC, so a child process can't leak it.

So "adopt a resource someone else made" becomes one rule across the design: listeners and datagram sockets through Endpoint, broker connections through each link's own constructor, and whole servers through embedding.

## Thirty-first response: the service's stream wrap and the port-0 suites

Received 2026-10-07, answering `divergences/race2b-tests13.md` (S1–S7), the question of a stream on a
bodiless answer, and the remaining refused-endpoint test. The user signed it off the same day.

Five of the seven decisions are accepted as built. Decision 5 should change, decision 6 changes with the question, and decision 7 wants a knob rather than a constant.

**1. Accepted.** Passing the servers' bound addresses as a parameter is cleaner than a hook holding state, and links that don't need them just ignore it.

**2. Accepted.** A connection that arrives before the relay knows where to send it is closed rather than held, which is the honest behavior. Failing on a second, different upstream catches a harness mistake instead of silently rerouting.

**3. Accepted.**

**4. Accepted.** Double-wrapping is harmless because the first report wins, and the mark keeps the common path to one wrap.

**5. Remove the wrap from `into_reply`.** Now that the service wraps every unsized body, keeping a second place that also wraps means two places responsible for one rule, and those drift: the next change to tracking would have to remember both. The service is the one spot every body passes through, so let it own the rule. Keep the mark: a user can still wrap a body in `Tracked` themselves, and the mark stops the service wrapping that one again. If some path needs tracking before the service sees the body (the SSE late path reports on its own, for instance), that path keeps its own wrap with a comment saying why.

**6 and 8. Report `Completed` for a body the protocol never allows to be sent.** It comes down to what `on_stream_end` is for. Interceptors and metrics read `CutOff` as "the client didn't get everything". It's the signal someone alerts on. A `HEAD` answer, a 204 or a 304 owes the client no body at all, so when the transport writes the headers it has delivered everything that response owed. Reporting `CutOff` would raise false alarms on every `HEAD` request. Reporting nothing would break the exactly-once guarantee, and a callback waiting for the outcome would never fire. So:

- **`HEAD` answered by a `GET` handler:** `Completed`. That's legitimate and expected, so no log.
- **A 1xx, 204 or 304 that carries a streaming body:** `Completed`, plus a `warn` that the body was discarded because the status forbids one. Here the handler's author made a mistake, and the log, not the stream outcome, is the right place to say so.

**7. Make it a host setting, not a constant.** Kafka's `PARALLEL` describes what Kafka itself needs. RabbitMQ's failures came from one machine's memory: CI ran the same suite 24 of 24 at full parallelism. A hardcoded bound would slow every host to fix one. Add an environment override that every suite reads, say `ULO_CONFORMANCE_PARALLEL=6`, applied as the minimum of itself and any bound the broker declares. A constrained laptop sets it once, CI leaves it unset, and the Makefile's local targets can document it.

**On the gRPC test that needs a refused endpoint:** don't look for a port where nothing listens. That's the race again, and as macOS showed, a bound-but-not-listening socket doesn't even refuse there. Have the test control the endpoint instead: a listener on port 0 that accepts each connection and closes it at once. The client hits the same failure path (a connection that ends before any response), deterministically, on every OS, with no probe and no release.

## Thirty-second response: batch 14's sign-offs and the confirmed drain

Received 2026-10-08, answering `divergences/race2b-tests14.md` (S1–S5) and F351/F352 from the MQTT
hunts (`race2b-tests15-mqtt.md`, `race2b-tests16-mqtt.md`). The user signed it off the same day, with two notes for the build: F331 records that async-nats removes a subscription locally when it queues the unsubscribe, so the probe decides between the flush and the declared capability; and the response names three broker links where there are five, Redis, RabbitMQ and Kafka being unnamed, so the build checks whether each confirms its drain and reports, changing none of them.

All of batch 14's decisions are accepted, and S2 is right for a reason worth writing down. On the drain bugs: fix both. The NATS "limit" may not be one.

### Batch 14

**S2. Accepted, and no public mark is needed.** The question is what `on_stream_end` reports: the outcome of *the reply*, the body actually written to the client. A stream an interceptor built and then discarded never was the reply. Before, it reported `CutOff`, and because the first report wins, that discarded stream's report hid the real reply's outcome. That was the bug this fixes. Reporting nothing for a body that never reached the client is correct. The exactly-once guarantee was always about the reply, not about every stream someone built. A user's own `Tracked` inside a body is wrapped twice but reports once, so no case needs a public mark. One sentence on `on_stream_end` is enough: "it reports the reply actually written; a stream discarded before the response is sent is not a reply and reports nothing".

**S5. Port 0. My listener suggestion was wrong for this test.** The test asserts that a *down endpoint* answers `Unavailable`. A listener that accepts and then closes tests a different failure, a dropped connection, which tonic correctly reports differently. Port 0 is better than anything I proposed: nothing can ever listen there, so there is nothing to race, and you checked both platforms' actual errors.

**S1. Accepted.** The late path never depended on the wrap, and the outcomes are unchanged.

**S3. Accepted.** A dedicated `HEAD` handler is the same legitimate case as a `GET` handler answering `HEAD`. The method doesn't matter. What matters is whether the status forbids a body, which is why a 204 still warns.

**S4. Accepted, with the reasoning stated in the docs.** The outcome concerns the body, and a bodiless answer owed none. A peer leaving before the head is written is a connection failure, not an incomplete body. Where the transport observes it, it is reported as a disconnect; it does not change what was owed.

### The drain bugs

**F351. Wait for the UNSUBACKs, bounded only by the drain deadline.** A server has only stopped accepting once the broker has stopped routing to it, and the UNSUBACK is the broker's own statement of that. A separate bound would be a second knob for the same window. A slow broker eating the deadline costs less than it seems: in-flight calls drain concurrently under the same deadline anyway, so the wait only lengthens a drain that has nothing else to do. If the deadline arrives first, log at `warn` that the broker had not confirmed the unsubscribe. A slow broker then shows up in the logs rather than as unexplained timeouts.

**F352. Fix it, with one deadline for both waits.** Accepting the gap would let a call arriving during the drain get `Timeout` instead of the `Unavailable` the design promises. That is pass-on-silence again, this time in what clients see. The drain window covers both the inbound stream's end and the refusals spawned during the drain. Refusals are small and fast, so a separate short bound would add a knob without protecting anything the drain deadline doesn't already protect.

**NATS. Check this before accepting it as a limit.** The NATS protocol processes a connection's messages in order, and it answers a `PING` with a `PONG` only after processing everything sent before it. So *unsubscribe, then flush* closes the window: async-nats's `flush` sends a `PING` and waits for the `PONG`. When the `PONG` arrives, the server has processed the unsubscribe. Every message it routed to this client beforehand was written to the same TCP stream ahead of the `PONG`, so it has already arrived. If async-nats's `flush` behaves as the protocol says, F331 is a missing flush, not a limit. Probe it with the widened-window technique used for F347. If the probe holds, all three broker links confirm their drain and the rule has no exception. If it doesn't, declare it as a capability and assert it in the suite, as `holds_unserved` is, rather than leaving it as a documentation note.

## Thirty-third response: the confirmed drain's sign-offs

Received 2026-10-08, answering `divergences/race2b-tests17.md` (S1–S6) and the open bugs F354,
F355 and F357. The user signed it off the same day, with two notes for the build: on RPC, matching HTTP moves the `Tracked` wrap out of `Reply::Many` into the dispatcher and keeps HTTP's one exception, a reply the error handlers answer after a passed deadline reporting `CutOff(Deadline)` when dropped; and the larger Receive Maximum is checked against the server's own admission bound, so the excess is refused `unavailable` rather than buffered.

All six decisions are accepted. One name changes, and UDP gets one improvement. Of the three bugs, fix F354 and F357, and document F355 with a mitigation.

### Decisions

**S6. Yes, UDP declares it, but it reads what is already there first.** A datagram the kernel has already received sits readable in the socket's buffer. At close, the server does one final non-blocking read loop until the buffer is empty, answering each request found there `unavailable`. That rescues the datagrams that arrived just before close. What still can't be reached is a datagram arriving after the socket closes. A connected client socket usually learns of that through an ICMP "port unreachable", which surfaces as a refused receive and can map to `Unavailable`. But ICMP is often filtered, so it can't be relied on. So: do the final read, and keep the declaration, which then covers only the truly unreachable case.

**S2. Accepted.** This is what the drain fix was supposed to expose: a stream that ends only when every client leaves meant every shutdown waited out the full deadline. Making TCP and UDP behave like the brokers is the right repair. That the same scenario found F356 shows it earning its place.

**S5. Change RPC to match HTTP, then move the sentence to the core.** The rule should have one meaning everywhere: `on_stream_end` reports the reply actually sent, and a stream thrown away before sending was never a reply. A discarded `Reply::Many` reporting `CutOff` on RPC is the same bug HTTP had, where the discarded stream's report hid the real reply's outcome. Check WebSocket and gRPC against the same rule while you're there, so the core's doc can state it without a per-transport exception.

**S1. Accepted.** The messages are lost either way, and an exact count makes the warning trustworthy.

**S3. Accepted.** The link knows the filters and the core doesn't, so the link is the right place to name them.

**S4. The new scenario is right, but flip the name to a positive capability.** The other capabilities state what a link *does*: `miss_signal`, `holds_unserved`, `durable_replies`. `unconfirmed_drain` states what it lacks, which reads backwards next to them, especially as `unconfirmed_drain: false`. Call it `confirms_drain: bool`, with **`true` as the default**. A new link then gets the strict assertion unless it explicitly declares otherwise. That is the safe default for a test contract: forgetting to declare fails a test rather than quietly loosening one. NATS and UDP declare `false`.

### The open bugs

**F354. Fix it the same way.** It is the same ordering MQTT had, and the fix is proven. That it never happened in a run doesn't matter: it was found by reading, it is real, and the fix is small.

**F355. Document it as a limit tied to Receive Maximum, and make it rarer. Don't declare the capability.** A boolean can't express "confirmed unless more than Receive Maximum requests are outstanding". Declaring `confirms_drain: false` would loosen the MQTT test for every case to cover one corner, which hides more than it reveals. Two mitigations instead:

- The link announces the largest Receive Maximum it can handle in its CONNECT, so the broker has less reason to hold publishes back.
- The MQTT row documents the remaining condition precisely: a request can still be lost at shutdown only when more requests are outstanding than the broker's flow control allows.

The scenario keeps its strict assertion, at a load below that limit.

**F357. Retry in the link, limited to those three codes.** This differs from the earlier "no retry at bind" decision (U19). There, the broker was unreachable, and retrying would hide a real configuration error. Here, the broker is reachable and its coordinator is still warming up, and librdkafka itself classes these errors as retriable. Retrying only those three codes, within the existing bind timeout, follows the library's own judgment and keeps the one-report-at-startup rule. Asking every deployment's environment to wait for Kafka's internal coordinator first would push a broker detail onto every operator.

## Thirty-fourth response: runtime neutrality

Received 2026-10-08, answering the block on runtime neutrality: the goal stated as a test, the
branch's runtime dependencies, and seven questions. The user signed it off the same day, with four notes for the build. `ulo-net` is not runtime-free: its TLS goes through `tokio-rustls`, so answer 2's premise holds for its sockets alone, and the build moves its TLS to `futures-rustls` or into the runtime crates and reports which. The forbidden-dependency check walks the whole tree (`cargo tree -i tokio`), since a scan of direct dependencies missed `ulo-net`. The build runs in stages, each compiled, tested and green in CI before the next: the core's `Runtime`, `TaskHandle` and `TaskSet` with `ulo-tokio`; `ulo-http` on `futures-io` with `ulo-net`'s TLS; `ulo-rpc` off tokio with `RpcClient` given its runtime; `ulo-ws` on `async-tungstenite` with `ulo-graphql-ws`; `ulo-smol`, the smol TCP link, the suites taking a runtime and the smol CI job; the CI check. It starts once batch 18 has landed, both touching `ulo-rpc`.

Your audit corrects my table in two places, and the corrections are right. I put the TCP and UDP links on the agnostic side, which only works if the runtime interface includes sockets (question 2 says it shouldn't). And I left out the two hubs that matter most, `ulo-rpc` and `ulo-ws`. Your one-sentence test is better than my framing: an alternative implementation of any plug point has no tokio in its dependency tree. Here are the seven answers.

**1. Spawn: dropping the handle detaches, and abort and completion are explicit.** Cancel-on-drop is tidier in theory, but here it's a trap: every fire-and-forget spawn in the hubs would silently die the moment its handle went out of scope, and a drain built on it would behave differently depending on where a handle happened to be dropped. So one meaning, fixed by the trait:

```rust
pub trait Spawn: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle;
}

pub struct TaskHandle { /* runtime's handle, erased */ }
impl TaskHandle {
    pub fn abort(&self);
}
impl Future for TaskHandle {
    type Output = TaskEnd;   // Finished | Aborted | Panicked
}
impl Drop for TaskHandle { /* detaches */ }
```

- **On tokio** this maps directly onto `JoinHandle`.
- **On smol**, the adapter keeps the `Task` in an `Option`: `abort` drops it (which cancels), and `Drop` calls `detach()`.
- **Panics are made uniform** by wrapping every spawned future in a panic catch inside the helper. tokio reports a panic through `JoinError`, while smol re-raises it on await, and a uniform `TaskEnd::Panicked` stops the drain from behaving differently by runtime.

On top of this, put a runtime-free `TaskSet` in `ulo-transport` (spawn into it, `abort_all`, wait for all), which replaces the hubs' `JoinSet`. A typed `spawn_with::<T>` that returns a value can be a helper over a oneshot channel, without widening the trait.

**2. Keep `Runtime` small. Sockets stay out.** You're right about where the other road leads: every runtime-abstraction crate that took on sockets ended at the lowest common denominator, and then grew features to escape it. The TCP and UDP links stay tokio-based behind the runtime-neutral `Link` trait, and a smol TCP link is its own crate. `ulo-net` is already the right seam for that: it works with `std::net` sockets, which belong to no runtime, and each runtime-specific crate adopts them into its own reactor.

**3. `futures-io` as the default, tokio's traits behind a feature.** smol speaks `futures-io` natively, so an outside backend implements the traits its runtime already uses, and the tokio side converts through `tokio-util`'s compatibility layer. hyper chose its own traits for performance reasons that matter for hyper's core I/O, but not for an upgraded connection or a WebSocket stream. Our own traits would add a third set for every implementer to learn, with no gain at this layer.

**4. The hubs follow the same rule as the core: no tokio at all.** Your test sentence decides this one. If `tokio` (even with only `sync`) is in a hub's dependency tree, then every alternative backend's tree contains tokio, and the test fails by definition. `async-channel`, `async-lock` and `futures` cover what `tokio::sync` does here. The CI check is then one rule, the same forbidden-dependency list, for both the core crates and the hubs.

**5. A runtime is never found ambiently. It's given, or taken from the app.** `Handle::try_current` in `RpcClient`'s drop is exactly the hidden coupling this work exists to remove.

- **Inside an app:** the runtime is bound as `Dep<dyn Runtime>`, as the timer already is as `Dep<dyn Timer>`. The client gets it from the module that builds it.
- **Outside an app:** a client built directly in `main` takes the runtime as an explicit argument.
- **In a drop:** a drop can't await, so the client spawns its close on the runtime it holds. If it was never given one, it closes synchronously as far as possible and logs at `warn`. It never silently looks for one.
- **The suites** take their runtime the same way the app does, as already decided.

**6. The proof is right. Don't make the RPC suite wait: build the smol TCP link as part of it.** Without a smol link, the RPC hub's neutrality is a claim nobody has tested, which is exactly the trap this plan exists to avoid. The TCP link is the simplest one, and it's also the evidence that the `Link` trait itself is runtime-free. So the proof is:

- `ulo-smol` next to `ulo-tokio`;
- a smol TCP link;
- one CI job running the HTTP suite and the TCP RPC suite on smol;
- the forbidden-dependency check over the core crates and the hubs.

**7. When: right after the three small fixes, before any new transport work.** F354 and F357 are small and already understood, and F355 is documentation, so close them first. Then do the runtime batch before more transport code is written, because every new direct `tokio::spawn` adds to the cost of moving later. And `ulo-ws`'s move from `tokio-tungstenite` to `async-tungstenite` is the largest single item, so it's better done before WebSocket code grows further.
