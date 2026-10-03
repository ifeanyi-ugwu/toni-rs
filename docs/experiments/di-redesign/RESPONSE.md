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

---

## Fourth response: the before-shutdown move, long-lived calls, and per-hook timeouts

Received 2026-10-01, answering the two proposals, then a question from the user on per-hook
timeouts. Reviewed in `SHUTDOWN_REVIEW.md`. The fifth and sixth responses settle each point of that
review; on the user's sign-off of those two, this response's proposals are folded as they amend it.

Both changes are improvements. I confirm them, with a few additions, one of which is a real gap.

1. BeforeApplicationShutdown before stop-accepting

Confirmed. Your reasoning holds: that hook's name only means something if it runs while the app is still serving, and moving it fixes the Kubernetes case without adding a sixth hook. Three things go with it.

The phase table gains a phase. During BeforeApplicationShutdown, new executions must still be allowed, because traffic is still flowing. So the table becomes:

Phase	New executions	Singleton lookups
Running	allowed	allowed
Stopping (before-shutdown hooks)	allowed	allowed
Draining	refused (Closed)	allowed
Destroying onward	refused	LookupError::Closed

Your example needs an injectable timer. self.timer.sleep(..) only works if the app's configured Timer is reachable as a binding. The core should bind it as a singleton, as Dep<dyn Timer>, at wiring time when one is configured. That way services can sleep without a runtime dependency, and a site reading it on an app without a timer becomes an ordinary missing-dependency error.

On the open point, give it its own bound, not drain_timeout. The two windows mean different things. Your example deliberately spends 5 seconds, and sharing a budget would silently cut the drain to 5 seconds. So add a separate before_shutdown_timeout, bounding the whole stage. On expiry, the unfinished hooks are abandoned and recorded as ShutdownFailure::Hook with a timed-out source, and the sequence continues.

The same hang risk applies to OnModuleDestroy and OnApplicationShutdown. A stuck destroy hook blocks close forever. I'd add a hook_timeout applied to each of those hooks individually, with the same failure recording. Without a Timer, none of these bounds can exist. The documentation should say that hooks then run unbounded, in line with your rule that a timerless close abandons executions immediately.

2. Long-lived calls during the drain

Both additions are confirmed. Here's what each needs.

WebSockets. Closing every connection with 1001 at the start of the drain is right. But there's a gap: the phase table refuses new executions during the drain, and a gateway's disconnect handler (presence cleanup, room leave) is itself a new execution. As written, server-initiated closes would silently skip that cleanup, exactly when every connection closes at once.

The fix is a transport-only exemption in the value API: Execution::open_terminal(..). It's permitted during Draining, counted in the drain, and subject to drain_timeout like any other execution. User code can't open one, so the refusal rule stays intact for everything else.

Streams. draining() as a separate signal from cancellation is the right split. Three small points:

Add a synchronous is_draining() next to it, for loops that check between items rather than racing a future.
draining() resolves when Draining begins, after the before-shutdown stage, not when close is first called. Your text implies this, but it's worth stating, since the two are now separated by a whole stage.
The transports should use the same moment for their protocol-level equivalents. HTTP/2 and gRPC send GOAWAY, so clients stop opening new streams on the connection. HTTP/1 closes idle keep-alive connections. Otherwise clients keep sending requests into connections that will only refuse them.

The full close sequence then reads:

BeforeApplicationShutdown(signal)        [before_shutdown_timeout; traffic flowing]
→ stop accepting; draining() fires; WS 1001; GOAWAY; idle keep-alives closed
→ drain                                  [drain_timeout; terminal executions allowed]
→ cancel + abandon the rest
→ OnModuleDestroy                        [hook_timeout each; reverse order]
→ close sockets
→ OnApplicationShutdown(signal)          [hook_timeout each; reverse order]

Folding these in means two new knobs (before_shutdown_timeout, hook_timeout), one new phase, the Timer binding, and open_terminal. None of them need a separate decision unless you want different defaults. I'd use 10 seconds for both timeouts, matching drain_timeout.

**The user asked:** "if the API is properly made, should each hook get its own individual timeout?"

**The author answered:**

Yes, each hook should get its own timeout. But the design should also say who sets it, and per-hook timeouts alone aren't enough.

Why per-hook is right

The person who knows how long a hook should take is the one who wrote it. Your Discovery hook deliberately sleeps 5 seconds. A cache flush should take milliseconds. A single stage-wide budget forces both to share a number that fits neither. So the timeout belongs where the hook is declared, with an app-wide default for hooks that don't specify one.

For trait hooks, that's an associated const with a default:

rust
impl BeforeApplicationShutdown for Discovery {
    const TIMEOUT: Option<Duration> = Some(Duration::from_secs(10));

    async fn before_application_shutdown(&self, signal: &Signal) { /* ... */ }
}

None means "use the app's hook_timeout". It's the same for all five hook traits, so startup hooks can't hang connect forever either. An init hook that times out becomes StartupError::Hook with a timed-out source.

For closure hooks there's an API detail to get right. Probe P12 showed .ready(..).retries(..).timeout(..).on_destroy(..) chaining on one handle type, so .timeout attached to the readiness check. Once hooks have timeouts too, a bare .timeout() is ambiguous. ready and each on_* should return a small sub-builder whose .timeout() applies to that item and which forwards everything else back to the binding handle:

rust
m.singleton(..)
    .ready(|pool: Dep<PgPool>| ..).retries(5).timeout(secs(10))   // the check's timeout
    .on_destroy(|pool: Dep<PgPool>| ..).timeout(secs(3));         // the hook's timeout
What this simplifies, and what it needs in addition

With per-hook timeouts, the before_shutdown_timeout I proposed last time becomes unnecessary. I'd withdraw it. The stage budget only existed because individual hooks had no way to state their own needs.

But per-hook bounds don't bound the whole shutdown. Hooks run one after another in reverse order, so 20 hooks at 10 seconds each can take more than three minutes. Meanwhile the orchestrator kills the process at its grace period (30 seconds by default in Kubernetes), mid-hook, with nothing reported. So add one outer cap, shutdown_timeout, covering the whole close sequence.

When that cap expires, the remaining hooks are abandoned and recorded. Sockets still close, because that step runs no user code and finishes quickly. Then the report is returned.

I'd leave shutdown_timeout unset by default, since the right value depends on the platform. The documentation should recommend setting it a few seconds under the orchestrator's grace period.

The settled set of knobs is then: hook_timeout (default for every hook, 10 seconds), per-hook overrides via TIMEOUT or the sub-builder, drain_timeout, and an optional shutdown_timeout as the outer cap.

---

## Fifth response: answers to SHUTDOWN_REVIEW.md

Received 2026-10-01. The author saw a summary of `SHUTDOWN_REVIEW.md`, not the file: its four main
points, the smaller fixes, and three of its seven questions. The user signed it off in full the
same day, together with the sixth response.

I checked each point myself rather than taking the review's word for it. It's right on almost everything. In a few places I'd refine the fix, and I can't settle two items without material I don't have.

The four main points
1. open_terminal needs a token. Agreed. A public method is callable by anyone, and transports live in separate crates, so the method must be public. A type with a private field that only the core constructs is the standard way to make the restriction real. One consequence is worth stating: anyone who implements Server receives the token, including users writing their own transport. That's the correct semantics, though. The rule was never "only our crates", it was "only transports".

2. Timer needs methods. Agreed, with one addition. fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> is right. 'static lets callers spawn the future, and an async fn would indeed make the trait unusable as dyn.

I'd also add fn now(&self) -> Instant. Without it, deadlines and timeouts are measured with std::time::Instant while sleeps run on the runtime's clock. In tests with a paused or mocked clock, which tokio supports, sleeps advance while deadlines never do. Then every timeout test either hangs or passes for the wrong reason. One trait, one clock.

3. Abandoning a hook must be defined. Agreed. At the timeout, the core drops the hook's future. The documentation should spell out what follows from that: a hook dropped mid-await can leave its own state half-done, and any task it spawned keeps running, because dropping a future doesn't stop work spawned elsewhere. Note that this differs from executions, which are cancelled and then left running, since transports and subtasks still hold them.

4. Closing WebSockets at drain start isn't graceful. Agreed, with one refinement. Strictly speaking, closing with 1001 doesn't stop in-flight executions. They keep running, but their replies have nowhere to go, so the effect is the same and the conclusion holds.

The proposed fix leaves a gap, though. A busy connection keeps receiving messages during the drain, and each one would be a refused execution. So the full rule is:

Idle connections close with 1001 immediately.
Busy connections stop reading at drain start, finish their in-flight messages, then close with 1001.
Everything is bounded by drain_timeout.
That's the exact equivalent of HTTP/1's "finish this request, then close the connection".

The smaller fixes
One handle type with a state parameter: agreed. It's simpler than forwarding between types, and misuse fails to compile.
Failure reasons in the report: agreed. The reason is panicked, timed out, skipped by the overall cap, or returned an error, where the hook can return one. Destroy and shutdown hooks return (), so for them it can only be one of the first three.
load refused from Stopping onward: agreed. A module loaded during Stopping would miss the before-shutdown stage that's already running, so its shutdown would be incomplete. It returns Closed.
Stale sections in DESIGN.md: I can't check this. The current file isn't in this conversation. Only the probe crate came through. If you paste the document, I'll list every place that still describes the old order or cancel-first.
The questions
Does shutdown_timeout start at the trigger? Yes. Its purpose is to finish before the orchestrator kills the process, and the orchestrator's clock starts at SIGTERM. If the cap started later, it could overrun the grace period, which defeats it. So the before-shutdown stage counts against it. Per-hook timeouts still apply inside that window.

Is is_draining() true during Stopping? No. Draining has a precise meaning: new work is refused and the drain window has started. During Stopping, traffic is normal by design. Making is_draining() true early would push streams to end before the discovery hook has even deregistered, which is exactly the problem you moved that hook to solve. If something needs early notice, BeforeApplicationShutdown is that notice.

The rest got cut off. The message stops at "Does a terminal e". If the question is how a terminal execution sees drain state, the answer is that it's opened during the drain, so is_draining() is already true and draining() resolves immediately. Paste the remaining questions and I'll answer each one.

**The user added the last question:** "Does a terminal execution opened just before the drain
timeout lose its disconnect handler?"

**The author answered:**

Yes, it can lose it, and I think that's the correct outcome, not a gap to close.

Here's what happens. A terminal execution follows the same rules as any execution in the drain. At drain_timeout, its cancellation fires and it's abandoned, meaning it's left running, not dropped. A handler opened just before the timeout gets almost no window. It can keep going after the cancellation, but it then races the destroy hooks. Once Destroying begins, any fresh singleton lookup it makes returns LookupError::Closed, and services it already holds may have been destroyed underneath it. Connections still busy at the timeout are worse off: their terminal execution would open after the drain has ended, so it's refused and the handler never runs.

I wouldn't add a reserved grace window after the timeout to save these handlers, for two reasons.

First, shutdown cleanup can never be the thing correctness depends on. The same disconnect handler also never runs on SIGKILL, an OOM kill, a crash, or a lost node, and those happen in production regardless of how good the drain is. So state that outlives the process, like presence in Redis or room membership in a shared store, has to clean itself up anyway, through TTLs or heartbeats. State that dies with the process doesn't need cleanup at all. Either way, a handler running at shutdown is an optimization.

Second, a grace window only moves the edge. Any handler opened just before the end of the grace window has the same problem, and the app gains another knob.

What the design should do instead:

Document the guarantee plainly. Disconnect handlers at shutdown are best effort. Anything that must survive a crash belongs in TTL- or heartbeat-based state.
Make losses visible. Count terminal executions that were refused or abandoned in the shutdown report, for example Shutdown { signal, abandoned, terminal_skipped }. The transport should log which connections missed their handler.
Reduce the window where possible. Under the rule from last time, idle connections close at drain start, so most disconnect handlers start with the full window. Only connections still busy near the timeout are at risk, and those were going to be cut anyway.
If you disagree and want a guaranteed window, the fallback is a short terminal_grace after drain_timeout during which only terminal executions may run. But I'd wait to see a real case before adding it.

---

## Sixth response: bounds without a Timer, and the four remaining questions

Received 2026-10-01, answering what the fifth response had not seen: the review's refinement on
bounds without a `Timer` and its questions 1, 4, 5 and 7. The user signed it off in full the same
day, together with the fifth response.

### The refinement: one rule for bounds without a Timer

I agree with the rule: an explicit bound needs a Timer and is a wiring error without one, the app defaults apply only when a Timer exists, and a Timer stays optional otherwise. That's simpler and more honest than what I said.

One existing case should stay deliberately asymmetric. **The drain without a Timer is zero-length, not unbounded.** That was the earlier decision: a timerless close abandons live executions at once. If the drain had no bound, a single slow client could hold shutdown open forever. Hooks without a bound run unbounded because they're the application's own code. Executions depend on clients the app doesn't control. That difference should be stated next to the rule.

**How a hook declares "no bound":** not `Some(Duration::MAX)`. Besides reading as a trick, it's a trap. `Instant + Duration::MAX` overflows, and `Instant`'s `Add` panics on overflow. So every place that computes a deadline would have to remember `checked_add`, and one that forgets turns "unbounded" into a panic in the shutdown path. Three states don't fit `Option` anyway, so give them their own type:

```rust
pub enum Bound { Default, After(Duration), Unbounded }

impl OnModuleDestroy for Indexer {
    const TIMEOUT: Bound = Bound::Unbounded;
    async fn on_module_destroy(&self) { /* ... */ }
}
```

`Unbounded` needs no Timer, so it never triggers the wiring error. The outer `shutdown_timeout` cap still applies to it. Unbounded per hook doesn't mean unbounded overall.

**Readiness `.timeout`: per attempt, or across retries?** Make `.timeout` bound the whole check, retries and backoff included, so it means the same thing as a hook's `.timeout`, which bounds the whole run. Add `.attempt_timeout(d)` for the per-attempt bound. Both are optional, and both need a Timer. The total is what startup budgeting cares about, and the attempt bound is what catches a single hung ping. With two names, nobody has to guess which one they're reading.

### The four questions

**1. One timer or two?** One. The app's timer is the `Timer` binding, so overriding it would replace the internal timer as well. But I wouldn't let a test override it as an ordinary binding. A timer is app configuration, and it drives the drain, the hooks and the readiness checks together. So `override_value::<dyn Timer>` should be a wiring error with the hint "set it with `TestApp::timer(..)`".

That keeps one source of truth, and a test that wants a mocked clock sets it in the one place that clearly affects everything. The same applies to any module that tries to bind `dyn Timer` itself. The core's binding is a global export, so a second one is already reported as ambiguous by the existing rules.

**2. `draining()` for tasks without an execution.** Yes, `AppHandle` should have `draining()` and `is_draining()` too, resolving at the same moment as on executions. Background loops such as queue consumers and pollers should stop pulling new work exactly when transports stop accepting, and getting `Closed` from `execute` is far too late as a first notice.

The documentation should also point out that the drain doesn't track detached tasks, only executions. Background work that must finish cleanly should run each unit of work through `execute`, so the drain waits for it.

**3. Unbounded constructors and factories.** `connect_lazy` plus a readiness check is a good idiom, but not every driver offers a lazy connect, so it can't be the only answer. A factory awaiting a network connect hangs `connect` just as surely as a hook would. Constructors should get the same bound as hooks:

- **Factories:** the binding itself is the first item on the handle, so `.timeout(..)` directly after `singleton(..)` bounds construction. This also refines the review's typestate rule: there's never "no item before `.timeout()`", because the binding is always there.
- **`#[injectable]` types:** `const CONSTRUCT_TIMEOUT: Bound` on `Construct`, set from the macro as `#[injectable(timeout = Duration::from_secs(5))]`.
- **The app default:** a separate `construct_timeout`, because connecting legitimately takes longer than a hook. I'd default it to 30 seconds. Like the other defaults, it applies only when a Timer exists.

The same bound applies when execution-scoped bindings are built during a call, since a hanging constructor there blocks the call just the same.

**4. The ambiguous `TIMEOUT`.** This is smaller than it looks. *Defining* the const is never ambiguous: inside `impl OnModuleInit for X`, `const TIMEOUT` clearly belongs to `OnModuleInit`. Only *reading* `Self::TIMEOUT` is ambiguous, and the core always reads it fully qualified in generated and internal code. Users almost never need to read it. One line in §9.1 is enough: "to read a hook's bound when a type implements several hook traits, write `<T as OnModuleInit>::TIMEOUT`." Naming the constructor's const `CONSTRUCT_TIMEOUT` rather than `TIMEOUT` keeps it out of this entirely, which is why I named it that way above.

---

## Seventh response: `listen()` without a Timer, and the eleven details of the shutdown fold

Received 2026-10-01. The user signed it off in full the same day.

### The question: keep `listen()`'s refusal

Keep it. My wording was loose. "A single slow client could hold shutdown open" described what would happen *if* a served app had no Timer, and the design rules that case out. Your version, "work the application does not control", is the accurate statement of the asymmetry.

Accepting a timerless `listen()` with a zero-length drain would be worse on two counts:
- Every shutdown would cut every in-flight request, silently. That makes graceful shutdown impossible for anyone who forgot `.timer(..)`, and it isn't detectable until production.
- Per-call deadlines need a Timer to enforce. A served app without one would accept deadlines it can't keep.

So refusing at `listen()` is the right place to catch it. No change to §9.5, §12, or the defaults rule.

### The eleven details

1. **Confirmed, with one addition.** `TimedOut { after: Duration }`. An error saying a hook timed out without saying after how long sends people into the code to find out. Carrying the bound also shows whether the limit was the per-hook bound, the default, or the outer cap. Panic payloads converted to a message inside `BoxError` is fine.

2. **Confirmed.**

3. **Confirmed.**

4. **Correction.** A readiness check with no bound can hang on its *first* attempt. A ping to a host that silently drops packets never returns, so the retries are never reached. And external I/O is exactly what readiness checks exist for. When a Timer exists, a check with neither `.timeout` nor `.attempt_timeout` should get `construct_timeout` as its attempt bound. Without a Timer, it runs unbounded under the same rule as constructors.

5. **Confirmed.** That's the explicit-bound rule applied consistently.

6. **Confirmed**, for both `ExecutionRef` and `AppHandle`.

7. **Correction: name the type.** `Closed` must fit inside whatever `load` returns, and `load` already has other failures: wiring errors, startup failures, refusals of controllers or late contributions. Give it its own enum:

   ```rust
   pub enum LoadError {
       Closed(Closed),
       Wiring(WiringErrors),
       Startup(StartupError),
       Refused(LoadRefusal),   // controllers, middleware, late contributions, global exports
   }
   ```

   Leaving it unnamed invites someone to cram `Closed` into `StartupError` later, where it doesn't belong.

8. **Confirmed.**

9. **Confirmed**, assuming the steps are: 1 before-shutdown, 2 stop accepting, 3 drain, 4 cancel and abandon, 5 destroy, 6 close sockets, 7 shutdown. If the document numbers them differently, the mapping follows the meaning: lookups are refused from the first destroy hook onward.

10. **Confirmed, with one practical note.** Transports open terminal executions from connection tasks that they spawned earlier, so the token can't be a short-lived borrow. Make `DrainToken` cheaply `Clone` (an `Arc` inside) while keeping it impossible to construct outside the core. Cloning is harmless: `open_terminal` still checks the phase, so a token used after the drain has ended is refused like anything else.

11. **Confirmed.** Mention one path explicitly: the drain can end early because the `shutdown_timeout` cap expires, and then "end of the drain" fires at that moment rather than at `drain_timeout`. It's not a fourth source, but readers will wonder.

---

## Eighth response: the seven details of the seventh fold

Received 2026-10-01. The user sent it to be folded in the same day.

Four of these are fine as written. Three need corrections, and two of those fix problems in what I wrote last time.

1. **Correction: say which limit fired.** Skipped meaning "never started" is right. But `TimedOut { after: shutdown_timeout }` on its own is ambiguous whenever a hook's own bound and the cap have similar values, which is common, since both tend to be round numbers. That ambiguity is exactly what the `after` field was meant to remove. Add the source of the limit:

   ```rust
   TimedOut { after: Duration, limit: Limit }
   pub enum Limit { Item, Default, ShutdownCap }   // explicit bound, app default, outer cap
   ```

   `after` stays the configured duration of whichever limit fired.

2. **Mostly confirmed, with one change.** Writing either bound as the opt-out from the default is right. But I'd give readiness `.unbounded()` too. "Wait for the database however long it takes" is a legitimate startup pattern, and without `.unbounded()` the only way to express it with a Timer present is a huge retry count, which is a workaround. More importantly, every bounded item would then accept the same three states (default, explicit, unbounded). Users learn one vocabulary instead of remembering that readiness is the exception.

3. **Correction: mark them `#[non_exhaustive]`, and StartupError too.** Matching StartupError makes sense for consistency, but the consistency should run the other way. These are public error enums in a framework that will grow. Without `#[non_exhaustive]`, adding any variant later is a breaking change for every user who matches exhaustively. `LookupError` was already marked, so mark all of them.

   The variant list is also missing one case. A lazy module that declares an execution input should be refused, because inputs belong to transports and their per-handler check ran at wiring time. Add `Input { module, key }`.

4. **Correction, to my own earlier suggestion.** As I sketched `LoadError`, `LoadError::Startup(StartupError::Wiring(..))` can be constructed but never happens, because load reports wiring errors through `LoadError::Wiring`. `StartupError::Bind` is unreachable from load as well, since lazy modules bind no transports. The fix is to split the connect-phase failures into their own type and compose the two errors from exact parts:

   ```rust
   pub enum ConnectError { Construct { .. }, Readiness { .. }, Hook { .. } }

   pub enum StartupError { Wiring(WiringErrors), Connect(ConnectError), Bind { .. } }
   pub enum LoadError    { Closed(Closed), Wiring(WiringErrors), Connect(ConnectError), Refused(LoadRefusal) }
   ```

   Every variant combination is then reachable, and code that handles construction failures handles them once, for startup and load alike.

5. **Confirmed.**

6. **Confirmed.**

7. **Confirmed.** While you're in §13, check that `DrainToken` and `Bound` are listed there too. Transports name the first, and every integration that declares a bound names the second.

---

## Ninth response: readiness failures, redaction, and the seven details of the eighth fold

Received 2026-10-01. The user signed it off in full the same day.

### The gap: Readiness should carry `FailureReason`

Readiness should carry `reason: FailureReason`, the same as every other connect failure. A separate `Limit` type for readiness would split one concept in two. Code that reports or handles timeouts would need two code paths, and users would have to learn which errors use which type.

`Limit` does need to distinguish the whole-check bound from the per-attempt bound, so add one variant:

```rust
#[non_exhaustive]
pub enum Limit {
    Item,         // an explicit bound on the whole item: Bound::After, .timeout(..)
    Attempt,      // an explicit .attempt_timeout(..)
    Default,      // the app default that applies to this item
    ShutdownCap,  // the outer shutdown_timeout
}
```

`Default` stays a single variant with one stated meaning: "the app default for this kind of item". For readiness, that default is per attempt (`construct_timeout`, from detail 4 last time). For hooks, it covers the whole run. `after` makes the actual duration explicit.

Readiness keeps `attempts`, and its `reason` describes how the check finally ended:
- If the whole-check bound fired, it's `TimedOut { limit: Item }`.
- If the last attempt hit its bound when no retries were left, it's `TimedOut { limit: Attempt }` or `TimedOut { limit: Default }`.
- If the retries ran out on errors, it's `Errored(..)` holding the last error.

This also exposes a second gap: **redaction no longer has anything to apply to.** `source: Redacted` was what guaranteed redaction [42]. A `FailureReason` holds plain `BoxError`s, so `Errored` and `Panicked` would carry the raw text. Panic messages can contain a URL too.

And the problem is wider than readiness. A factory whose `connect` fails usually reports an error that contains the connection string, so `ConnectError::Construct` leaks the same credentials that readiness was carefully protecting. So run the redaction pass on every `BoxError` before it enters any `ConnectError`, `ShutdownFailure` or `LookupError::Construct`, with the redacted text boxed in its place. The type then guarantees nothing by itself, but every path that builds these errors goes through one function, and that's easy to audit.

### The seven details

1. **Confirmed for the enums. Correction for the structs.** Public structs with public fields need `#[non_exhaustive]` just as much. Adding a field to `Shutdown`, `ShutdownError` or `GuardRejected` later breaks every user who destructures them without `..`. Users can also construct `Closed` if it's a public unit struct, which freezes its shape. Mark all four. Then users can only read them, which is how they're meant to be used.

2. **Correction: this can be a compile error.** Handle items already carry a typestate parameter. Once `.timeout(..)` or `.unbounded()` has been called, the item can move to a "bounded" state that has neither method. That makes conflicting writes, and calling `.timeout` twice, impossible to compile, which follows the brief's "compile time where feasible" [45]. Trait constants can't conflict, because each item has only one constant. So the wiring-error rule isn't needed at all.

3. **Confirmed.** Under the typestate, `.unbounded()` on a readiness check also excludes `.timeout` and `.attempt_timeout`.

4. **Confirmed.**

5. **Confirmed.** `Limit::Attempt` from above covers the remaining explicit spelling.

6. **Confirmed.**

7. **Understood.** The `reason` field replaces `source` on Readiness, so all three `ConnectError` variants report failures the same way.

---

## Tenth response: redaction scope by origin, and the `Redacted` type kept

Received 2026-10-01. The user signed it off in full the same day.

### The two redaction questions

**1. `try_value` errors: yes, and look closely at why.** Config loaders are the most likely place for a credential to appear in an error message ("failed to parse DATABASE_URL=postgres://admin:hunter2@..."). But the registry-based part of redaction can't help on this path. The secret that would have been registered is inside the config that failed to load, so it was never registered. Here the URL-userinfo strip is the only protection. That's still worth having, and the documentation should be honest that it's a backstop on this path, not a guarantee.

**2. `Bind`: name it in.** Your question shows the real problem: my list of three types was the wrong way to define the scope. A list has to be extended each time a new error variant appears, and every missed case is a silent leak, which is how both of these questions came up. Define the scope by where the error comes from instead:

> Every error the core did not create itself, from user code, integrations or transports, passes through the redaction function before it is stored in any core error type.

That covers `WiringErrors`, `Bind`, `ShutdownFailure::Close`, and anything added later, with no list to maintain. Bind errors rarely contain credentials, but a TLS key path or a proxy URL with a password in it is possible, and the cost is one function call on a path that fails once.

### The ten details

1. **Confirmed.**

2. **Confirmed.**

3. **Confirmed, with the rule written out.** Not making these once-only is a reasonable way to keep the typestate small. A repeated `.retries` is redundant, while a repeated bound conflicts with itself. But the document should state "the last write wins", so it's a rule rather than an accident of implementation.

4. **Confirmed.** A panic signals a bug, not a temporary fault, so retrying it only repeats the bug.

5. **Confirmed.**

6. and 10. **Correction to both, together.** Replacing an error with its redacted text loses something important on the runtime path. `LookupError::Construct` can fire during a call. An execution-scoped constructor that fails with a domain error, such as "tenant not found", needs its error handler to downcast that error and map it to a 404. With the original error discarded, the handler only sees text, and that mapping becomes impossible.

So keep a type, but one that enforces what it claims:

```rust
pub struct Redacted { inner: BoxError, text: String }

impl Display for Redacted { /* writes `text` */ }
impl Debug   for Redacted { /* writes `text`; never formats `inner` */ }
impl Error   for Redacted { /* source() returns None */ }

impl Redacted {
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E>;   // reaches the original
    pub fn into_inner(self) -> BoxError;
}
```

The objection in detail 6 was that a marker type would claim something it couldn't enforce. This type does enforce something: its formatting never prints the original. Logging, `{:?}` and error reports all go through `Display` or `Debug`, so they're safe. Programmatic access to the original error stays possible, but only through an explicit method that a reviewer can find.

`source()` returns `None` on purpose. Otherwise error-chain reporters, which walk `source()` and print every link, would print the original error and bypass the redaction.

7. **Correction**, following from question 2. Narrow the sentence to the origin rule, not to three types.

8. **Confirmed.**

9. **Confirmed.**

---

## Eleventh response: the five details of the tenth fold

Received 2026-10-01. The user signed it off in full the same day and asked for the closing
consistency read it proposes.

The probe is the evidence that matters: the chain assertion fails when `source()` returns the inner error, so the test catches exactly what it claims to. Four details are confirmed and one gets a small addition.

1. **Confirmed.** Private fields already rule out construction and destructuring outside the core, so `#[non_exhaustive]` would add nothing.

2. **Confirmed, with one addition.** Keeping the field type uniform is right, and wrapping the core's own error is consistent with the origin rule. But make the wrapped error a typed public struct, such as `NoTimer { transport }`, rather than a formatted message. Then `downcast_ref::<NoTimer>()` lets a caller tell "this app was misconfigured" apart from "the port was taken", which is the same kind of branching `Redacted` exists to keep possible. A plain string would make the core's own error the one thing in that field that can't be told apart from the rest.

3. **Confirmed.** That split puts each statement where a reader would look for it: the guarantee on the type, the reasoning next to it, and the scope with the rest of the redaction rules.

4. **Confirmed.**

5. **Confirmed.** The table lists refusals, and redaction refuses nothing. Stating the scope in three places that agree with each other is enough.

With no open questions and these folded in, the shutdown, timeout and redaction work is complete. If you want a final pass, the next thing worth doing is reading the whole 1,161 lines once from start to finish, looking only for statements that two rounds of edits might have left saying slightly different things. Several pieces were revised more than once: the close order, `Limit`, `LoadError`, and the redaction scope. Those are where stale wording tends to survive.

---

## Twelfth response: the closing read, and `ConstructError` defined

Received 2026-10-01, answering the closing consistency read and the one name it found without a
shape. The user brought it to be folded in the same day.

The six fixes are all correct, and all six are the kind of drift that builds up when sections are revised separately. I have nothing to add to them.

Here's the definition for ConstructError. It's slightly more than one line, because one choice in it matters:

rust
#[non_exhaustive]
pub enum ConstructError {
    Site(LookupError),   // reading a dependency failed; propagated as the dependency's own error
    Failed(BoxError),    // the constructor's own error; redacted by the core when stored
}

impl From<LookupError> for ConstructError { .. }
impl ConstructError { pub fn failed(e: impl Into<BoxError>) -> Self { .. } }
Why two variants: when a constructor fails because a site read failed (an input that wasn't seeded, ExecutionRequired, a nested construction failure), the user needs the error about the dependency, naming the deeper key. Wrapping it as "constructor X failed" would point them at the wrong place. So the core passes Site errors through unchanged, and turns only Failed into Construct { reason: Errored(Redacted) }. That's also why the origin rule applies only to Failed: a LookupError is the core's own, and any outside error inside it was already redacted where it was stored.

Why there's no blanket From<E: Error>: LookupError itself implements Error, so a blanket impl would overlap From<LookupError>. That's the same E0119 coherence conflict as probe P02a. So ? works directly on site reads, and constructor errors go through ConstructError::failed. The macro does that mapping for #[construct] functions returning Result<Self, E>. People writing Construct by hand use .map_err(ConstructError::failed)?. That line belongs in §13's integration example.

Panics and timeouts aren't ConstructError variants. The core observes them from outside the constructor and records them directly as FailureReason::Panicked and FailureReason::TimedOut. A constructor can't report its own timeout, so the type shouldn't suggest it can.

With that in place, every named type in the design has a shape.

---

## Thirteenth response: the rebuild's divergences, and naming the `Site` family

Received 2026-10-01, answering `DIVERGENCES.md`. The user signed it off in full the same day: the
rename, the positions on D1–D20, the three added decisions and the `#[routes]` documentation line.

### Naming the `Site` family

You're right to raise it, and now is the time. Nothing has compiled yet, so a rename costs one search-and-replace. After the first compile it costs every integration crate.

Here's what's wrong with the current names. The design's own convention says types are nouns and traits are capabilities. `Site` is a noun used as a trait, and "site" isn't established DI vocabulary, so users have to learn it. `Sites` and `SiteDesc` inherit the same problem, and `s.site::<Dep<PgPool>>()` says nothing about what the call does.

What I'd use:

| Now | Proposed | Why |
|---|---|---|
| `trait Site` | `trait FromContainer` | It reads as a capability ("can be obtained from the container") and follows axum's `FromRequestParts` and `FromRef`, which Rust web developers already know |
| `Construct::sites(s: &mut Sites)` | `Construct::dependencies(d: &mut Dependencies)` | It says what's being declared |
| `Sites::{field, param, site}` | `Dependencies::{field, param, add}` | `field` and `param` add a name for diagnostics. The unnamed form simply adds one |
| `SiteDesc` | `Requirement` | `Requirement::dep(key)`, `Requirement::optional::<S>()` read as what a read needs |

The compile error then reads: "`PgPool` cannot be obtained from the container. Write `Dep<PgPool>`". The `on_unimplemented` text carries the meaning, so nobody concludes they should implement `FromContainer` for `PgPool` themselves.

Two things stay: `Dep`, `Many` and `Ext`, which are fine nouns, and the prose term "injection point" in the docs. If you rename, D17 disappears.

### The 20 decisions

I agree with the recommendation on 15 of them: D1, D2, D3, D4, D8, D9, D10, D11, D12, D14, D15, D18, D19 and D20, plus D6 as a deferral. Here are the other five.

**D5: agree with the goal, not the form.** Doubling every override method (`override_value_qualified`, `override_factory_qualified`, `override_try_factory_qualified`, and `override_many` will want one too) multiplies the API for one axis of variation. `TestApp<Pending>` already exists precisely to scope the most recent override, so put the qualifier there:

```rust
.override_value::<PgPool>(fake).qualified::<Replica>().in_module::<DbModule>()
```

That's one method instead of four. It reads in the same order as the binding it replaces, and the typestate already stops it attaching to the wrong override.

**D7: don't accept it silently. Refuse it until it's fixed.** The design promises "built once, shared", and the built behavior breaks that in a way nobody will notice. A rate limiter declared on the controller limits each handler separately, and the app works, just more permissively than written. Waiting for the transport protocol is reasonable. Shipping a silent semantic difference meanwhile isn't. Make controller-level `value = expr` a compile error, with the hint "declare it per method, or bind it by type for shared state", until the protocol can pass impl-level values down. A refusal that later relaxes into a feature breaks nobody.

**D13: accept for the first compile, but flag it as must-fix before release.** "Resources release as the instances drop" covers pools and file handles. It doesn't cover what init hooks did outside the process: registering with service discovery, taking a distributed lock, starting a lease. A failed startup that leaves the service registered as healthy is the kind of bug that pages someone at night. Filing it as "needs its own design" is right. The priority should be stated as higher than "revisit later" suggests.

**D16: accept, but record why it differs from D8.** Both are a written wait that doesn't wait, and D8 refuses where D16 accepts. The consistent rule is "refuse when the fault is visible at `wire()`". `.backoff` is visible there, but a deadline is a runtime value passed to `execute`, so refusing it would mean widening `execute`'s error type for one rare case. Write that rule down in the document, or the next reviewer will flag the inconsistency again.

**D17:** replaced by the rename above.

### Three items in the reading sections that deserve decisions

These are listed as "for reading", but I'd treat each as a decision.

**Role detection by `type_name` prefix [C 6].** The `type_name` format is explicitly documented as unstable. A compiler upgrade could silently change which contributions count as global guards. As written, the failure is loud, a wiring refusal, but it would surface in an app that worked yesterday, after a toolchain bump. The robust fix is a dedicated, typed entry point for role contributions, such as `m.enhancer::<AnyGuard<Http>>().provide::<AuthGuard>(..)`, bounded by a sealed `Role` trait that the role key types implement. That makes detection compile-time and independent of the compiler's string format.

**Short type names in diagnostics [A 1].** Cutting `a::Config` and `b::Config` to `Config` produces diagnostics that hide information exactly when two same-named types are involved, such as "missing `Config`" while a different `Config` is bound right there. Keep the short form as the default, but when one report would print two different keys alike, print full paths for those keys. It's a small check at formatting time.

**Transport `close` runs unbounded [E 12].** My earlier reasoning that closing sockets "runs no user code and finishes quickly" was too optimistic. A transport's `close` can wait on a TLS close-notify or a broker acknowledgment. As built, it runs after the `shutdown_timeout` cap with no bound, so one stuck broker hangs shutdown forever. That's exactly what the cap exists to prevent. Bound each transport's `close` by whatever is left of the cap. When there's no cap, use `hook_timeout`, and record a timeout as `ShutdownFailure::Close`.

The rest of sections 2 to 5 reads consistently with the design. One papercut worth a line in the docs: in a `#[routes]` impl, a helper method annotated with `#[tracing::instrument]` is treated as a handler [G 8]. Moving helpers to a separate `impl` block is a fine rule, but people will hit it in their first week, so it belongs in the `#[routes]` documentation.

---

## Fourteenth response: wave 2's divergences

Received 2026-10-03, answering `DIVERGENCES.md`'s Wave 2 section. The user signed it off in full the
same day.

Wave 2 tracks the signed decisions closely, and the rename came out well: none of the new texts suggest implementing the trait, which was the risk. I agree with five of the eight new decisions. Of the other three, D22 and D23 have a better fix available than the options listed, and D21 needs one widening.

### The eight decisions

**D21: (b), but for every `into` list, not only role keys.** Reusing the `#[guards]` grammar is right. `value = ..` is safe at module level, because a module registers once, so the per-handler problem that made D7 a compile error can't occur here. But the description scopes it to enhancers, and `into dyn Plugin: [value = MetricsPlugin::new(cfg)]` hits the same gap. Give every `into` list one grammar.

**D22: refuse it at compile time, without giving up the shared builder.** Option (b) is described as costing the shared builder, but it doesn't have to. `Contribute` can take a marker parameter, as in `Contribute<'m, U, Q, Mark = Plain>`, with `qualified` implemented only for `Plain`. `enhancer` returns the `Enhancer`-marked builder. Every other method stays shared through `impl<.., M> Contribute<.., M>`. That's the same typestate device the handles and `TestApp` already use. It turns a wiring error into E0599 at the call, which is what [45] asks for when it's feasible, and here it costs one type parameter.

**D23: none of the three options. Decide roles by `TypeId` at freeze.** The real problem isn't the macro. It's that `contribute::<AnyGuard<Http>>()` and `enhancer::<AnyGuard<Http>>()` register the same key and behave differently, and the macro is left guessing which one the user meant from tokens. But the core can know exactly. `Mount::handler::<T, H>` is generic over the transport `T`, so when a handler is mounted, it can record `TypeId::of::<AnyGuard<T>>()`, `AnyInterceptor<T>` and `AnyErrorHandler<T>`. At freeze, any contribution whose key's `TypeId` is in that set gets the enhancer role, however it was registered.

That handles aliases, `macro_rules` wrappers and the value API identically, with no token inspection and no `type_name`. `into K: [..]` can then always lower to `contribute`, which makes D23's two limits disappear. Keep `m.enhancer()` for the one case the `TypeId` set can't see: a role key for a transport with no mounted handler, where marking it explicitly avoids a spurious scope refusal. With that, the `Role` bound on `enhancer` still catches a type that isn't a role key.

**D24: agree.**

**D25: agree.** Name both calls, the way `DuplicateReadiness` does.

**D26: agree.**

**D27: agree.** It's the right model: a handler's panic is an error like any other error from the inside, and interceptors that log or time calls should see it. One sentence belongs in `Next::run`'s docs: state touched by the panicking stage may be inconsistent, so an interceptor should treat the `Err` as fatal for that call rather than retry.

**D28: (b), agree.** A match arm instead of a downcast, and the same shape as `Hook`.

### Four things in the reading sections

**Two shapes for "a panic during a call".** A panic while the container builds an enhancer or controller arrives as `LookupError::Construct { reason: Panicked }`, and one inside its code as `PanicRecovered` [R 3]. Each is defensible on its own. But the common error handler, "any panic → 500 and an alert", now has to match two unrelated types, and the first one is nested. Add a small public helper, such as `ulo::is_panic(&BoxError) -> bool`, or a method on the error-handler context, so that handler is one line and doesn't depend on knowing the distinction.

**Module names still collide.** The full-paths-on-collision rule covers `KeyName` only [W 9]. Two module types sharing a last path segment, such as `billing::Module` and `users::Module`, still print alike. That's exactly the confusion the rule fixes for keys, and module names appear in the errors where it matters most (`Ambiguous`, `ExportNotBound`). The same grouping pass over `ModuleName` fields is cheap. The rendered strings like `consumer` and path steps can stay short, as the limit says.

**Section 3's role rules change if you take the D23 fix.** "A role is decided at freeze", and the provider-to-enhancer move for a binding named by type, still hold. The two limits of token-level recognition, and the document's note that `contribute` under a role key is a provider "since a negative bound cannot refuse it", become obsolete.

**`enhancer()` returning a builder that has `qualified`** is D22. If D22 is decided as above, the second-section entry about `enhancer` returning the same builder as `contribute` changes to "the same builder, marked".

Everything else in sections 2 to 6 reads consistently with the design and the earlier rounds. That includes the close bound shrinking with the cap's remainder, the closure scope check exempting enhancer closures, and `.backoff` being refused even with zero retries.

---

## Fifteenth response: what decides the scope of `with = closure`

Received 2026-10-03. The user asked whether `with = closure` building a singleton in an `into` list
but per execution in `#[guards(..)]` is the better way: "is it relaly the better way? that differnce? i was htining scope that is if it is singltonor others is decided elsewhere and som how elsE?" The user signed it off the same day; the `with(execution) = ..` form waits
until someone needs it.

You're right to question it. As built, with = closure means two different things depending on where it's written: a singleton in an into list, built per call in #[guards(..)]. That's a scope decided by spelling position. Nothing in the design says position should decide scope, and a reader moving a guard from a method to a global into list would silently change when it's built.

The brief already says what decides it. Item [26]: an enhancer "declared by closure" is "built per execution". That's a rule about enhancers, not about method attributes. So the consistent rule follows the role:

Declared by closure as	Scope
An enhancer, at any tier (method, controller, global)	per execution, as [26] says
A provider contribution (into dyn Plugin: [with = ..])	the provider rule: a singleton, refused at wiring if it reads execution data
Under that rule, the two cases you saw only differ when they are different roles. A global guard written into AnyGuard<Http>: [with = |u: Ext<CurrentUser>| ..] works exactly as it would on a method. A plugin closure that reads execution data is refused, for the same reason any singleton provider that needs an execution is refused.

How this fits the D23 fix: under the TypeId approach, the macro no longer knows the role at expansion, and it shouldn't have to. The macro lowers with = closure to one value-API method, say Contribute::with(factory, coerce), whose recipe records "declared by closure" rather than a scope. At freeze, once the role is known, the core resolves it: per execution for enhancers, singleton for providers. One spelling, one meaning per role, decided in the one place that knows the role.

Two follow-ups:

A provider collection that genuinely wants a per-execution closure can still say so explicitly. The value API already has .execution(factory, coerce). If you want a macro form, with(execution) = .. is the natural extension of the grammar. I'd only add it when someone needs it.
value = expr is already consistent across positions: a value is built once and shared everywhere it's declared. That's why D7's controller-level case became a compile error rather than a different meaning. So with is the only spelling that needed this fix.
DESIGN.md §4 should then drop the sentence describing the difference, and state the role rule once, in §7, where enhancer scopes are already explained.

---

## Sixteenth response: scope as its own axis

Received 2026-10-03, revising the fifteenth. The user replied to it: "ok, i was just thningit should all mean same thing with is a clousre then scopign should be somehow else" The user signed it off the same day,
with all three explicit scopes (`with(singleton | execution | transient) = ..`, mirroring
`#[injectable(..)]`) and item [26] of the brief amended.

Yes, and that's cleaner than the role table I just gave you. It splits two questions that the current design mixes together:

How is it built? By a type (Type), from a value (value = expr), or by a closure (with = closure). That's all with should say.
How long does it live? Scope. That should be one separate axis, with one default rule and one way to override it, the same everywhere: on types, on closures, in method attributes, in into lists, in the value API.
The design already has a scope axis for types: #[injectable] means Auto, and #[injectable(execution)] overrides it. Closures can follow the same pattern:

rust
#[guards(with = |u: Ext<CurrentUser>| RoleGuard::admin(u))]             // Auto
#[guards(with(execution) = || RequestTimer::start())]                    // explicit
into dyn Plugin: [with = |cfg: Dep<Config>| MetricsPlugin::new(cfg)]     // Auto
Auto keeps the meaning it already has. Built once if nothing it reads needs an execution, per call if something does. For a provider, it's a singleton, refused if it needs execution data. So the first guard above becomes per call because it reads Ext, and the plugin is a singleton. The same closure means the same thing wherever you write it.

Two consequences to accept knowingly:

It changes the brief. Item [26] says an enhancer declared by closure is "built per execution". Under this rule, a closure guard that reads nothing per call is built once. That's usually what you want, and cheaper too. But it's a change to your requirement, so amend [26] rather than leave the document silently disagreeing with it.
Fresh state needs the explicit form. Inference looks at what the closure reads, not at what it does. A closure that reads nothing but must create something new per call, such as || RequestTimer::start() capturing the current time, would be inferred as a singleton and built once. That's a bug that's hard to see. So the docs should say plainly: if the closure creates per-call state, write with(execution). That's the one case where intent can't be inferred, and it's the case the explicit form exists for.
In the value API, it maps the same way. Contribute::with(factory, coerce) and EnhancerSpec::guard_with(factory) register Auto, and the existing explicit methods (.execution(..), .transient(..)) cover the overrides. So the macro grammar and the value API stay aligned, with no position-dependent meaning anywhere.
