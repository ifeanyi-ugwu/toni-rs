# Review of the fourth response: shutdown

For the user and the author. Scope: `RESPONSE.md` lines 202–290 against `DESIGN.md` §3.5, §3.8, §6.3,
§8.6, §9, §10.2, §12 and §13. Each claim is marked Probed (`probes/src/bin/p15_*` onward, on 1.88 and
1.98), Read (the two documents) or Asserted (reasoning not backed by code).

## Holds

- Stopping as a phase in which executions and singleton lookups stay allowed. Read: the table at
  215–219 follows from the hook's move, and nothing in `DESIGN.md` contradicts it.
- `Dep<dyn Timer>` is constructible once `Timer` is dyn-compatible. Probed, P15.
- `const TIMEOUT: Option<Duration>` with a default beside an RPITIT method, read by the generated
  `Hooks<T>` registration; two hook traits on one type keep separate consts. Probed, P16.
- `before_shutdown_timeout` withdrawn for per-hook bounds plus an outer cap. Read, 282–288; the cap's
  expiry semantics are item 5 below.
- `is_draining()` beside `draining()`, both at the start of Draining, the moment GOAWAY and the idle
  keep-alive close share. Read.
- A terminal execution counted in the drain and refused from Destroying on. Probed, P18, for the
  phase check.
- An init hook's timeout as `StartupError::Hook`, a shutdown hook's as `ShutdownFailure::Hook`. Read:
  §10.2 has both variants with a `source: BoxError`. The variants' granularity is item 6.

## Refine

1. **Define `Timer` before binding it.** `DESIGN.md` names `Timer` at lines 12, 712, 725, 762 and 971
   and gives it no method. Probed, P15/P15b: `fn sleep(&self, Duration) -> BoxFuture<'static, ()>` on
   `Timer: Send + Sync + 'static` is dyn-compatible and is the whole trait the core needs. `timeout(d,
   fut)` is a runtime-free select over `sleep`, and `sleep_until` is `sleep(deadline − now)`. Written
   `-> impl Future`, `sleep` is E0038 and no `dyn Timer` exists. For the binding: the key is `dyn Timer`
   and `TypeId::of::<dyn Timer>() != TypeId::of::<dyn Timer + Send + Sync>()` (P15 prints `false`), so
   a site spelled with the redundant bounds misses and §10.1 step 3's two-spelling report applies. The
   binding is a value in the core's own global module, the §6.4 `fw-http` pattern, or no module sees it.

2. **The sub-builder is one type with a state parameter, not a struct per item.** Probed, P17:
   `Handle<T, K = NoItem>`, `K` naming what the last call wrote (`ReadyItem`, `HookItem`). `.timeout`
   is defined for `K: HasTimeout`, `.retries`/`.backoff` for `K = ReadyItem`, every other method once on
   `Handle<T, K>` moving to the next state, and a method writing no timed item (`also_as`, `qualified`)
   returns `NoItem`. The response's chain compiles as written and each timeout applies to its item. The
   refusals: `.timeout` with no item is E0599 naming `NoItem: HasTimeout` (P17b); `.retries` after
   `.on_destroy` is E0599 with "the method was found for `Handle<T, ReadyItem>`" (P17c). An
   `on_unimplemented` message does not reach an E0599 note, so the hint has to be the trait's name.
   The module hooks `m.on_init(..)`/`m.on_destroy(..)` (§9.1 line 689) need the same `.timeout`, which
   the response does not name; P17 shows `m.on_init(..).timeout(..)` on a borrowing `ModuleHook<'_>`.

3. **A `pub` `open_terminal` restricts nobody; make it take a proof.** The response says "User code
   can't open one". Asserted: a `pub fn` on a public type is callable from every crate, and the core
   cannot tell a transport crate from a user crate. Probed, P18/P18b: a `DrainToken` with a private
   field and no constructor, handed owned to `Server::drain` when Draining begins, is the proof.
   `Execution::open_terminal(&token, ..)` keeps the phase check, so the token proves who and the phase
   proves when: a token kept past Draining still gets `Closed`. `DrainToken(())` from user code is
   E0423. `Execution::open` (§13) stays convention, which costs nothing: it refuses from Draining on.

4. **WS 1001 at drain start makes WebSocket the one transport whose drain is not graceful.** Read:
   §3.8 fires cancellation when "the client disconnects", and a server-initiated close is a
   disconnect. Closing every connection as Draining begins drops the reply channel of every message
   execution in flight, where HTTP/1 closes only idle keep-alives and HTTP/2 lets streams below the
   GOAWAY id finish. The same shape fits WS: idle connections close with 1001 at once, a connection
   with executions in flight closes when they end, bounded by `drain_timeout`. Asserted.

5. **`shutdown_timeout`'s expiry is defined for hooks only; define it per stage.** Read, 284–286. The
   cap can expire during Stopping, the drain, or a hook. One rule: on expiry every remaining step that
   waits on user code is skipped (hooks not run are recorded; a drain not finished cancels and abandons
   at once) and the two framework steps, stop accepting and close sockets, still run. "Abandoning" a
   hook also needs a meaning: a hook future borrows `&self` (§3.5), cannot be detached, and is dropped,
   stopping at its next await with its effects half-done. An abandoned execution keeps running (§3.8).
   The per-hook timeout drops the future the same way. Asserted.

6. **The report conflates three hook outcomes.** `ShutdownFailure::Hook { source }` (§10.2 line 825,
   documented as "the hook panicked") would now carry a panic, a per-hook timeout and a cap
   abandonment. A hook never started because the cap expired is not that hook's failure. Asserted:
   a `cause` on `Hook` (`Panicked` / `TimedOut` / `Skipped`) or a separate `ShutdownFailure::Skipped
   { hook, key }`; `StartupError::Hook` takes the same split. The line-825 comment changes either way.

7. **An explicit timeout without a `Timer` is the §9.3 wiring error, not "unbounded".** Read: §9.3 and
   §10.1 step 6 make a readiness `.timeout` with no `Timer` a wiring error; line 225 has hook bounds
   "run unbounded" without one. One rule: an explicit bound (`.timeout`, `TIMEOUT = Some(..)`) needs a
   `Timer` and is a wiring error without one; the app defaults (`hook_timeout`, `drain_timeout`) apply
   only when a `Timer` exists. `Timer` stays optional for an app writing no bound. Undefined beside
   it: how a hook declares no bound (`Some(Duration::MAX)` works; say so), and whether a readiness
   `.timeout` is per attempt or across the retries. §9.3 never says, and a `.timeout` on the hook
   beside it invites reading both the same way. Asserted.

8. **`load` from Stopping on.** Read, §8.6: shutdown includes lazily loaded modules in reverse order
   of loading. A `load` arriving once the sequence has started adds a module whose init hooks run
   during shutdown and whose destroy hooks never do. The phase table needs a `load` column: allowed in
   Running, refused from Stopping on. Asserted.

9. **Fold targets the response does not list.** Read: §2 line 49, §3.8 line 293 ("or shutdown
   begins"), §9.2 line 700, §9.5 lines 753–755 and §14 items 9 and 12 all state the old order or the
   cancel-first rule. The sequence at 243–249 replaces each.

## Questions

1. Is the `dyn Timer` binding the object the app times the drain and the hooks with, so a test
   override of `dyn Timer` (§11) replaces the internal timer too, or are there two timers?
2. Does `shutdown_timeout` start at the trigger, so Stopping counts against it? The recommendation
   at 288 holds only if it does.
3. `is_draining()` is `false` throughout Stopping. Is that intended for a long-lived stream that wants
   early notice, or should Stopping be observable too?
4. Does `draining()` live only on `ExecutionRef`? A detached task holding an `AppHandle` has no
   execution, and `execute` answering `Closed` is its only notice.
5. Constructors and factories have no bound. `hook_timeout` keeps a hook from hanging `connect`; a
   factory awaiting a connect still can. Is "`connect_lazy` plus a readiness check" the answer?
6. A terminal execution opened a moment before `drain_timeout` is cancelled almost at once, and a
   connection abandoned at the deadline never runs its disconnect handler. Intended?
7. With `TIMEOUT` on every hook trait, a type implementing two reads its own as
   `<Self as OnModuleInit>::TIMEOUT`; `Self::TIMEOUT` is ambiguous. Worth a line in §9.1?

## Probes

| Probe | Subject | Result, 1.88 and 1.98 |
|---|---|---|
| P15 | `Timer` with boxed `sleep`; `Dep<dyn Timer>` in a service with a `Send` hook future; `timeout` from `sleep` alone; the two key spellings | compiles; `Ok(5)` / `Err(TimedOut)`; TypeIds differ |
| P15b | `sleep -> impl Future + Send` | E0038 |
| P16 | `const TIMEOUT` default and override on two hook traits, read by autoref registration | `init=Some(10s) destroy=None`; none for `Plain` |
| P17 | `Handle<T, K>` chain as the response writes it, `also_as` back to `NoItem`, `m.on_init(..).timeout(..)` | each timeout on its own item |
| P17b | `.timeout` with no item | E0599 naming `NoItem: HasTimeout`; no `on_unimplemented` text |
| P17c | `.retries` after `.on_destroy` | E0599, "found for `Handle<T, ReadyItem>`" |
| P18 | `DrainToken` through `Server::drain`; `open` in Stopping; `open_terminal` after Draining | 3 opened in Draining, none after |
| P18b | `DrainToken(())` in user code | E0423, private field |

Run from `probes/` with `cargo run --bin <name>` and `cargo +1.88 run --bin <name>`; a `_fails` file
is built, and its error is the result. The only toolchain difference: 1.88 reports P15b's E0038 twice.
