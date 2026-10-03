# Divergences: race 2a, CX (core extensions)

Every place CX's code departs from `transports/DESIGN.md` or `DESIGN.md`, or fills a gap a user
would see, beyond what `race2a-spine.md` already lists. Each entry gives what the design says,
what the code does, and why. Requests for other areas follow the entries.

Files changed: `transport/pipeline.rs`, `execution/mod.rs`, `graph/visibility.rs`,
`graph/scopes.rs`, `graph/wire.rs`, `error/wiring.rs`.

## Entries

### 1. `dispatch_late` sets `is_late` for good, and `EndStream` is read at the top level

- **Design:** §2.3: `dispatch_late` runs the error handlers with `cx.exec().is_late() == true`;
  `Err(fw::EndStream)` from a handler ends the stream. Silent on how long `is_late` stays true,
  and on an `EndStream` that no handler returned.
- **Code:** `dispatch_late` sets the flag before the first error handler and never clears it.
  `End` is answered when the error the walk ends with is an `EndStream` at its top level
  (`err.is::<EndStream>()`), which includes an `EndStream` passed in as the item error and left
  unclaimed. An `EndStream` wrapped inside another error, a `CallError` for example, is not
  looked for and renders as that error.
- **Why:** a late error can only arrive after the reply began, so no later error on the same
  execution can be an early one. Reading the top level alone matches how a handler writes it,
  `Err(EndStream.into())`.

### 2. `recover(Some(handler), ..)` sets the handler's `HandlerInfo` on the execution

- **Design:** X3: `dispatch` sets `HandlerInfo` before the first guard. §3.3: a pre-dispatch
  entry's failure after routing reaches the handler's error handlers. Silent on what
  `cx.exec().handler()` answers inside those error handlers.
- **Code:** with a handler, `recover` sets its `HandlerInfo` first, as `dispatch` does (a second
  set changes nothing, so the late path keeps `dispatch`'s). With `None`, `handler()` stays
  `None`.
- **Why:** an error handler written for `dispatch` reads the route's metadata through
  `handler()`, and a scoped pre-dispatch failure has a matched route to read.

### 3. `recover(None, ..)` resolves with the execution's current module (point CX resolves)

- **Design:** §3.3: on a miss only the global error handlers apply. Silent on the module.
- **Code:** the resolver `exec.resolver()` gives: the module `route_to` set, otherwise the one
  the execution opened in, which is the root for a transport that routes after opening.
- **Why:** global error handlers are contributions, built against their own modules, so the
  module decides only which table the role key is checked in. With no route matched the
  execution has not been routed, and the root is the module the design opens it in (X8).

### 4. A panicking `on_stream_end` callback is caught and dropped (point CX resolves)

- **Design:** X5: callbacks run synchronously when the reply stream finishes. Silent on a panic.
- **Code:** each callback runs under `catch_unwind`, in `report_stream_end` and in the immediate
  run of a callback registered after the end. The payload is dropped and the callbacks after it
  still run. Documented on `on_stream_end` and `report_stream_end`.
- **Why:** `Tracked` reports from its `Drop`. A panic there while the thread is already
  unwinding aborts the process, and any other panic skips the remaining callbacks and unwinds
  through the transport's write path. The panic hook prints the panic before the catch, so the
  failure stays visible without a logger in the core.

### 5. Handler parameters in steps 3 and 5, and how a report names them

- **Design:** X3: the wiring walk adds a handler's parameter reads to its roots as it adds a
  closure's; §6.4 and §10.1 step 5: the per-handler input check, its sample path running through a
  service.
- **Code:** step 3 checks `HandlerSpec::dependencies` against the controller module's table and
  reports a missing or ambiguous key with the consumer ``UsersController::get (param #1)``. Step
  5 takes each binding a parameter reads, and each contribution of a collection it reads, as a
  root of the input walk; a non-optional input a parameter reads directly is reported with the
  path ``UsersController::get_rpc (Rpc) → Dep<RequestHead> (param #1)``. Parameter reads take no
  part in the cycle check or the needs-execution pass: they are extracted per call, so a
  parameter reading `Ext<T>` makes neither the controller nor anything else per-execution.
- **Gap:** `#n` counts the handler's container-read parameters, not its position in the
  signature, since `Injected::dependencies` writes `Dependencies::add`. A handler
  `get(&self, id: Path<u64>, svc: Dep<Svc>)` reports `svc` as `param #1`. See request R1.

### 6. A handler parameter reading `Many<K>` blocks a lazy load contributing to `K`

- **Design:** §8.6: a lazily loaded module may not contribute to a collection it does not
  introduce. `reads_collection` listed bindings, hooks, readiness checks, enhancer closures and
  role keys as the base's reads.
- **Code:** a handler parameter's collection read counts too, refused as
  `LoadRefusal::Contribution`.
- **Why:** the handler reads the collection at every call, so a contribution added after wiring
  would change what a running handler sees, which is what the refusal exists to prevent.

### 7. `WiringError::InputConflict`, a new public variant

- **Design:** X4: a module's `m.input::<T>().seeded_by::<Tr>()` and a transport's `inputs` with
  the same `(key, seeder)` are one declaration; a transport-declared input's origin prints as
  "declared by transport `Http`". No variant carried a transport origin, since every existing
  input report names a module.
- **Code:** `InputConflict { key: KeyName, first: String, second: String }`, step 2. Reported for
  a transport's declaration beside a module's of the same key with another seeder, beside
  another transport's declaration of the key, or beside a single binding under the key. Each side
  is a pre-rendered line: ``declared by transport `Http` at <file>:<line>``, ``declared in
  AppModule with seeder `Rpc` at <file>:<line>``, or ``bound in AppModule at <file>:<line>``. A
  binding is reported once per key. Inputs that only modules declare keep the spine's reports:
  `DuplicateBinding` for a second module declaration and for a binding under a declared key.
  `Declared` gains `transport_inputs`, filled where the freeze calls `Transport::inputs`.
- **Why:** one variant covers the three conflicts without a new public origin type. The cost is
  that the sources are text: a test matches the variant and the key, not the origin's parts, and
  a module name inside the text is not lengthened when two modules print alike, as rendered text
  already is in `Missing::consumer`.

### 8. Two transports declaring one input is refused

- **Design:** an input has one seeder (`seeded_by::<Tr>()`, `InputDecl::seeder`). Silent on two
  transports whose `inputs` name the same key.
- **Code:** reported as `InputConflict` (entry 7), naming both transports. Before, the first
  transport to mount won without a report, and every handler of the second transport reading the
  input failed the per-handler check as `InputNotSeeded`.
- **For 2b:** §2.10 lists `ConnectionInfo`, `UpgradeHead` and `SessionHandle` under WebSocket,
  which has two transports, `Ws` and `WsConnect`. If both declare them, wiring fails; if one does,
  the other's handlers read them unseeded. One input with two seeders needs a design decision
  before `ulo-ws` declares its inputs.

## Requests

### R1. To T (`ulo-transport`) and MX (`ulo-handler-codegen`): name container parameters in reports

- **Item:** `Injected::dependencies` in `crates/ulo-transport/src/extract.rs`, reached through
  `Param::dependencies` from the code `emit::dependencies` generates.
- **What:** a handler's container parameter is recorded with `Dependencies::add`, so wiring
  reports print it as `param #n`, `n` counting container parameters only (entry 5). Recording it
  with `Dependencies::param::<S>(name)` would print ``param `svc` ``. `Param::dependencies` has no
  name to pass, so this needs a contract change: for example a `Param::dependencies_named(d, name)`
  that the `ViaContainer` impl answers with `d.param::<S>(name)` and the `ViaCall` impl with
  `P::dependencies(d)`, called by the generated code with the parameter's identifier.
- **Priority:** reports only; nothing is unchecked without it.

### Notes for P (`ulo-http` service and stage), no change requested

- `recover(Some(handler), ..)` sets the handler's `HandlerInfo` on the execution (entry 2), so a
  call through the scoped sub-step that then reaches `dispatch` keeps that one.
- `recover(None, ..)` resolves at the execution's current module (entry 3); call it before
  `route_to` on a miss, as the stage's order already does.
