# Refinements and questions on DESIGN.md

Section numbers refer to `DESIGN.md`; bracketed numbers to `CAPABILITIES.md`. Every claim is
marked **Probed** (a compiler run in the appendix), **Read** (follows from the text of two
sections) or **Asserted** (reasoning, not run). Probes ran on rustc 1.98.1 stable, edition 2024;
1.88 is not installed. Every language feature the probes lean on was stable before 1.88:
return-position `impl Trait` in traits and `async fn` in traits (1.75), `#[diagnostic::on_unimplemented]`
(1.78), associated-type bounds (1.79), `AsyncFnOnce` (1.85), explicit generic arguments beside
argument-position `impl Trait` (1.63). The one negative finding about a `const fn` (R5) can only
be worse on 1.88, since const-stability moves in one direction.

Items are ordered by severity: what does not compile or contradicts another section comes first.

## Refinements

**R1. `providers = [AppConfig::from_env()?]` does not compile inside `register`.** (§4, §8.1;
Probed, P09.) `register(&self, m: &mut ModuleDef<'_>)` returns `()`, and `?` in a `()`-returning
fn is E0277. The expansion shown in §4 drops the `?` and passes the `Result` to `m.value(..)`, which
would bind `Result<AppConfig, E>` under the key `Result<AppConfig, E>`. Change: the macro lowers
`expr?` to `m.try_value(expr)`, and `try_value` records an `Err` on the `ModuleDef` for `wire()` to
report beside the other wiring errors, under a `WiringErrors` entry that names the module and the
key. Cost: one method, one error variant, and a rule that `register` never returns early.

**R2. "A `Result` output is fallible" cannot be one `singleton(f)` in the value API.** (§4, §11, §13;
Probed, P02a–c.) Two blanket impls over `T` and `Result<T, E>` are E0119. A marker parameter
(`FactoryOutput<Plain>` / `FactoryOutput<Fallible>`) removes the overlap and moves the failure to
the call: a `Result` output satisfies both, so `M` is E0283 "type annotations needed". Autoref
ranking over the closure's concrete type does pick the fallible arm, but only at a call site that
names the concrete closure, which a generic `fn singleton<F>` never does. Change: two methods per
scope in the value API — `singleton`/`try_singleton`, `execution`/`try_execution`,
`transient`/`try_transient`, and `override_factory`/`override_try_factory` in §11 — and the
`#[module]` macro keeps the single spelling by ranking at the call site it writes. Cost: eight
names in §13's table where there were four; the sentence in §4 moves to the macro's description.

**R3. §7's example fails its own rule.** (§7; Read.) `#[guards(AuthGuard)]` sits on an impl with an
HTTP handler and an RPC handler, and the paragraph below says each transport's handlers check the
guard against that transport's role. `AuthGuard` implements `Guard<Http>` only, so
`spec.guard::<AuthGuard>()` for `get_rpc` fails on `AuthGuard: Guard<Rpc>`. Change one of: the
example implements both roles; controller-level enhancer attributes name their transport
(`#[guards(http = AuthGuard)]`); or a controller-level enhancer is refused when any handler's
transport lacks the role, with the message naming the handler. Applying it only where the role
exists is the one option to rule out — a guard that guards three of four handlers is the failure
[25] exists to prevent. Cost: attribute syntax.

**R4. `app.execute(.., |exec| async move { .. })` borrows its argument into the returned future.**
(§6.3; Probed, P10, P10b.) With `exec: &Execution` the design's spelling is "lifetime may not live
long enough": a plain closure's return type cannot name its argument's lifetime. Two spellings
compile. `F: AsyncFnOnce(&Execution) -> R` with the closure written `async |exec| { .. }` keeps
ownership of the execution in the framework, which can then close it after the future completes.
`F: FnOnce(ExecutionRef) -> Fut` hands out an owned handle, which makes the execution a shared
object (see R13). Change: pick one and write it in §6.3. Cost: none for the first; the second
decides R13.

**R5. `Key::names: &'static KeyNames` has no stable spelling.** (§3.1; Probed, P11.) A `static`
inside a generic fn is one static for every instantiation and cannot hold `T`'s name. An inline
`const { &KeyNames { ty: type_name::<T>(), .. } }` fails: "`std::any::type_name` is not yet stable
as a const fn" on 1.98.1, hence on 1.88. `type_name` returns `&'static str` at runtime; two
`&'static str` fields carry the same information with no `const`. Change: `ty_name` and `q_name`
fields, excluded from the manual `Eq`/`Hash`; two keys compare on the `TypeId`s alone. Cost: `Key`
grows by one pointer pair and loses `derive(PartialEq, Eq, Hash)` for a six-line impl.

**R6. The `AllowedIn` message names the wrong subject.** (§3.3, §5; Probed, P06.) `{Self}` in an
`on_unimplemented` on `AllowedIn<S>` is the site type, so the user reads "`Ext<CurrentUser>` cannot
read per-execution data" — but `Ext<CurrentUser>` is what reads it. The type that cannot is the one
declared `singleton`. `{S}` renders (the probe's label printed "a `Singleton` type cannot read this
site"). Change: `message = "a `{S}` type cannot read `{Self}`"`, label on the site, and the existing
note. Cost: none.

**R7. The `Dep<T>: Send + Sync` refusal cannot carry a "what to write" hint.** (§5, §12, [46];
Probed, P08, P08b.) The error a user sees is the auto trait's own: "`(dyn Repo + 'static)` cannot be
sent between threads safely". A wrapper trait with a blanket impl and an `on_unimplemented` note is
bypassed: rustc reports the unsatisfied `Send` from the blanket's where-clause and drops the
wrapper's note. The fix the user needs — `Send + Sync` as supertraits of `Repo` — cannot be stated
at compile time. It can at wire time: `dyn Repo` and `dyn Repo + Send + Sync` are distinct `TypeId`s
with distinct `type_name` strings (P08), so a missing-dependency report can check whether the
missing key's name equals a bound key's name up to ` + core::marker::Send + core::marker::Sync` and
name both spellings. Change: strike the compile-time claim for this row of §12; add the wire-time
hint. Cost: one string comparison on the missing-dependency path.

**R8. Two orders are given for one collection.** (§3.2, §7; Read.) The `Many<T>` row says
"registration order"; §7 says global contributions follow "module topological order and then
declaration order". A topological order of the import graph exists once cycles are refused, but it
is not unique, so two modules with no edge between them have no order under §7's rule. Change: one
rule, stated once — depth-first post-order over imports from the root, imports in the order
written, then declaration order inside a module. It is deterministic, and a module's contributions
come after those of everything it imports. Cost: none beyond text; §9.2's "topological order" for
hooks needs the same tie-break (Q9).

**R9. A `Send` future over `&Resolver<'_>` requires `Resolver: Sync`, hence `Execution: Sync` and
every seeded input `Sync`.** (§3.2, §3.4, §3.7, §6.4; Probed, P05, P05b.) `Site::read` and
`Construct::construct` hold the resolver across awaits; the generated constructor awaits one site
after another. A `Cell` inside the execution — standing for any `!Sync` input, such as a request
body stream — makes every site's `read` future `!Send`, and the error is reported at the trait's `+ Send`,
not on the input. §3.7 bounds `Transport::Cx: Send` only. Change: write `Execution: Send + Sync`,
`Resolver<'_>: Sync`, and `seed<T: Send + Sync + 'static>` into §3.8 and §6.4; a body stream then
lives behind a lock or on `Cx`, not as an input. Cost: a rule for transport authors that the text
currently leaves to the compiler's message.

**R10. `alias::<PgPool, ReadOnly, PgPool, Replica>()` has one type parameter too many.** (§4, §13;
Read.) An alias with two different types would be a coercion, and §4 puts coercions in `also_as`.
With the type fixed, four parameters leave the direction unstated: which pair is the new key. Change:
`alias::<T, New, Existing>()`, or `m.provide_alias::<PgPool, ReadOnly>().of::<Replica>()`. Cost:
none.

**R11. A `Construct` type bound through a factory compiles its hook impls and never runs them.**
(§3.5, §9.1; Asserted, from P02a's shape.) `#[injectable] struct Cache` plus
`impl OnModuleInit for Cache` compiles; `m.singleton(|| async { Cache::custom() })` binds it by
factory. §9.1 says trait hooks run only for types the container constructs, and `singleton` cannot
notice `Fut::Output: Construct` — the check needs negative reasoning over a blanket, which is the
E0119 of P02a again. The hook exists, compiles under [41]'s check, and is skipped. Change: a
`provide_with::<T>(factory)` recipe, keyed by a `Construct` type, that builds through the factory
and runs `T::hooks`; plus the sentence in §9.1 saying `singleton`'s output never has trait hooks even
when the type implements `Construct`. Cost: one method.

**R12. Reading global guards as `Many<AnyGuard<Http>>` builds every guard before the first runs.**
(§7, §3.2, [27]; Read.) `Many<T>` yields `Arc<[Arc<T>]>`, and reading it constructs every
contribution. §7 step 2 says a guard is obtained only after the one before it admits, and a
per-execution guard's construction is the cost [27] defers. The transport cannot read the
role collection as a `Many`; it needs the collection's binding handles and a per-item resolve.
Change: name that surface in §13 — `Resolver::many_keys::<T>()` or an `EnhancerSpec` iterator that
resolves one entry at a time — and say `Many<T>` is the eager form for user code. Cost: one
resolver method.

**R13. `&mut T::Cx` guards nothing the design still needs guarded, and a streaming reply has
nowhere to keep the execution.** (§3.7, §3.8, §7, [23]; Read, with P13.) §3.8 declares
`Execution::extensions(&self) -> &Extensions`, and §7's guard writes through it
(`cx.extensions().insert(..)`); the write already goes through a shared reference. What `&mut`
still does is force `Next::run` to reborrow and forbid a reply from holding the context: P13 shows
`T::Reply: 'static` follows from `Transport: 'static`, so a reply can carry the execution only by
owning or sharing it, never by borrowing `cx`. A body, a reply stream or a WebSocket item sequence
outlives `intercept`'s return; the per-execution instances it was built from, and the cancellation
signal [23] promises, have to outlive it too, and the text does not say who owns the `Execution`
after the pipeline returns or what fires `cancelled()`. Change: `Cx` is a cheap-clone `Send + Sync`
handle to the execution, guards and interceptors take `&T::Cx`, the reply carries a clone, and the
signal fires when the last clone drops. Cost: interior mutability on whatever else `Cx` mutates,
which the design already pays for extensions.

## Questions

**Q1. Which module's visibility does a lookup without a `ModuleRef` use?** (§3.8, §6.3, §8.5, [47];
Read.) `Execution::get`, `app.get` and `exec.get` in the `execute` closure name no module. §8.2 says
visibility is per module and §8.5 limits a `ModuleRef` to what its module sees. If these resolve
against the root module, a service exported only within a subtree is unreachable from a job; if
against every binding, a key two modules bind privately is ambiguous, and `LookupError` has no
variant for that. Which is it, and which variant answers the ambiguity?

**Q2. Which binding does `override_value::<dyn UserRepo>` replace when two modules each bind the
key privately?** (§11, §8.2; Read.) Keys are per module; one key can have several unrelated
single bindings across the graph. Is an override that matches more than one a wiring error, is there
an `in_module::<M>()` scoping, or does the key of an override have to be unique app-wide?

**Q3. What handle does `app.load(..)` run on once `serve(self)` has consumed the app, and which
collections can a lazy module still contribute to?** (§8.6, §9.4, §14.6; Read.) `serve` takes
`self` and `load` needs the graph behind a lock. Singletons are eager and `Many<T>` is read at
construction, so after `connect` every collection with a singleton holder is "already injected".
Is a late contribution possible only to a collection read per execution, and is that stated?

**Q4. How does a `Secret<_>` become "registered" so the redaction pass can find it?** (§9.3, §13;
Read.) In §13 the `Secret<String>` is moved into the factory closure and the module's
configuration value; the container never receives it as a value, and `of_value` is generic over the
configuration and cannot walk it. For that module only the URL-userinfo strip runs. Is `Secret`
expected to register itself on construction (a global list), or is the redaction of registered
values limited to `m.value(Secret<_>)` bindings?

**Q5. Does a guard's `Ok(false)` reach the error handlers?** (§3.7, §7 step 4; Read.) Step 4 routes
errors through the handlers; a refusal is not an error in the signature. If the transport renders the
refusal directly, an application cannot reshape a 403 for one route; if it is routed, the handler
receives a `BoxError` of what type? Either answer is fine; the design gives neither.

**Q6. Is a bare `DbModule::for_root(url)` beside `DbModule::for_root(url).keyed::<Primary>()` one
module or two, and does `Keyed` requalify re-exports?** (§8.3, §3.6; Read.) `Keyed`'s identity
includes `Q`, so the two are distinct and one URL yields two pools with two readiness checks. Is that
intended, and does the requalification at the export boundary cover `reexport PgPool` inside `M`,
inputs declared inside `M`, and contributions (which §8.2 makes app-wide and §13 leaves unqualified)?

**Q7. Is a per-execution service reading an HTTP input, reached from an RPC controller, a wiring
error or a runtime one?** (§6.4, §12, [44]; Read.) The graph knows every controller's transport
and every site's key; an input seeded by one transport and read on a path from another is knowable
at `wire()`. The text answers `LookupError::NotFound { kind: Input }` at runtime for standalone
executions and says nothing for the cross-transport case.

**Q8. `Option<Ext<T>>` and `Option<Dep<Input>>`: is an extension not written `None` or `NotFound`?**
(§3.2, §12; Probed, P05.) The `Option<S>` row defines `None` as "unbound", a static property.
Extensions and inputs are always bound and sometimes absent at runtime. P05's `Option<Ext<T>>` maps
the runtime miss to `None`; the design should say whether that is the rule, since `Ext<CurrentUser>`
before an authenticating guard is the common case.

**Q9. What is the tie-break for hooks and readiness checks between bindings with no edge?** (§9.2,
[40]; Read.) "Topological order" and "a module's own hooks run after its providers'" leave two
independent modules unordered. R8's rule (import post-order, then declaration order) would settle
both; is that the intended order?

**Q10. What does a hand-written `Construct` impl call to fill `hooks`?** (§3.4, §13, principle 2;
Probed, P04.) The autoref probe pair works with concrete `Self` and needs a public probe type per
hook trait (five pairs). An integration crate writing `Construct` by hand, as §13 promises, has to
call them — `Hooks::probe::<Self>()` or five methods — and §13's table lists only `Hooks<T>`.

**Q11. What does `ModuleRef::get` return during `connect` for a singleton not yet built?** (§8.5,
§9.2, §14.6; Read.) A `ModuleRef` read as a site exists before the store is complete, and an init
hook can call `here.get::<X>()`. Eager order guarantees the caller's dependencies exist, not `X`.
Building on demand contradicts §14.6; `NotFound` misdescribes a bound key; a third variant is
needed or the call is refused until `connect` returns.

**Q12. Is `WrongType` reachable?** (§10.2; Read.) A `TypeId` key cannot hold a value of another
type. If the variant means single-versus-collection, its fields say so (`BindingKind`) and the name
does not; if it means anything else, what?

§14 ends at item 7 and reads complete. Three defaults the body relies on are missing from it:
the collection order (R8), who owns the execution after the pipeline returns (R13), and that
`Many<T>` is eager (R12).

## Appendix: probes

rustc 1.98.1 stable, edition 2024, `rust-version = "1.88"`, no dependencies. Crate: `probes/`
beside this file, one file per probe under `src/bin/`, run one at a time with
`cargo run --bin <name>`. A `*_fails.rs` name means the failure is the result.

| Probe | Shows |
| --- | --- |
| `p01_closure_unsize.rs` | `\|a\| a` unsizes `Arc<RedisCache>` to `Arc<dyn Cache>` at the closure's return under `F: Fn(Arc<T>) -> Arc<U>` with `U` from the turbofish; the stored erased coercion round-trips through `Arc<dyn Any + Send + Sync>`; the role twin `dyn ErasedGuard<Http>` over a blanket impl boxing the `impl Future` runs |
| `p01b_closure_unsize_fails.rs` | the non-implementing type is E0277 at the closure's `a`: "required for the cast from `Arc<NotCache>` to `Arc<dyn Cache>`" |
| `p01c_apit_turbofish.rs` | `also_as::<dyn UserRepo>(\|a\| a)` with one type argument compiles when the closure is argument-position `impl Fn` |
| `p02a_factory_output_fails.rs` | blanket impls over `T` and `Result<T, E>` are E0119 |
| `p02b_factory_marker_fails.rs` | the marker parameter compiles the impls and fails the call: E0283 on `M` for a `Result` output |
| `p02c_factory_autoref.rs` | `(&Probe(f)).kind()` reaches the fallible arm for a `Result` future and the plain arm otherwise; the first run with `&&` reached the plain arm twice, so the rank is one autoref, not two |
| `p03_hook_supertrait.rs` | `trait OnModuleInit: Construct<Scope: HookCapable>` compiles; a user's `async fn` implements the `-> impl Future + Send` method and the future is `Send` |
| `p03b_hook_on_execution_fails.rs` | a hook on a `PerExecution` type is E0277 at the impl header, printing the design's message with `{Self}` rendered `PerExecution` and the note |
| `p03c_hook_not_send_fails.rs` | an `Rc` held across an await in the impl's `async fn` fails on the trait's `+ Send`, pointing at the impl's fn |
| `p04_hooks_autoref.rs` | `Construct::hooks` filled by a probe pair with concrete `Self` registers one `for<'a> Fn(&'a T) -> BoxFuture<'a, _>` hook for the implementing type and none for the other; the closure `\|s\| Box::pin(s.on_module_init())` gets the HRTB signature from the bound |
| `p05_site_read_erased.rs` | `Site::read(&Resolver<'_>)` as RPITIT; `Option<S>` forwarding a runtime miss to `None`; a generated `construct` awaiting two sites; the registry slot `for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, _>` filled through a helper fn; the constructor future is `Send` |
| `p05b_execution_sync_fails.rs` | a `Cell` in the execution makes every `read` future `!Send`: "within `Resolver<'_>`, the trait `Sync` is not implemented for `Cell<u32>`" |
| `p06_allowed_in_fails.rs` | the `AllowedIn<Singleton>` assertion fails at the site type; `{Self}` is `Ext<CurrentUser>`, `{S}` renders `Singleton` |
| `p07_two_notes_fails.rs` | two `note =` entries both print, with `{Self}` substituted inside a note |
| `p08_typeid_dyn.rs` | `TypeId::of::<dyn Repo>() != TypeId::of::<dyn Repo + Send + Sync>()` with `Repo: Send + Sync`; the two `type_name` strings |
| `p08b_shareable_diag_fails.rs` | a wrapper trait's `on_unimplemented` note is not printed when its blanket impl's `Send + Sync` bound is what fails |
| `p09_register_qmark_fails.rs` | `?` inside `register(&self, ..)` returning `()` is E0277 |
| `p10_execute_closure.rs` | `F: AsyncFnOnce(&Execution) -> R` with `async \|exec\| { .. }` borrows the execution into the future; `FnOnce(ExecutionRef) -> Fut` with an owned handle also compiles |
| `p10b_execute_borrow_fails.rs` | the design's `\|exec\| async move { .. }` over `&Execution` is "lifetime may not live long enough" |
| `p11_key_names_const.rs` | first run: `type_name` in an inline `const` is "not yet stable as a const fn"; two `&'static str` fields and a manual `Eq`/`Hash` over the `TypeId`s compile and print `PgPool @ Replica` |
| `p12_factory_arity_coerce.rs` | one `singleton(f)` over a `Factory<Args>` trait for arities 0, 1, 2 describes sites from the parameter types; `contribute::<dyn HI>().singleton(f, \|a\| a)` infers `T` from the first closure before checking the second; `.ready(..).retries(..).timeout(..).on_destroy(..)` chains on one handle type |
| `p12b_factory_unannotated_fails.rs` | an unannotated factory parameter is E0283 with "consider giving this closure parameter an explicit type", so every factory parameter carries its site type |
| `p13_reply_static.rs` | `T::Reply` boxed into a `'static` future compiles with only `Reply: Send` written: `Transport: 'static` gives the projection `'static` |
| `p14_execute_send.rs` | an inherent `async fn execute<F, R>(&self, f: F) -> R where F: AsyncFnOnce(&Execution) -> R`, called as `app.execute(async \|exec\| ..)` from a concrete site, returns a future that passes `fn assert_send<T: Send>(_: &T)`; the closure also takes `exec.handle()` for an owned clone and the run prints `(Ok("borrowed"), "borrowed")` |
| `p14b_execute_trait_send_fails.rs` | the same `execute` as a trait method returning `impl Future<Output = R> + Send` fails at the impl: "the trait `Send` is not implemented for `<F as AsyncFnOnce<(&Execution,)>>::CallOnceFuture`"; the one bound that would state it, `for<'a> <F as AsyncFnOnce<(&'a Execution,)>>::CallOnceFuture: Send`, is E0658 `async_fn_traits` |

### Rust 1.88.0 re-run

Every probe was re-run on rustc 1.88.0 (`cargo +1.88`) beside 1.98.1, with the output of the two
runs diffed after build chatter was stripped. Every probe has the same compile outcome, exit code
and error codes on both. The diagnostic probes render identically where it matters: `{S}` is
`Singleton` in P06's label on 1.88, `{Self}` is `Ext<CurrentUser>` in P06's message and
`PerExecution` in P03b's label, both of P07's notes print, P12b prints "consider giving this
closure parameter an explicit type", and P08b drops the wrapper's note on both. P02c reaches the
same autoref arms, P08 prints the same `TypeId` inequality and P11 the same `PgPool @ Replica`.

What differs is presentation:

| Probe | 1.88.0 | 1.98.1 |
| --- | --- | --- |
| P01b, P03b, P06, P07, P12b | "the trait `X` is not implemented for `Y`" is an `= help:` line without a span (P01b prints it as the label) | the same text as a `help:` with a span pointing at the type or impl; P01b's label becomes "unsatisfied trait bound" |
| P03c, P05b, P14b | the `!Send` future's caret underlines the whole `async fn` or return type | the caret sits on the `{` or the `impl Future` span |
| P12b | ``= note: cannot satisfy `_: Site` `` | ``= note: the type must implement `Site` `` plus "required by a bound introduced by this call" |
| P12 | `#[warn(dead_code)]` on by default | `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default |
| P14b | the `!Send` error is reported twice, at the async block and at the signature: 4 errors | once, at the signature: 3 errors |
| all | line numbers in a two-digit gutter are left-aligned (`6  \|`) | right-aligned, the pad before the digit |
