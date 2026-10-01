# Response to REFINEMENTS.md

The design author's position on each refinement and answers to the twelve questions, as received.
The user signed it off in full on 2026-10-01: the thirteen refinements with the adjustments to R3,
R10 and R13, the twelve answers, the new variants and surfaces, and the three added defaults.

---

This is a strong review. Every refinement holds up, and I accept all thirteen. Two of them I'd adjust rather than take as written, and one probe result I'd want re-checked on 1.88. Below is my position on each item, then answers to the twelve questions. Nothing here has touched the document yet.

## Refinements

**R1 (`?` in `register`): accept.** Lowering `expr?` to `m.try_value(expr)`, which records the error for `wire()`, is better than letting `register` return `Result`. An early return would stop registration at the first failure and break the "all errors in one pass" rule [44]. Adding "`register` never returns early" as a stated rule is right.

**R2 (plain versus fallible factories): accept.** It's the honest outcome of the coherence problem. The `try_` prefix follows std convention (`try_from`, `try_reserve`), so the eight names read naturally. The `#[module]` macro keeps one spelling because it writes the concrete call site, which the probe shows the ranking needs.

**R3 (the guard example contradicts its rule): accept. Here's the option I'd pick.** Controller-level enhancers apply to every handler, strictly. If any handler's transport lacks the role, that's a compile error naming the handler. Alongside that, allow the transport-scoped form `#[guards(http = AuthGuard)]` for mixed controllers. I agree with ruling out "apply only where the role exists", since a silently unguarded RPC handler is exactly the bug [25] exists to prevent.

**R4 (`execute` borrows its argument): accept, and take the `AsyncFnOnce(&Execution)` spelling.** The framework keeps ownership and closes the execution when the future completes. This combines well with R13: the execution is internally shared anyway, so code that needs to hand it to a spawned subtask can call `exec.handle()` for an owned clone.

One thing to probe: `execute` must stay an inherent `async fn`. Its future is then `Send` through auto-trait leakage whenever the caller's closure future is. If `execute` ever moves into a trait, stable Rust can't bound the `AsyncFnOnce` future as `Send`. I'd add a probe asserting `Send` on `app.execute(async |exec| ..)` from a concrete call site.

**R5 (`KeyNames` isn't const-constructible): accept.** Two `&'static str` fields with manual `Eq` and `Hash` over the `TypeId`s is the correct fix.

**R6 (the `AllowedIn` message has the wrong subject): accept as written.**

**R7 (no hint for the `Send + Sync` refusal): accept, and add a clarification.** Two situations get conflated here:
- The trait lacks the `Send + Sync` supertraits. Then `Dep<dyn Repo>` fails at compile time with the auto-trait message and no hint. Nothing can improve that, so documentation carries it.
- Both spellings compile, but registration used `dyn Repo + Send + Sync` and the site reads `dyn Repo`, or the reverse. That's where the wire-time name comparison helps.

So strike the compile-time claim in §12's row, and add the wire-time hint for the second situation.

**R8 (two orders for one collection): accept.** Depth-first post-order from the root, imports in written order, then declaration order. It's deterministic, and imports come before importers, which matches intuition.

**R9 (`Send` resolver futures require `Sync`): accept.** `Execution: Send + Sync` and `seed<T: Send + Sync + 'static>` become written rules, and body streams live on `Cx` behind a take-once slot. Transport authors should read that rule in the text, not reverse-engineer it from an error at `+ Send`.

**R10 (the alias has one parameter too many): accept, with a different spelling.** I'd write `m.alias::<PgPool, ReadOnly>().of::<Replica>()`. It reads as "alias `PgPool @ ReadOnly` of the `Replica` binding," so the direction is in the words rather than in parameter position.

**R11 (hooks compiled but skipped on factory-built types): accept both halves.** Add `provide_with::<T>(factory)` for `Construct` types that need a custom build, and add a sentence in §9.1. I checked whether wiring could catch the silent skip instead. It can't: the graph only learns which types are `Construct` through `provide`, so documentation plus a dedicated method is the right level.

**R12 (`Many` builds every guard first): accept.** `Many<T>` is the eager form for user code. Transports get a lazy surface, `Resolver::entries::<T>()`, which yields one handle per contribution with its own `resolve().await`. §7's pipeline is written against that.

**R13 (`&mut Cx` and who owns the execution): accept the main change, but correct the cancellation part.** `Cx` becomes a cheap-clone `Send + Sync` handle, guards and interceptors take `&T::Cx`, and a streaming reply carries a clone. Agreed.

But "the signal fires when the last clone drops" has it backwards. When the last clone drops, no one is left to observe cancellation. Two events need separating:
- **Cancellation** is fired by the transport: the client disconnects, the deadline passes, or shutdown begins. It fires while clones are still alive, which is the whole point.
- **End of execution** happens when the last clone drops. That's when the per-execution cache is released and execution-scoped instances are dropped.

With that split, a streaming reply keeps its per-execution instances alive exactly as long as it runs, and it can still observe cancellation.

I also agree §14 is missing three defaults: the collection order (R8), the execution's ownership (R13), and `Many<T>` being eager (R12).

## Questions

**Q1 (lookups without a module handle): they use the root module's visibility.** That means the root's own bindings, its imports' exports, and globals. The root's table is already free of ambiguity once wiring passes, so no new error variant is needed. A service exported only within a subtree is unreachable from `app.get` by design. Reaching it goes through `app.module::<M>()?`, and a job that belongs to a subtree runs through that module's `execute`. Inside a transport call, the execution's resolver carries the dispatching controller's module, so handlers see what their module sees.

**Q2 (an override matching several private bindings): it's a wiring error listing every match.** Two ways out: `.in_module::<M>()` to pick one, or `.everywhere()` to replace all of them explicitly. Replacing several silently would make tests pass for the wrong reason.

**Q3 (what `load` runs on after `serve`, and late contributions):** Before `serve`, the app hands out an `AppHandle`, which is `Clone + Send + Sync` and wraps the shared inner state. `get`, `module`, `execute`, `load` and `close` all live on that handle. `serve(self)` consumes only the `Bound` typestate. The graph sits behind a lock that only `load` writes.

For collections, I'd make the rule static rather than time-dependent. A lazy module can't contribute to any key that a pre-existing binding reads as `Many<T>`, whatever that binding's scope. It can contribute only to collections it introduces itself. That's checked at `load` without asking what has already run.

**Q4 (how secrets get registered): not through a global list.** A process-wide list leaks between tests. Registration is explicit per graph:
- `m.secret(&self.url)` in the value API.
- The `#[module]` macro auto-registers any `Secret<_>` field of a configured module, since it can see the fields.
- `m.value(Secret<_>)` registers itself.

§13's hand-written `DbModule` gains one `m.secret(&self.url)` line. The URL-userinfo strip stays as a backstop, not the mechanism.

**Q5 (does `Ok(false)` reach the error handlers): yes.** The pipeline turns a refusal into a core error, `GuardRejected { guard: &'static str }`, and routes it through the handlers like any other error. Handlers downcast the `BoxError` to reshape the response per route. A guard that wants a different status, say 401 instead of 403, returns its own `Err`.

**Q6 (bare and keyed forms of one module): two modules, by design.** That follows from the identity rule. Anyone who wants one pool under two keys should use an alias, not a second import. On the export boundary:
- **Re-exports are requalified**, since they leave the boundary like any export.
- **Contributions aren't exports**, so they keep their key. Two keyed databases correctly produce two health indicators.
- **Execution inputs are app-wide** and belong to transports. A keyed module declaring one is a wiring error.

**Q7 (a per-execution service reading an HTTP input from an RPC path): a wiring error.** Inputs get declared together with the transport that seeds them, as in `m.input::<RequestHead>().seeded_by::<Http>()`. Wiring then walks each handler's reachable execution-scoped bindings and checks every non-optional input against that handler's transport. A service shared across transports can read the input as `Option<Dep<RequestHead>>`. Standalone executions remain the only runtime case, because nothing statically says what they seed.

**Q8 (`Option` over a runtime miss): `None`.** One rule for all of them: `Option<S>` is `None` whenever `S` would fail with `NotFound`, whether statically unbound or absent at runtime. Every other error still propagates: construction failures, `ExecutionRequired`, and ambiguity.

**Q9 (tie-break for hooks and readiness checks): the R8 order.** Concretely, a stable topological sort. Among the bindings that are ready to run, pick the smallest (module post-order index, declaration index). A module's own hooks follow its providers'. Shutdown is the exact reverse.

**Q10 (what a hand-written `Construct` calls): explicit methods, not probe types.** `Hooks<T>` gets one method per hook, each with a `where` bound: `h.on_module_init()` compiles only when `T: OnModuleInit`. A hand-writer knows which traits they implemented, so they call those methods. The probe types stay hidden macro internals. A `fw::hooks!(h)` helper macro can sit on top as sugar that probes all five.

**Q11 (`ModuleRef::get` during connect for an unbuilt singleton): refused with its own variant.** That's `LookupError::NotReady { key }`. Building on demand would break eager ordering, and `NotFound` would lie about a key that exists. Once `connect` returns, it can't occur.

**Q12 (whether `WrongType` is reachable): yes, keep it, but give it its real meaning.** It fires on erased-key lookups in the value API, where an integration holds a runtime `Key` and asks for the wrong `T`. Single-versus-collection mismatches get their own variant, `WrongKind { expected, found }`.

## Two small notes

The probes ran on 1.98.1. Stable features only move forward, so that's sound for what compiles. But diagnostic wording, including how `{S}` renders in `on_unimplemented`, and some inference behavior aren't guaranteed identical on 1.88. I'd add a CI job pinned to 1.88 before treating the diagnostic probes (P03b, P06, P07) as final.

On sharing the probes: if a zip won't upload, you can paste them as plain text, all files concatenated with a `// ==== src/bin/p05_site_read_erased.rs ====` line between them. I can read and reason about them that way. I can't run them here, though, so they'd stay "Read" rather than "Probed" on my side.

Once you sign off, the changes to the document come down to these: the thirteen refinements with my adjustments to R3, R10 and R13; the twelve answers written into their sections; the new variants (`NotReady`, `WrongKind`) and the lazy surfaces (`entries`, `AppHandle`, `provide_with`); and three more defaults in §14.

---

## Second response: the two open questions and the eleven filled details

Received 2026-10-01, answering the two questions and the eleven details raised when the first
response was folded into `DESIGN.md`. The user signed it off in full the same day, adopting the in-flight
shutdown rule as proposed.

Good to have both probes settled, and good news that 1.88 matches 1.98 across the board.

## Question 1: When does an execution end?

Your rule is right: `execute` drops its handle when the closure's future completes, and any outstanding `handle()` clone keeps the cache and execution-scoped instances alive until it drops. Force-ending the execution would bring back the problem R13 fixed. A subtask holding a clone would find its instances gone underneath it, so either every handle access would need to be fallible or we'd need a use-after-end error. Neither is worth it.

The text should also say one thing explicitly: **completion of `execute` does not fire cancellation.** That keeps it consistent with transports, where finishing the response doesn't cancel a streaming body. Cancellation for a standalone execution fires only on its deadline or when the app closes. A subtask that should stop when the job finishes has to be awaited by the job, not left detached.

## Question 2: `handle.close(..)` while `serve` is waiting

It ends `serve`. Shutdown is a single, one-time event, and whichever trigger arrives first wins: the signal future passed to `serve`, or a `close` call on any handle. That trigger's signal name is the one the hooks receive.

- **A `close` while shutdown is already running** doesn't restart anything. It waits for the same shutdown to finish and returns the same outcome. Its own signal argument is ignored, and so is `serve`'s signal if it arrives during a handle-initiated close.
- **Both callers get the outcome.** `serve` and every `close` caller receive the result, so the shutdown error needs to be `Clone`. Wrapping its contents in an `Arc` internally is enough.
- **Without `serve`** (a job or CLI on `Connected`), `close` runs the same sequence minus the socket step.

This leaves one decision open that the design doesn't cover yet: what happens to in-flight executions during shutdown. I'd do it this way: at the start of the socket step, transports stop accepting, then fire cancellation on every live execution and wait for them to drain, up to a timeout configured with the `Timer`. Executions still alive after the timeout are abandoned, and the shutdown result reports how many. That's a new rule, though, so it's your call.

## The eleven details

1. **Confirmed, with one addition.** `seed` belongs only on `Execution`, the holder's handle, never on `ExecutionRef`. Otherwise a subtask or a guard could inject inputs that the wiring check never saw. `Execution` itself is not `Clone`.
2. **Confirmed.** `try_provide_with` follows from the same coherence problem.
3. **Confirmed.** The key already carries the bound type's name, so `{ key, requested }` gives the error everything it needs to show both sides.
4. **Confirmed.** It's worth one sentence that `ModuleRef::execute` resolves with that module's visibility, while the other two use the root's, as in the Q1 answer.
5. **Confirmed.**
6. **Confirmed, with two notes.** First, the unscoped form remains the default and must match exactly one binding. `.everywhere()` is the explicit opt-in, not the default. Second, configured and keyed modules need a way to name one instance, so add `.in_module_keyed::<M, Q>()` and `.in_module_of(&config)`. `in_module::<M>()` alone is ambiguous there, and should fail as an ambiguous override rather than pick one.
7. **Confirmed.** "Forbidden status" should be defined per transport: HTTP 403, gRPC `PERMISSION_DENIED`, an error reply on RPC, an error frame on WebSockets. Each transport crate documents its mapping.
8. **Confirmed, with one more element.** The error should also print the dependency path from the handler to the service that reads the input, and both transports involved (the handler's, and the one that seeds the input). Without the path, the user knows what is wrong but not how the input got reached.
9. **Confirmed.**
10. **Confirmed.** Using one order for both is the point.
11. **Confirmed.**

Apart from the in-flight shutdown rule, nothing else here needs a decision from you.

---

## Third response: drain placement, `execute` after close, and the nine details

Received 2026-10-01. The user adopted both of its decisions, drain first with cancellation only at
the timeout and the phase table with `Result<R, Closed>`, and sent two proposals back to the author
before folding: `BeforeApplicationShutdown` moved ahead of stop-accepting, and a WebSocket 1001 close
plus a `draining()` notice for long-lived calls.

### The drain placement: you're right, it should move

Running destroy hooks while executions are still in flight was a mistake in my placement. An execution could reach a singleton after its `OnModuleDestroy` had run, for example a pool that's already closed, and the result would be confusing failures at the worst moment. The drain moves ahead of the destroy hooks:

> stop accepting → drain → `OnModuleDestroy` → `BeforeApplicationShutdown(signal)` → close sockets → `OnApplicationShutdown(signal)`

This keeps the memo's order for the three hooks and only moves the drain.

While checking this, I found a second problem, also in my own earlier rule, and I'd change it too. I said to fire cancellation on every live execution and *then* wait. That makes graceful shutdown not graceful: every request in flight is told to abort the instant shutdown starts, even ones that would finish in 50 ms. The better rule is:

1. Stop accepting.
2. Wait for live executions to finish on their own, up to `drain_timeout`.
3. At the timeout, fire cancellation on whatever is still running, and abandon it.

It's still one knob, and in-flight work gets the whole window to complete normally. Cancellation becomes the deadline signal, not the opening move. Say whether you want this. It's a behavior change to what's in the document.

One tradeoff comes with keeping the memo's hook order: `BeforeApplicationShutdown` now runs after traffic has stopped. So the common Kubernetes pattern of deregistering from discovery *before* you stop accepting can't use that hook. If you need it, it calls for a separate pre-drain hook. I wouldn't add one now. I'm only noting that the order makes it impossible today.

### The gap: `execute` during or after shutdown

It has to be refused. That means changing the signature, because `execute` currently returns the bare `R` and has no way to refuse:

```rust
pub async fn execute<F, R>(&self, f: F) -> Result<R, Closed>
```

`Closed` is a small public struct. Users whose closures return `Result` write `.await??`, which is mildly awkward but honest.

This calls for naming the app's phases explicitly, since the refusals line up with them:

| Phase | New executions | Singleton lookups |
|---|---|---|
| Running | allowed | allowed |
| Draining | refused (`Closed`) | allowed, since in-flight work still needs them |
| Destroying onward | refused | refused: `LookupError::Closed { key }` |

The last row covers abandoned executions. They're still alive after the timeout, and a fresh lookup must not hand them an instance whose destroy hook has run. A `Dep<T>` an execution already holds stays a valid object, and nothing can revoke it. The documentation should say that abandoned work may observe destroyed singletons through dependencies it already holds.

### The nine details

1. **Confirmed.**
2. **Confirmed.** Running every step is the right call. One stuck hook must not leave sockets open.
3. **Confirmed.**
4. **Confirmed.** Under the rule above, it's the window before cancellation fires.
5. **Confirmed.** `Bind` is the right variant, because the timer's absence only matters once there's a transport to drain. `wire()` can't know that.
6. **Confirmed.** I did mean the drain still runs on `Connected`.
7. **Confirmed for the example.** For real applications, consider a small `fw::Error` enum with `From<StartupError>` and `From<ShutdownError>`, so `main` keeps a typed error rather than a boxed one.
8. **Correction: include the winning signal.** A `close` caller whose own signal lost has no other way to learn what actually ended the app. CLI tools and containers often map signals to exit codes (130 for SIGINT, 143 for SIGTERM). So `Shutdown { signal: Signal, abandoned: usize }`.
9. **Confirmed.**

So you have two decisions: whether to adopt "drain first, cancel only at the timeout", and whether the phase table and `Result<R, Closed>` are right for `execute`. Everything else above is settled.
