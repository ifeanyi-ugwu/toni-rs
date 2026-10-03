# Refinements and questions on the transport design

For the user and the design's author. Section numbers refer to `transports/DESIGN.md`; "core §n"
to `DESIGN.md` one level up; bracketed numbers to `transports/CAPABILITIES.md`; `Dn` to
`DIVERGENCES.md`; `Fn` to `FRAMEWORK_GAPS.md`. Every claim is marked **Probed** (a compiler run,
appendix), **Read** (follows from the text of the design or from the built core on
`experiment/di-redesign`, with the file named), **Spec** (a specification, cited) or **Asserted**
(reasoning, not run). Probes ran on rustc 1.98.1 and 1.88.0, edition 2024, and behaved the same
on both unless a line says otherwise.

The design is the direction. Nothing below proposes another; each refinement is a change inside
the design that makes it compile, hold together with the core as built, or follow a specification
it cites. Items are ordered by severity: what does not compile or contradicts the core first.

## Refinements

**R1. §2.3: the `AnswerProbe` arms are placed on the wrong reference depths, and a `Classified`
error loses its kind.** (Probed, P24, P24b.) With `(&&&AnswerProbe(out)).answer()`, method lookup
tries the impl on `&&AnswerProbe` first, then `&AnswerProbe`, then `AnswerProbe`: at each autoderef
step the by-value candidate is the impl one reference below the step's type, which is what P02c
shows with one reference. Every `Classified` error is `Error + Send + Sync + 'static` and so
`Into<BoxError>`, so the arms are not disjoint, and the arm reached first wins. Placed as §2.3
lists them, `Classified` on `AnswerProbe`, `Into<BoxError>` on `&AnswerProbe`, `Answer` on
`&&AnswerProbe`, a `Result<_, ApiError>` takes the boxing arm (P24b prints `boxed`), the error
reaches the handlers as a bare `BoxError`, and `classify` renders it `Internal`. Change: put the
`Classified` arm on `&&AnswerProbe<Result<V, E>>`, the `Into<BoxError>` arm on
`&AnswerProbe<Result<V, E>>` and the `Answer` arm on `AnswerProbe<V>` (P24 prints `classified`,
`boxed`, `value` in that order, the first through a `type ApiResult<T>` alias). The ranked methods
take `&self`, so the value sits in a `Cell<Option<_>>`, as the core's `__private::factory::Probe`
already does. Cost: none; the text's order stays as the priority order.

**R2. §2.1 X1: the assertion as written has nowhere to live, and the shape that compiles is not
evaluated.** (Probed, P21, P21b, P21c, P21e, and the proc-macro pair in the appendix.)
`const _: () = assert!(.. Self::__FW_KEY_get ..)` cannot sit inside the impl: `const _` is a free
item, and inside an impl it is "`const` items in this context need a name" (P21c). Outside the
impl `Self` does not resolve. Given a name and placed inside the impl, where `Self::` works, the
associated const is evaluated only where it is read, in a non-generic inherent impl too: P21b
misspells the key, reads the const nowhere and compiles on 1.88 and 1.98.1. Change: `#[routes]`
emits a free `const _` after the impl, naming the controller type in place of `Self`, spanned on
the key token; the proc-macro pair shows the outer macro's const resolving the inner attribute's
items and E0080 landing on `"htpp"` with the message naming the key and the handlers (P21e for the
hand-expanded shape). For a generic controller a free const cannot name `T`; emit a named
associated const and read it from `Controller::mount` (`let () = Self::__FW_KEYS_CHECK_htpp;`),
which evaluates it when `mount` is instantiated (P21d compiles unread, P21d_fails fails read).
`key_in` compares bytes in a loop: a trait method is not callable in a `const fn` on stable, so `str == str` is out (Read); P21 shows the loop. Cost: two emission
shapes in `fw-handler-codegen`. With this, X1 closes F295 as claimed.

**R3. §2.1 X2: `__FwShared` needs a type contract, since `#[routes]` cannot name a value's type.**
(Probed, P22, P22b.) The type of `Timing::new()` is known to nobody but the compiler. Change:
`__FwShared<V0, V1, ..>` carries one type parameter per impl-level `value` entry, in the order
written across the three enhancer attributes, and `mount` infers them from the expressions; each
per-handler `fn __fw_mount_<name><V0: Interceptor<Http>, ..>(m, shared: &__FwShared<V0, ..>)`
bounds the parameters it uses by the role for its own transport and leaves the others unbounded
(a `http(value = ..)` entry is a parameter an RPC handler's fn never names). P22 shows one `Arc`
unsized into `AnyGuard<Http>` and `AnyGuard<Rpc>` with a strong count of three; P22b shows a value
lacking `Guard<Rpc>` failing E0277 at the `__fw_mount_get_rpc` call, which `#[routes]` spans at the
handler's name. The transport attribute learns the numbering from the controller-tier tokens it
already receives, so the position of each `value` entry becomes part of the `__handler` contract,
as the turbofish order of `Contribute::try_singleton` already is (DIVERGENCES wave 3). With this,
X2 lifts F294's refusal as claimed.

**R4. §3.1, §4.1, §5.1, §6.1: the streaming examples do not compile on edition 2024 without
`use<>`.** (Probed, P26, P26b.) `fn events(&self) -> Sse<impl Stream<..>>`, `fn watch(&self) ->
impl Stream<..>` and `fn history(&self, ..) -> impl Stream<..>` return an opaque type that captures
`&self`'s lifetime whatever the hidden type borrows, so the reply cannot be held as `'static`
once the handler returns (P26: "lifetime may not live long enough" at the box). `+ use<>` on the
opaque type fixes it (P26b). Change: the transport attribute appends `+ use<>` to every opaque
type in the return position of a handler with no type or const parameters and no `use<..>` or
lifetime already written, or the design states that users write it; `fw-handler-codegen` is the
place for the rewrite. The same applies inside an `async fn`'s return type (Read, RFC 3617: the
capture rule is per opaque type). Cost: one rewrite.

**R5. §2.5: `#[meta]` is outside the `#[routes]` protocol as built, and the WebSocket hook
attributes fall inside it.** (Read, `crates/ulo-macros/src/routes/mod.rs`, `shared/attrs.rs`.)
`#[routes]` removes only `guards`, `interceptors` and `error_handlers` from the impl and leaves
every other impl attribute in place, so `#[meta(..)]` on the impl reaches expansion as an
attribute macro nobody defines, or as `fw`'s own `meta` macro, which then has to error like
`#[guards]` does outside `#[routes]`. On a method, any attribute outside the inert list makes it a
handler, so `#[meta(..)]` alone makes a helper a handler, and `#[meta]` on a real handler must be
consumed by the transport attribute or it is unresolved. Change: add `meta` to the inert list,
parse it in `#[routes]` at both tiers, carry it in the protocol as
`__handler(name, controller(..), method(..), meta(controller(..), method(..)))`, and export a
`meta` attribute macro whose body is the "goes below `#[routes]`" error. X3's `HandlerDecl.meta`
then has a source. `#[fw_ws::on_connect]`, `on_disconnect` and `after_init` methods are classified
as handlers by the same rule and receive a `__handler` attribute, which each must consume (Q1
asks under which transport).

**R6. §7: `providers = [with = |q: Dep<QueryRoot>, ..| ..]` is not a providers-list form.** (Read,
`crates/ulo-macros/src/module_attr/providers.rs`.) A providers entry is a type, `Type as dyn T`,
`expr?`, `expr`, a bare closure (lowered to `singleton`/`try_singleton`) or `into K: [..]`;
`with = ..` and `with(scope) = ..` exist in `into` lists and the enhancer attributes alone. This is
F300's open corner. Change: write the bare closure, or state that the transport race extends the
providers grammar with `with = ..` and `with(scope) = ..`, lowering as the `into` form does, which
closes that corner. Either way the example changes.

**R7. X3: the handler's own parameters never reach the wiring pass.** (Read,
`crates/ulo/src/graph/scopes.rs` `InputWalk::run`, `transport/controller.rs` `Mount::handler`.)
§2.2 says `Param::dependencies` lets "wiring check handler parameters like any constructor's",
and X4 relies on "the per-handler input check from core §6.4" working unchanged. The built walk
starts from the controller binding, the three role collections and the enhancer declarations;
`HandlerDecl` carries no parameter reads, and X3's builder lists `.controller`, `.method`,
`.meta`, `.route`, `.shape` and no dependencies. A handler reading `Dep<RequestHead>` on an RPC
controller, or `Dep<Foo>` its module cannot see, passes `wire()` and fails at the first call.
Change: `.dependencies(Dependencies)` on the builder, filled by the generated code from each
parameter's `Param::dependencies`, and the walk adds those reads to its roots as it adds a
closure's. The core already has a `pub(crate) struct HandlerDecl`; the public builder needs
another name or the record a rename.

**R8. §2.2: `ExtractFailure::Malformed(Redacted)` cannot be built outside the core.** (Read,
`crates/ulo/src/redact.rs`: `Redacted::from_parts` is `pub(crate)`; core §10.2: code outside the
core "builds none of them".) Change: `Malformed(String)`, the transport redacting with its own
rule, or a core extension `AppHandle::redact(BoxError) -> Redacted` that runs the graph's
registered secrets over it, listed in §11 as X14. The second keeps one redaction rule; the first
costs nothing in the core.

**R9. §2.2: `Option` is a `Param` over `Param`, not a concrete impl beside the core's types.**
(Probed, P23, P23b.) "One concrete impl each" for `Dep`, `Many`, `Ext`, `Option<_>`, `ModuleRef`
and `ExecutionRef` reads as `impl<T, S: FromContainer> Param<T> for Option<S>`; `Option<Json<U>>`
then needs `impl<T, P: Param<T>> Param<T> for Option<P>`, and the two overlap on `Option<_>`
(P23b, E0119). Change: one impl, over `P: Param<T>`, forwarding `CONSUMES_BODY`; `Option<Dep<U>>`
is covered because `Dep<U>` is a `Param`, and `Option<UserType>` arrives through rank two as
`Injected<Option<UserType>>`, since `Option<S: FromContainer>` is itself `FromContainer` in the
core. P23 shows the two ranks with the transport as a parameter of the probe type rather than of
the method, since lookup checks an impl's where-clauses and never a method's. `Valid<P>` takes the
same shape.

**R10. §2.2: no pair can be skipped by spelling, so emit every pair.** (Probed, P25, P25b.) "Pairs
whose types are known not to consume the body are skipped" has no source: `CONSUMES_BODY` is a
property of the type, and a macro sees tokens; `type Body<T> = Json<T>` consumes and `Path` does
not whatever either is called. Skipping by a list of names reopens the hole [2] closes. n
parameters emit n(n-1)/2 one-comparison constants (three for three, P25), and the second
assertion of P25b fails with "`user` and `login` both consume the body", the second parameter an
alias. Cost: the sentence; the compile cost is nil.

**R11. X6: `StartupError::Configure { transport, source }` cannot carry "every failure
collected".** (Read, §2.7; `crates/ulo/src/error/mod.rs`, `app/mod.rs` `listen`.) One variant
names one transport and one source. Change: `Configure(Vec<ConfigureFailure>)` with
`{ transport, source: Redacted }` per entry, each transport's `prepare` itself answering one error
that lists its own failures the way `WiringErrors` does. `listen()` as built binds in queue order
and closes the servers already bound on a failure; the `prepare` loop slots in before it, and
`ErasedServer` gains `prepare` and `bound`.

**R12. X9: `Mounted::module_meta::<T>()` must hand over the declaring module, or global middleware
resolves against the wrong visibility.** (Read, `crates/ulo/src/module/meta.rs`,
`transport/pipeline.rs` `obtain`, `execution/mod.rs`.) A `Meta`'s dependencies are checked at
`wire()` against the module that wrote it, and the built pipeline resolves a by-type enhancer with
the controller module's visibility (`graph.lookup(at.module, key)`). X8 opens the execution with
root visibility, and a middleware declared in module `M` and resolved through the execution's
resolver before `route_to` misses a binding only `M` sees, with `LookupError::NotFound` at the
first request where `wire()` passed. Change: `module_meta` yields `(ModuleRef, Arc<T>)` in
collection order, and `fw-http` resolves each middleware through that module inside the request's
execution. The core has no public way to do that today: `Resolver::in_module` is `pub(crate)`, and a
`ModuleRef` carries an execution only when `Resolver::module()` made it. X9 therefore also adds
`ModuleRef::in_execution(&ExecutionRef) -> ModuleRef`, or `ExecutionRef::resolver_in(&ModuleRef)`,
so a per-execution middleware is still built once per request.

**R13. X4: say how the transport-declared inputs meet the module-declared ones.** (Read,
`crates/ulo/src/binding/alias.rs` `Input::seeded_by`, `graph/wire.rs` `check_bindings`.) The core
declares inputs through `m.input::<T>().seeded_by::<Tr>()` on a module node, refuses one in a keyed
module and refuses one declared twice. X4 adds a second path at `Mount::handler`. Change: state
that a transport ships no input module once X4 exists; that a declaration arriving from both
paths with the same `(key, seeder)` is one declaration rather than a duplicate; and what
`InputDecl::declared_in`, which reports print, holds for a transport-declared input (the
controller's module, or the root). Note the consequence that an app binding a transport whose
handlers mount nowhere declares none of its inputs, so a non-optional `Dep<RequestHead>` reads as
a missing dependency rather than an unseeded input (Q4 for the GraphQL case).

**R14. X3: `.route(&'static str)` cannot be written by a configured module.** (Read, §7, core
§8.3; `crates/ulo/src/module/def.rs` `controller::<C>()`.) `GraphqlModule::for_root(GraphqlConfig
::at("/graphql"))` mounts at a runtime value, and `Controller::mount` is `fn(&mut Mount)` with no
access to the module's configuration: `ModuleDef::controller::<C>()` records `C::mount` by type.
Change: `route(impl Into<Cow<'static, str>>)`, and a mounting form that reaches the configuration,
`ModuleDef::controller_with::<C>(move |m: &mut Mount| ..)` or a module `Meta` value `Mount` reads
for the path. Without one, every integration at a configured path, GraphQL and a health endpoint
alike, leaks the string or hard-codes it.

**R15. §5.2: "a pattern nothing handles answers `err` with kind `unimplemented` on every link"
contradicts the link table two paragraphs down, and cannot be produced server-side on four
links.** (Read; Spec.) On NATS the table maps no-responders to `Unavailable`, and a server that
subscribed to no such subject never sees the request (Spec: NATS no-responders is a client-side
`503` status header, since NATS 2.2). On Redis, RabbitMQ and Kafka the subscription, queue or topic
is the routing table, so a request for an unhandled pattern is never delivered to this server:
`PUBLISH` answers a receiver count of zero (Spec: Redis `PUBLISH` reply), a `mandatory` publish to
no queue returns `basic.return` (Spec: AMQP 0-9-1, `basic.publish` with the `mandatory` flag), and Kafka
answers `UNKNOWN_TOPIC_OR_PARTITION` or auto-creates the topic. [32]'s uniformity is therefore
produced in `RpcClient`, by mapping each link's "no destination" signal to one kind, and on NATS
that signal cannot tell "no handler" from "server down". Change: pick the kind the client answers
for an unknown pattern (one of `unimplemented` or `unavailable`), state per link which signal
produces it, and make the conformance scenario assert that. TCP and UDP keep the server-side
`err`.

**R16. §5.2: a client call's binary payload is refused at the call, not in `prepare`.** (Read.)
"A handler or client call using a binary payload on such a link is refused in `prepare`": a
client call is a runtime value. Change: handlers in `prepare`, from a shape recorded on the
`HandlerDecl` (a `Param` constant, or `Shape`), and a client call with `RpcError` of a named kind
before any I/O.

**R17. §2.4: `classify(&BoxError)` cannot see `CancelReason::Deadline`.** (Read, §2.6.) The reason
is on the execution, not in the error. Change: `classify(&BoxError, &ExecutionRef)`, or each
transport tests `cancel_reason()` before classifying. Minor.

**R18. §3.1: `Event::data` must split on CR and CRLF as well as LF.** (Spec, WHATWG HTML, Server-sent
events, "Parsing an event stream": a line ends at CRLF, LF or CR.) A CR inside a payload ends the line at
the client; "multi-line data is split into several `data:` lines" has to split on all three. "Interpreting an event stream" in the same chapter ignores an `id` field whose value contains
U+0000, and `event`/`id` values cannot carry a line terminator at all; say what the builder does
with one.

**R19. §2.9: the HTTP span is missing two required attributes.** (Spec, OpenTelemetry semantic
conventions, HTTP spans, stable: `http.request.method`, `url.path` and `url.scheme` are Required
on a server span; `http.response.status_code` and `http.route` are Conditionally Required;
`error.type` when the request failed.) Add `url.scheme` and `http.response.status_code`. The span
name `{method} {http.route}` is as the convention writes it. For gRPC the span name is
`$package.$service/$method` (Spec, RPC spans), which §2.9 does not state.

**R20. §2.4: state the RFC 9457 `type` and `title` policy.** (Spec, RFC 9457 §4.2.1: when `type`
is absent or `about:blank`, `title` SHOULD be the status phrase, and `detail` carries the
human-readable explanation.) The table says "`type`, `title`, `status`, and `detail`" and nothing
about their values. If `type` is `about:blank`, `title` is the kind's status phrase and
`public_message` goes in `detail`; if a URI per kind or per error type, say which and where
`public_message` goes.

**R21. §2.4: a 401 needs a challenge when none is configured.** (Spec, RFC 9110 §15.5.2: a 401
MUST carry `WWW-Authenticate` with at least one challenge.) "The server's default challenge is
configurable" leaves the unconfigured case undefined. State the default (`Bearer` for an API, RFC
6750 §3) or make the kind unrenderable without one, which `prepare` cannot check.

**R22. §4.2: an id-less message that fails answers nothing, against [26].** (Read.) "A message
without an `id` is fire-and-forget: there's no ack, and errors are logged", while [26] and §4.2's
own last sentence say every failure answers the one `error` envelope. Change one: answer the
envelope without an `id`, or amend [26] with the exception and say that a guard's refusal of such
a message is invisible to the client.

**R23. X5/X10: say what the core records for a reason-less `cancel()` and where the reason
lives.** (Read, `crates/ulo/src/execution/notify.rs`, `app/shared.rs` `LiveSet::cancel_all`.)
`Notify` carries no payload; `ExecShared` gains a write-once reason slot, `cancel_all` writes
`Drain`, and `cancel_with` lives on both `Execution` and `ExecutionRef`, as `cancel` does today,
since a disconnect is noticed in a task holding a clone. `cancel()` either stays and records
nothing (`cancel_reason()` answers `None`) or is removed; the enum is `#[non_exhaustive]` either
way. `stream_outcome()` needs a second slot and a second `Notify` per execution.

**R24. X8: `route_to` takes effect for resolvers created after it.** (Read, `crates/ulo/src/
execution/mod.rs` `ExecShared::resolver`, `resolver.rs`.) `ExecShared.module` is a plain field
copied into each `Resolver` at creation; switching it needs an atomic or a mutex, and a resolver a
pre-routing middleware is holding keeps the old module. Instances that stage resolved sit in the
execution cache under their `BindingId`, so a key that names a different binding under the
controller's module is a second instance in one execution. State both; neither is a bug, and both
are visible to a middleware author.

**R25. X13: `AppHandle::phase()` includes `Closed`.** (Read, `crates/ulo/src/lifecycle/phase.rs`.)
The built phases from `Connected` on are `Running`, `Stopping`, `Draining`, `Destroying`, `Closed`;
the design lists four.

**R26. Two specification wordings.** (Spec.) §4.2: 1013 is an entry in the IANA WebSocket Close
Code Number Registry (registered 2012), which RFC 6455 §11.7 established; RFC 6455 §7.4.1 itself
defines 1000–1011 and 1015. §5.3: `basic.qos` prefetch applies per channel, or per consumer with
`global=false`, in RabbitMQ (Spec: RabbitMQ consumer prefetch; RabbitMQ reinterprets AMQP 0-9-1's
`global=true`), not per connection. Dead-lettering on `basic.reject` is RabbitMQ's
`x-dead-letter-exchange` extension, which §5.2 already phrases as conditional.

**R27. §3.7: `BackendLimits` needs an `upgrades` entry, for actix.** (Asserted.) `Request.upgrade:
Option<OnUpgrade>` presumes a per-request upgraded I/O, which hyper gives; actix-web's HTTP/1
upgrade is a service-level hook (`HttpService::upgrade`, receiving the request and the framed
connection), and rocket 0.5 exposes `IoHandler`/`IoStream` through `Response::add_upgrade`. The
table lists actix's h2c refusal only; either the actix backend wires `svc` through that hook, or
it declares `upgrades: false` and `prepare` refuses a gateway on the HTTP port with a `Configure`
error naming the limit, as the table's mechanism already provides.

## Questions

**Q1. §4.1: under which transport do `on_connect`, `on_disconnect` and `after_init` mount, and
how do `connect_guards(..)` reach a `MountedHandler<WsConnect>`?** (Read, §2.1, §4.1;
`crates/ulo-macros/src/routes/mod.rs`.) X1 names five keys. `#[routes]` classifies each of the three
methods as a handler and hands it a `__handler` attribute carrying the impl's `#[guards]` tier,
which applies "strictly" to every handler: a `#[guards(ws = RateLimit)]` on the gateway impl then
applies to `on_disconnect`, and an unscoped one must implement a role for whatever transport the
hooks mount under. `after_init` runs once with a `GatewayRef`, not per call, and §4.1 counts
"each connection hook" as an execution. The connect guards are written in the gateway attribute
on the impl, which the `on_connect` attribute on a method never sees, and `on_connect` is optional
while a gateway with none still needs a `WsConnect` handler for the guards to run through
`dispatch`. It matters because the `__handler` protocol and the role check decide what compiles.

**Q2. X8, §3.6, §10: what is answered when `Execution::open` returns `Closed` during the
drain?** (Read, `crates/ulo/src/app/shared.rs` `open`, core §9.5.) From Draining on, `open` is
refused. A request arriving on an HTTP/1.1 keep-alive connection before its `Connection: close`
response, a frame on a TCP link after `goaway` was sent but before the peer read it, and a gRPC
call on a connection whose GOAWAY is in flight all reach the transport with no execution to open.
Does HTTP answer 503 with `Retry-After` and `Connection: close`, and does the global middleware run
for it when its dependencies need an execution? The §10 table has a row for the drain window and
none for this.

**Q3. X7: where is a stream's first item pulled, and is suppressing on `Ok` intended?** (Read,
§2.3.) "An `Err` from a stream before its first item goes through `dispatch`'s error handlers like
any other error" requires that item to be pulled inside `dispatch`, before the reply is returned
and the response head committed; the built `dispatch` returns the reply and the transport drives
the stream after. If the first item is pulled inside, an SSE handler whose first event is minutes
away sends no headers until then, unless a keepalive comment counts as an item; if outside, the
sentence does not hold. Separately, an error handler written for the pre-stream case answers
`Ok(response)` with a 500 body; mid-stream that `Ok` "suppresses the error: the stream ends
cleanly". Is a clean end the intended reading of a reply that is itself a failure rendering?

**Q4. §7: is the GraphQL endpoint a mounted handler or a `Pipeline` route, and what follows for
roles and inputs?** (Read, core §7, D31/F297, X4.) If `GraphqlModule` mounts nothing through
`Mount`, an app whose only HTTP surface is GraphQL records no `AnyGuard<Http>` role key, so its
global HTTP guards are provider contributions refused at `wire()` when one needs an execution
(F297), and X4 declares no HTTP input. If it does mount a handler, R14 applies to its path.

**Q5. §5.4, §6.3: by what mechanism does a client "made inside an execution" learn the remaining
deadline?** (Read, core §3.8.) `RpcClient` and the tonic client are singletons injected as
`Dep<_>`; neither holds an `ExecutionRef`, and the core has no task-local or ambient execution,
so a tonic interceptor cannot find one. Does `request(..)` take the context, is the client wrapped
in an execution-scoped binding, or is the forwarding dropped from the design?

**Q6. §4.1: what does `Session<T>::describe` declare, and is the session readable in the connect
phase and in `on_disconnect`?** (Read, core §3.2, X4.) Wiring resolves every injection point by
key; X4's WebSocket inputs are `ConnectionInfo` and `UpgradeHead`. If `Session<T>` reads a third
input seeded per message, name it. The session is created before the connect guards and dropped
with the connection; `on_disconnect` runs as a terminal execution after the connection ended, so
is the session already gone there?

**Q7. §3.2: what does `prepare` do with a `Path<T>` whose `Deserialize` asks for no names?**
(Probed, `fieldrec` in the appendix.) A derived struct asks through `deserialize_struct` with its
serde names, renames included; a tuple through `deserialize_tuple(len)`; a scalar through
`deserialize_u64` and kin; a newtype recurses. A `HashMap`, a struct with a `#[serde(flatten)]`
field (which derives as a map) and a hand-written impl calling `deserialize_any` carry no names.
Are those skipped, refused, or matched on count where a count exists? §12's row implies every
`Path<T>` is checked.

**Q8. §2.6 X10: who awaits `stream_outcome()`, and which stream on a bidirectional call?**
(Read.) An interceptor returns the reply before the stream is consumed, so awaiting the outcome
inside `intercept` blocks the reply; the consumer has to spawn, which `fw_tokio::spawn_in` allows
and the core alone does not. A gRPC bidi call and an HTTP request with a body each have an inbound
and an outbound stream; `Tracked<S>` wraps the reply, so is the outcome the reply's alone?

**Q9. §3.7: does `Disconnected` fire when the server drops an unread request body?** (Read.)
`Request.body` is documented "dropping it early is a disconnect". A handler that never reads the
body drops it with the `Cx` at the end of the call; that is not the peer going away. The cancel
reason should fire on the peer's close alone, or §2.6's list needs the server-side case.

**Q10. [10]: is TLS offered on the separate-port WebSocket server and on the TCP link?** (Read,
§2.7, §3.5, §5.3.) §2.7 sets ALPN "per transport" for HTTP and gRPC and refuses `tls` on UDP; the
standalone `fw_ws::Server` and the TCP `Link` are not named either way.

**Q11. §3.6, §2.4: what value does `Retry-After` carry on a load-shedding 503 and on an
`Unavailable` without `RetryAfter` in its details, and where is it configured?** (Read.)

**Q12. §5.3: Kafka keyed by correlation id loses per-caller ordering; intended?** (Read.) One
caller's requests spread across partitions; the table's "ordered per partition" then orders
nothing a caller can observe across calls.

**Q13. §2.7, §9: does `fw-net` unset `LISTEN_FDS`, `LISTEN_PID` and `LISTEN_FDNAMES` after
reading them, and set `FD_CLOEXEC` on the inherited sockets?** (Spec, `sd_listen_fds(3)`:
`unset_environment` exists so a child spawned later does not read a stale `LISTEN_PID`, and the
function sets `FD_CLOEXEC` on every passed descriptor.) A `fw dev` child that spawns a subprocess
otherwise hands it the sockets.

**Q14. §2.4: what carries a `Detail::Json` that is not a JSON object on gRPC?** (Spec,
`google.protobuf.Struct` holds a map only.) A `Value::Array` or scalar needs `google.protobuf.Value`
or is refused.

### Evidence on decisions 1 and 4, for the user's call

**Decision 1 (per-module middleware app-wide by pattern).** NestJS's `MiddlewareConsumer
.forRoutes(..)` matches routes by path across the application, so the named precedent behaves as
the default does (Read, NestJS middleware docs). In the built core every `MountedHandler` carries
its module (Read, `crates/ulo/src/transport/controller.rs`), so "only the declaring module's
routes" costs the router one comparison and nothing in the core; the two models are equal in
mechanism and differ in what a shared auth module can protect. Under app-wide matching, two
modules selecting one pattern stack in collection order (core §3.2), which is defined but is
decided by import order rather than by anything written at the route. Under either model a lazily
loaded module's middleware is refused (`LoadRefusal::Middleware`).

**Decision 4 (connect guards after the 101).** RFC 6455 §4.2.2 lets a server refuse before the
upgrade with an HTTP error, "such as 403 Forbidden", and §7.4.1 defines the post-101 close codes
(Spec). A browser `WebSocket` cannot read the handshake's HTTP status: a refused upgrade fires a
generic `error` and a `close` with code 1006, while a close after 101 reaches the script as
`CloseEvent.code` and `reason` (Spec, WHATWG WebSocket API). So the default is the only path that
tells a browser client why, and refusing before 101 is what a reverse proxy's access log and a
non-browser client see as a 4xx with no WebSocket framing and no connection counted against the
gateway's limit (§4.2). Both are valid; the choice is which client learns the reason.

## Deferred items in the design

Everything the design marks "later", "at first", "optional", "opt-in" or "open to change",
listed so none is lost.

- §13 decision 4: refusing the handshake itself with 401 or 403 "could be added later as an
  option"; connect guards run after the 101.
- §13 decision 8, §9: `fw dev` socket holding is Unix-only "at first"; Windows restarts without
  holding and documents it.
- §11: X10 "could be folded into X5"; X12 needs no core change and is listed for the mapping.
- §2.2: the `validator` bridge "sits behind a feature flag".
- §2.6: `Sse::end_event(..)` is opt-in and off by default.
- §3.1: the body limit is a server setting (`.body_limit(..)`) with a per-route
  `#[meta(BodyLimit(..))]` override.
- §3.7: the `BackendLimits` table "is the starting point, and the conformance tests confirm each
  entry before release"; rocket's inherited sockets and port 0 "depend on the version's
  custom-listener support".
- §4.1: `session_with = |..| ..` as the alternative to `Default` for a session.
- §4.2, §13 decisions 5 and 6: `codec = msgpack` per gateway; `overflow = drop_oldest` as the
  alternative to closing a slow consumer with 1008.
- §6.2, §13 decision 7: gRPC reflection and the GraphQL playground on in debug builds, opt-in in
  release builds.
- §13 decision 2: `CallError::grpc_code(..)` overrides `Conflict → ABORTED` per error.
- §2.7: broker links "take their client libraries' TLS configuration".
- §2.4: "Each transport documents its rendering in §§3–7"; the RPC and WebSocket renderings of an
  `ExtractError` are stated by kind, the HTTP one adds 413 and 415.
- §5.2: the `fw-rpc-conformance` scenario list (unary, each streaming shape, cancel mid-stream,
  unknown pattern, deadline, binary payload, oversized payload, drain) is named and not yet
  written.
- §13: all eight defaults are "open to change"; the author asks the user to confirm 1 and 4.

## Probe appendix

Toolchains: rustc 1.98.1 (2026-09-01, the default) and rustc 1.88.0 via `cargo +1.88`, edition
2024. Every probe behaved the same on both; where a message's wording differs the 1.88 form is
noted. The probe crate is `docs/experiments/di-redesign/probes/` (no dependencies); its `target`
was deleted after the run. The proc-macro pair and the serde probe needed dependencies and live in
their own workspace beside this file, `transports/probes/` (members `macros`: syn 2, quote 1,
proc-macro2 1; `app`, with the `ok` and `bad` binaries; `fieldrec`: serde 1 with `derive`).

| Probe | Shows | Result |
|---|---|---|
| `p21_key_const_assert` | X1: `key_in` as a byte-comparing `const fn`; a named associated const inside the impl using `Self::__FW_KEY_*`; a free `const _` naming the type | compiles, prints `http in [http, rpc]: true` |
| `p21b_key_const_unread_assoc` | X1: the associated-const placement with the key misspelled and nothing reading it | compiles on both toolchains: the assertion never ran (R2) |
| `p21c_const_underscore_in_impl_fails` | X1: `const _` inside an impl | "`const` items in this context need a name" |
| `p21d_generic_const_unread` | X1: a failing associated const on `impl<T> Ctl<T>`, unread | compiles, prints `generic check never ran` |
| `p21d_generic_const_read_fails` | X1: the same, read from `mount()` and `mount` instantiated | E0080 ("evaluation of `Ctl::<u8>::__FW_KEYS_CHECK_htpp` failed" on 1.88) |
| `p21e_key_const_free_misspelled_fails` | X1: the free `const _` naming the type, key misspelled, no reader | E0080 carrying "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)" |
| scratch `app/src/bin/ok.rs` | X1 with a real attribute pair: `#[routes_like("http")]` on the impl emits the free `const _` naming `<Ty>::__FW_KEY_<name>` for each method carrying `#[get_like(..)]`; `#[get_like("..")]` on each method emits `__FW_KEY_<name>` and `__fw_mount_<name>`; the outer macro expands first | compiles, prints `ok: http rpc`: the outer macro's const resolves items the inner attribute wrote |
| scratch `app/src/bin/bad.rs` | the same with `#[routes_like("htpp")]` | E0080 spanned on the `"htpp"` token, message as above |
| `p22_shared_values_generic` | X2: `__FwShared<V0>` with the value's type inferred in `mount`; two per-handler fns bounded `V0: Guard<Http>` and `V0: Guard<Rpc>`; one `Arc` unsized to each role | compiles, prints `strong count 3; http sees http-limit; rpc sees rpc-limit` |
| `p22b_shared_value_missing_role_fails` | X2: the value implements `Guard<Http>` alone | E0277 "`RateLimit: Guard<Rpc>` is not satisfied" at the `__fw_mount_get_rpc` call |
| `p23_param_autoref_fallback` | §2.2: `Param<T>` for `Dep`, `Json`, `Option<P: Param<T>>` (forwarding `CONSUMES_BODY`), `Injected<S>`; rank one `P: Param<T>`, rank two `S: FromContainer` as `Injected`, the transport a parameter of the probe type | compiles; `Dep` and `Option<Json>` take rank one, `Session` and `Option<Session>` rank two; `Option<Json<_>>::CONSUMES_BODY == true` |
| `p23b_param_option_overlap_fails` | §2.2: `Param<T> for Option<P: Param<T>>` beside `Param<T> for Option<S: FromContainer>` | E0119 conflicting implementations for `Option<_>` |
| `p24_answer_probe_order` | §2.3: `Classified` arm on `&&Probe<Result<V,E>>`, `Into<BoxError>` on `&Probe<Result<V,E>>`, `Answer` on `Probe<V>`, called as `(&&&Probe::new(out)).answer()`; the first through `type ApiResult<T> = Result<T, ApiError>` | compiles, prints `classified: not_found`, `boxed: io`, `value: Json` |
| `p24b_answer_probe_reversed` | §2.3: the arms placed in the order the text lists them | compiles, prints `boxed: user not found` for a `Classified` error (R1) |
| `p25_body_consumer_pairwise` | §2.2: three pairwise `const` assertions over `CONSUMES_BODY`, one through `type Body<T> = Json<T>` | compiles, prints `3 params, 3 pairs checked` |
| `p25b_body_consumer_two_fails` | §2.2: `Json` beside `Login<T> = Form<T>` | E0080 carrying "`user` and `login` both consume the body; a handler reads the body once" ("evaluation of constant value failed" with the message on 1.88) |
| `p26_rpit_captures_self_fails` | §3.1: `fn events(&self) -> Sse<impl Iterator<..>>` boxed as `'static` | "lifetime may not live long enough" at the box |
| `p26b_rpit_use_bound` | the same with `+ use<>` | compiles, prints `[1, 2, 3]` |
| scratch `fieldrec` | §3.2: a serde `Deserializer` that records what `T::deserialize` asks for and returns before producing a value | `struct → Fields(["id", "slug"])` (renamed field by its serde name), `(u64, String) → Tuple(2)`, `u64 → Scalar`, newtype `Id(u64) → Scalar` through `deserialize_newtype_struct`, `HashMap → Map`, `#[serde(flatten)] → Map` (no names) |
| `p27_classify_from_blanket` | addendum A3 (a): `impl<E: Classify> From<E> for CallError` with `CallError` not `Classify`; `?` on a `Classify` error in a fn returning `Result<_, CallError>`; an `E: Into<CallError>` bound accepting both `UserError` and `CallError` | compiles, prints `not_found`, `not_found`, `internal` |
| `p27b_classify_from_boxed_impl` | addendum A3 (b): `impl From<BoxError> for CallError` and `impl From<std::io::Error> for CallError` beside that blanket | compiles on both toolchains: coherence accepts the pair, the trait being local |
| `p27c_classify_for_callerror_fails` | addendum A3 (a): `impl Classify for CallError` added | E0119 "conflicting implementations of trait `From<CallError>` for type `CallError`", "conflicting implementation in crate `core`" |

What the probes do not cover: X5–X8 and X13 against the built core are Read, not run; the
`HttpBackend` claims about actix and rocket are Asserted; every specification claim is cited and
not executed. F298 stands: the probes were run on 1.88 by hand.

## Addendum: the naming exchange

On `transports/RESPONSE.md`, "Naming exchange during the review", received while the review above
was being written and not yet signed off. The stance is the one above: the renames are the
direction, and what follows maps them onto the review's findings, probes the two coherence claims
the author asked to have probed, and answers the two questions the exchange leaves open.

### A1. The renames against the review

| Was | Now | Review items that name the old spelling | Substance |
|---|---|---|---|
| `Param<T>::extract`, error `ExtractError { param, failure: ExtractFailure }` | `FromCall<T>::from_call`, error `ExtractError` flat (each variant carries `param`, with a `param()` accessor), `ExtractFailure` folded into it | R7 (`Param::dependencies`), R9, R10 (`CONSUMES_BODY`), R16, §12's `Param<T>` row, P23 | None in R7, R10, R16. In R9 the two autoref ranks are `FromCall<T>` over `FromContainer`, both `From*`, which is the visibility the author wants in a diagnostic; the `on_unimplemented` text becomes "`{Self}` cannot be built from a `{T}` call", with the second note naming `FromContainer`. R8 stands under the flat shape as `Malformed { param, source: Redacted }`: the `Redacted` is the problem, whichever shape. (Read.) |
| `Answer<T>::into_reply`, error `ReplyError` | `IntoReply<T>::into_reply`, error `IntoReplyError` | R1, appendix P24/P24b | R1's substance changes and is restated in A2. §2.3's value-API helper `Answer::from_result(r)` becomes a `from_*` on an `Into*` trait, which points both ways under the author's From/Into rule; with the blanket `From` below it is `r.map_err(CallError::from)`, so the helper can go, or become a free fn. (Read.) |
| `FromContainer::read` (built) | `FromContainer::from_container` | R9 names the trait only | Mechanical. In `crates/ulo`: the declaration `dependency/mod.rs:38`; the six impls `dependency/dep.rs:55`, `many.rs:60`, `ext.rs:41`, `option.rs:14` with the recursive `S::read` at `:15`, `handles.rs:14` and `:28`; the two `macro_rules` arms in `binding/factory.rs:76` and `:96`; doc comments at `resolver.rs:18`, `execution/mod.rs:148` and `binding/factory.rs:2`. In `crates/ulo-macros`: the generated call `shared/dependencies.rs:54` and its doc at `:46`; the helper `dependencies::read` at `:48` and its callers `injectable/impl_form.rs:35` and `struct_form.rs:44` are the macro's own names and may stay. Documents: core `DESIGN.md` §3.2 (line 113) and §3.8 (line 338); `BUILD_PLAN.md` line 158 lists the helper. `Resolver::dep`/`many`/`ext`/`input` and `Entry::resolve` are the resolver's own reads and are untouched. (Read.) The argument stays a `Resolver`, the view of the container, as `from_str` takes the `&str` it is built from. |
| `Classified` (`kind`, `public_message`, `details`), `#[derive(Classified)]`, `#[kind(..)]` | `Classify::classify`, `#[derive(Classify)]`, `#[classify(..)]` | R1, §2.4's table, §3.1's example | None beyond A2. A derive helper attribute is scoped to its derive, so `#[classify(not_found)]` sits beside thiserror's `#[error(..)]` without collision. (Read.) |
| `CallError::classified(e)` | `impl<E: Classify> From<E> for CallError`; `CallError::from(e)` or `?` | R1 | A2 and A3 (a). |
| free `classify(&BoxError) -> CallError` | `CallError::from_boxed(BoxError)` | R17, X12 | R17 stands: the recogniser cannot see `CancelReason::Deadline`. Under the new name the fix is for the transport to test `cancel_reason()` before calling `from_boxed`, which keeps `from_boxed` a one-argument constructor; a `from_*` with a second argument is not the std shape. The exchange names the function twice, `CallError::from_error(&BoxError)` (its lines 48 and 58) and `from_boxed(err: BoxError)` (lines 102 and 108); by value is what `CallError.source: Option<BoxError>` needs, since the error handlers hand the unclaimed error on by value, so `from_boxed(BoxError)`. A3 (b) removes the coherence reason the exchange gives for not writing it as `From`. |
| `HttpBackend` | `fw_http::Backend` | R27, the appendix's last paragraph | Spelling only. |

### A2. R1 under the new names

With `impl<E: Classify> From<E> for CallError`, the first arm's bound can be `E: Into<CallError>`
rather than `E: Classify` (Probed, P27: the bound accepts a `UserError` through the blanket and a
`CallError` through the reflexive `From`). It then also catches a handler returning
`Result<_, CallError>` and a user type with its own `From<MyErr> for CallError`. The overlap R1 is
about remains: everything `Into<CallError>` here is `Into<BoxError>` too, `CallError` being an
`Error`, so the `Into<CallError>` arm sits on `&&IntoReplyProbe<Result<V, E>>`, the `Into<BoxError>`
arm on `&IntoReplyProbe<Result<V, E>>` and the `IntoReply` arm on the bare probe. Placed as §2.3
lists them, a `Classify` error takes the boxing arm and its kind is lost (P24b). The rename widens
what the first arm catches and removes nothing of the ordering. The first arm's body is
`BoxError::from(CallError::from(e))`, the second's `BoxError::from(e)`; both reach the handlers as
`BoxError`, and `from_boxed` finds a `CallError` inside either by downcast, so a `Result<_,
CallError>` that reached the second arm would still render right; the first arm hands the
handlers a `CallError` without the detour.

### A3. The two coherence claims

(Probed, P27, P27b, P27c, on 1.98.1 and 1.88.0, same result on both.)

**(a) holds.** `impl<E: Classify> From<E> for CallError` compiles while `CallError` does not
implement `Classify`, and `?` converts a `Classify` error inside a fn returning `Result<_,
CallError>` (P27). Adding `impl Classify for CallError` is E0119, "conflicting implementations of
trait `From<CallError>` for type `CallError`", "conflicting implementation in crate `core`" (P27c).
The rule "`CallError` keeps an inherent `kind()` and never implements `Classify`" is load-bearing;
no `on_unimplemented` can carry it, so the E0119 text above is what a future reader meets, and
the trait's doc should state the rule, as the exchange says.

**(b) is accepted, against the exchange's expectation.** `impl From<BoxError> for CallError` and
`impl From<std::io::Error> for CallError` both compile beside the blanket (P27b). The pair
overlaps only if `Box<dyn Error + Send + Sync>: Classify` could hold, and `Classify` is local to
the defining crate: no downstream crate may implement a foreign trait for a foreign type (`Box` is
`#[fundamental]`, but `dyn Error` is std's), and no upstream crate knows the trait, so coherence
treats the obligation as knowable and false. `anyhow` is refused the same impl because its bound
is std's `Error`, a foreign trait an upstream crate may implement for `Box<dyn Error>` in a future
version; the local trait is what decides it. (Read, for the reasoning; the compiler's verdict is
Probed.) So of the exchange's two reasons for a named constructor, the first does not hold and the
second does: a recogniser that walks an error and maps `GuardRejected`, `PanicRecovered`,
`LookupError::Construct` and the rest is not a plain conversion, and `From<BoxError>` would let
`?` run it on every `BoxError` with no visible call. That is the author's decision to make with the
evidence; coherence does not make it. The same holds for a `From<LookupError>` or
`From<ExtractError>` beside the blanket; `ExtractError` is better made `Classify` (`BadRequest`,
`Unprocessable` for `Invalid`) so it converts through the blanket, which is already how §2.2 has
it reach the handlers. P27b is one crate holding the trait, the type and the impls, which is
`fw-transport`'s situation.

### A4. GraphQL without HTTP (beside Q4)

(Spec.) The GraphQL specification (October 2021 edition) defines the language, validation and
execution and names no transport; "GraphQL over HTTP" (GraphQL Foundation working draft) is one
binding and graphql-transport-ws another. (Read, §7.) The `Engine` SPI is transport-neutral in its
signature: `execute(GqlRequest, ExecutionRef) -> BoxFuture<GqlResponse>`, `subscribe(..) ->
BoxStream<GqlResponse>`, `sdl()`, and the per-execution context through `exec.get::<GqlContext>()`
uses the core alone. What ties it to two transports is the crate, not the trait: the §1 table has
`fw-graphql` meeting the core "through `fw-http`, `fw-ws`", holding the HTTP endpoint, the
graphql-transport-ws gateway and the playground beside the SPI, and `GraphqlModule::for_root(
GraphqlConfig::at("/graphql").subscriptions("/graphql/ws"))` is HTTP-path-shaped. Nothing in §7
says how an engine is served over an RPC pattern, a plain WebSocket gateway or a gRPC method, and
an RPC-only application that wants the `Engine` pulls `fw-http` and `fw-ws`.

Refinement, inside the design: split the crate into a neutral `fw-graphql` (`Engine`,
`GqlRequest`, `GqlResponse`, the `GqlContext` convention, `fw_graphql::dep`) that the two engine
adapters depend on alone, and the bindings `fw-graphql-http` (the endpoint, the playground, the
GraphQL-over-HTTP status rules) and `fw-graphql-ws` (the graphql-transport-ws gateway). With
`GqlRequest: Deserialize` and `GqlResponse: Serialize`, both JSON-shaped in the GraphQL-over-HTTP
draft, a user serves the same engine over any link with an ordinary handler, `#[fw_rpc::message(
"gql")] async fn gql(&self, req: Payload<GqlRequest>, engine: Dep<dyn Engine>) -> GqlResponse`, and
`subscribe`'s stream over RPC's streamed-reply shape. Question for the author: is that split the
intent, with `GraphqlModule` becoming the HTTP binding's module, and does the neutral crate export
the engine under `dyn Engine` so a handler on any transport can take `Dep<dyn Engine>`?

### A5. `MiddlewareNext` as `fw_http::Next` (open)

The author's answer is not in the exchange. Evidence on the collision: the core's
`Next<'a, T: Transport>` holds the interceptor chain and is generic over the transport;
`fw_http::Next<'_>` would hold the middleware chain and the request. A module that writes `use
fw::Next;` and `use fw_http::Next;` fails E0252 (name defined twice) and writes one of them
qualified or `use fw_http::Next as HttpNext`, which is the alias the user suggested; the crate can
also export `pub type MiddlewareNext<'a> = Next<'a>;` for anyone who wants distinct names. (Read.)
Precedents for the module-path form: std's `fmt::Result` beside `result::Result`, `io::Error`
beside `error::Error`, `fmt::Write` beside `io::Write`, imported by module by convention; and the
core's own `app::Bound`, the typestate after `listen()`, beside `ulo::Bound`, the timeout enum,
documented as "the timeout enum is `crate::Bound`" (Read, `crates/ulo/src/app/mod.rs:7`). The two
`Next` types differ in signature, so a wrong import fails at the type. Nothing found bears
against the rename; it is the author's to confirm.

### A6. The rules applied to every trait and error type in both designs

Rules as the exchange states them: a trait is named after its predominant method, otherwise after
its role or concept; an error after its operation, or after its domain for a family; `From*`
builds `Self` from a source, `Into*` consumes `self`; `Try*` only beside an infallible sibling.
(Read over core `DESIGN.md` §3, §10.2, §13, the built `crates/ulo`, and `transports/DESIGN.md`.)

Pass, listed so the set is settled in one round: method-named `Construct::construct`,
`Validate::validate`, `OnModuleInit::on_module_init` and the four other hook traits,
`IntoConstructed::into_constructed` (hidden); roles `Guard`, `Interceptor`, `ErrorHandler`,
`Middleware`, `Controller`, and the `Erased*` twins; concepts `Module`, `Transport`, `Server`,
`Timer`, `Factory`, `Meta`, `Link`, `Engine`, `Backend`, `BroadcastAdapter`, `Method` (gRPC),
`GatewayConfig`; markers `Scope`, `ExplicitScope`, `HookCapable`, `AllowedIn`, `Role`, and the
`handle` state traits. Errors: `ConstructError`, `LoadError`, `LoadRefusal`, `IntoReplyError`
(operation); `LookupError`, `ConnectError`, `StartupError`, `ShutdownError`, `ShutdownFailure`,
`WiringError`/`WiringErrors`, `CallError`, `RpcError` (domain). `Try*`: every `try_*` in the core
has its infallible sibling (`value`, `singleton`, `execution`, `transient`, `provide_with`, `with`,
`override_factory`), and the transport design writes none; `Endpoint::parse` and
`Tls::from_pem_files` return `Result` under plain names. Direction: `Dep::from_arc`/`into_arc`,
`Redacted::into_inner`, `CallError::from_boxed`, `Tls::from_pem`, `FromCall`, `IntoReply` all point
the right way; `Key::of`, `ModuleIdentity::of_type`/`of_value` build from a type rather than a
value and are outside the `From` rule.

To settle:

1. `ExtractError` beside `FromCall::from_call`: the error names an operation the API has no method
   for. The author's own precedent covers it (`FromStr::from_str` fails with `ParseIntError`, "the
   operation users think of"), and `FromCallError` is the literal reading; the exchange chose the
   first and should say so where the rule is written, or the next reader reopens it. (Read.)
2. `Answer::from_result` (§2.3) under `IntoReply`: a `from_*` on an `Into*` trait (A1). (Read.)
3. `CallError::from_error(&BoxError)` and `CallError::from_boxed(BoxError)` are both in the
   exchange for one function (A1). (Read.)
4. The condition-named errors fall under neither branch of the error rule: core `Closed`,
   `NoTimer`, `GuardRejected`, `PanicRecovered`, `Redacted`; transport `Refusal` (§4.1), and
   `GuardRejected` reused. They name what happened rather than an operation or a domain. std has
   the same third class (`PoisonError`, `Utf8Error`, `AllocError`, `LayoutError`), so the rule is
   short a branch rather than the names wrong; within the class the core's are past participles and
   the transport's `Refusal` is a noun, where `ConnectRefused` would match `GuardRejected`. A
   decision on the branch settles all six at once. (Asserted.)
5. `HandlerDecl` and `InputDecls` (X3, X4) abbreviate where the core writes `Dependencies` and
   `Requirement` in full; not one of the stated rules, and R7 already asks for another name for the
   first. (Asserted.)
