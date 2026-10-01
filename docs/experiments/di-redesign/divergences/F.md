# Divergences: area F (the transport SPI and the enhancer pipeline)

Every place area F's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what F wrote, and why. All await the user's
sign-off. Requests for other areas follow the entries.

## Entries

### 1. The controller is built by `dispatch`, then read by the handler closure

- **Design:** §7 step 3: "Only once every guard admits does it build the interceptors, then the
  controller (per call if inferred), then run the handler inside the interceptor chain."
  `BUILD_PLAN.md` leaves to F whether `dispatch` resolves the controller through
  `exec.get::<C>()` inside the handler closure or takes it from the `MountedHandler`.
- **F:** the handler closure resolves the controller with `exec.get::<C>()`, the only place its
  type is known. Before running the chain, `dispatch` builds the controller binding into the call:
  a singleton is read from the store and a per-execution controller is built into the execution's
  cache, so the closure's lookup reads what is already there. A controller that fails to build
  reaches the error handlers without passing through the interceptors. A transient controller is
  not built ahead: a build in `dispatch` would be discarded and the closure would build a second.
  For that scope the controller is built where the closure reads it, inside the chain, after every
  interceptor has called `next.run()`.
- **Why:** `MountedHandler` cannot hold a per-execution controller, and `dispatch` is generic over
  the transport, not the controller. Building ahead keeps the design's order for the two scopes an
  `Auto` controller takes.

### 2. Error handlers within a tier run last-declared first

- **Design:** §7 step 4: "method level first, then controller, then global." Silent on the order
  inside one tier, and on the order among global contributions.
- **F:** the reverse of the stack order throughout: the method tier last-declared first, then the
  controller tier last-declared first, then the global contributions in reverse collection order.
- **Why:** the stack runs global, then controller, then method, each tier in the order written,
  and error handling unwinds it. The spine's `EnhancerSpec` doc states the same reversal, and the
  current framework consults handlers in that order.

### 3. An enhancer that cannot be built fails the call through the error handlers

- **Design:** §7 says errors go through the error handlers and that a guard's `Ok(false)` becomes
  `GuardRejected`. Silent on a guard, interceptor or error handler whose own construction fails:
  a per-execution build, a closure declaration whose site read fails, a global contribution.
- **F:** a guard, interceptor or the controller that fails to build ends the walk at that point,
  and its `LookupError` is offered to the error handlers like any other error. Later guards are
  not built, as after a refusal. An error handler that fails to build hands on its own
  `LookupError` in place of the error it was offered, as a handler returning `Err` hands on the
  error it returns, and the walk continues with the next handler. A failure to read the global
  error-handler collection ends the walk with that `LookupError`.
- **Why:** a construction failure inside a call is a `LookupError` (§10.2), and an error handler
  that downcasts it can map a domain failure, a tenant not found, to its own reply (§10.2's
  `Redacted` rationale). Passing on the original error past a broken handler would drop the
  handler's failure without a trace; replacing it keeps the failure visible to the handlers after
  it and to the transport.

### 4. `dispatch` catches no panic

- **Design:** §14.5 catches panics at poll boundaries in `connect`, `load`, `close` and a build
  inside a call (`LookupError::Construct`). Silent on a panic in a guard, an interceptor, a handler
  or an error handler.
- **F:** a build inside a call is caught where `AppShared::obtain` runs it (area D). A panic in
  `can_activate`, `intercept`, the handler closure or `handle` unwinds out of `dispatch`. The
  transport decides what a panicking call answers.
- **Why:** §10.2 has no error type for a panicked call that an error handler could match, and
  inventing one is a public addition. A transport already owns the task the call runs in and can
  wrap `dispatch`'s future to catch the unwind.

### 5. A transport's name is its marker's type name without the module path

- **Design:** `StartupError::Bind { transport: &'static str }`, `NoTimer { transport }` and
  `ShutdownFailure::Close { transport }` name a transport; §10.1's sample prints `Http` and `Rpc`.
  Silent on the text.
- **F:** `ErasedServer::transport_name` and `HandlerDecl::transport_name` answer
  `type_name::<T::Transport>()` cut after the last `::`: `Http` for `ulo_http::Http`. A generic
  marker keeps its full name, since cutting at the last `::` would land inside its parameters.
- **Why:** the field is `&'static str`, and a suffix of `type_name`'s string is one without an
  allocation. It matches the sample's spelling.

### 6. `MountedHandler<T>` is `Clone`

- **Design:** §13 names `Controller::mount` and `Mount`; the spine adds `MountedHandler` (spine
  entry 34) with no `Clone`.
- **F:** `impl<T: Transport> Clone for MountedHandler<T>`, written by hand so it does not require
  `T: Clone`.
- **Why:** without it no transport can call `dispatch`. A server receives its handlers through
  `Mounted::handlers()` borrowed for the length of `bind`, cannot construct a `MountedHandler`,
  and calls `dispatch(&handler, ..)` for every call it serves after `bind` returns. A clone costs
  one `Arc` per declaration.

## Requests for other areas

### R1 (A). `Resolver::instance` is called from the pipeline directly

`crates/ulo/src/transport/pipeline.rs` calls `Resolver::instance(id)` for a by-type enhancer
declaration and for the controller, beyond the `Entry::resolve` path the contract table lists.
Keep it `pub(crate)` with its signature. The pipeline also relies on `Resolver<'a>` being covariant
in `'a` and `Sync`, and on `Entries` and `Entry` being `Send`, which the spine's fields give.

### R2 (C). What `HandlerDecl` carries for the wiring pass

- `enhancer_deps` lists the controller tier, then the method tier; within each, guards, then
  interceptors, then error handlers, in the order written. A by-value declaration contributes
  nothing.
- `EnhancerDep::Type(key)` is the enhancer's own type, unqualified. The pipeline resolves it with
  `Graph::lookup(handler_record.module, key)` and expects `Visible::Binding`, so step 3 should
  resolve it against the controller module's visibility, report a miss as a missing dependency
  naming the handler (spine entry 36), and mark the binding `Role::Enhancer`.
- `EnhancerDep::Closure(sites)` is read per call inside the handler's execution, in the controller
  module. An `Ext` or `ExecutionRef` site in it needs no scope check, and the input check applies
  with the handler's transport.
- `transport_name` is already the short name (entry 5).
- The pipeline reads `FrozenBinding::effective` and looks the controller up as
  `Graph::lookup(HandlerRecord::module, MountedHandler::controller().key())`, the key
  `ErasedServer::bind` takes from `record.keys().next()`. A controller's binding has to be
  visible from its own module under that key, and `HandlerRecord::module` has to be the
  controller binding's origin.

### R3 (D). Fields `ErasedServer::bind` reads, and the `NoTimer` path

- `ErasedServer::bind` reads `AppHandle::shared` and `AppShared::config.timer`; `AppConfig::timer`
  is outside the contract table. Keep both.
- `bind` answers `Err(NoTimer { transport })` when the app has no `Timer`, rather than handing the
  server no clock. `listen` can refuse before binding or wrap that error as
  `StartupError::Bind { transport: server.transport_name(), source: redact(err) }`; either gives
  §12's refusal.
- `ExecutionRef::resolver()` must attach the execution (`exec: Some(..)`), so a per-execution
  enhancer and the controller built by `dispatch` land in the execution's cache, where the handler
  closure's `exec.get::<C>()` finds them.

### R4 (G). `EnhancerSpec`'s methods return `&mut Self`

`spec.guard::<G>()` and its kin return `&mut EnhancerSpec<T>`, and `Mount::handler` takes each tier
by value. `__enhancer_specs!` has to bind the spec to a local, call the methods on it, and pass the
local: `let mut c = EnhancerSpec::new(); c.guard::<AuthGuard>(); m.handler(name, c, method, h);`.
A chain on a temporary, `EnhancerSpec::new().guard::<A>()`, yields a borrow that cannot be passed.
`guard_with` and its kin infer `Args` from the closure's annotated parameter types.
