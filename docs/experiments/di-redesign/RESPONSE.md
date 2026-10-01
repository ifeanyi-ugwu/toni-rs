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
