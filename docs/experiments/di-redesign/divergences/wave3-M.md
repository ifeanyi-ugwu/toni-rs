# Divergences: wave 3, agent M (the macros)

Every place M's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see, for D21 (one grammar for every `into` list) and D23 (`into` lowers to `contribute`). Each
entry gives what the design says, what M wrote, and why. Superseded wave 2 entries and requests for
core files follow the entries.

## Entries

### 1. A `with` closure in an `into` list is written as in `#[guards]`

- **Design:** §4: an `into` list takes "the entry forms `#[guards]` takes", and `with = closure`
  lowers "through the same autoref call site a `providers` factory takes". `#[guards]` wraps a
  synchronous body in `async move`. A providers-list closure returns its future as written.
- **M:** an `into` list's `with` closure goes through `#[guards]`' wrapper. A synchronous body is
  wrapped in `async move`, and a closure already `async`, or whose body is an `async` block, is
  kept, so both spellings compile. A providers-list closure outside an `into` list still returns
  its future.
- **Limit:** a synchronous body returning a future it does not await,
  `|c: Dep<C>| Plugin::connect(c)` with `connect` an `async fn`, becomes a future of a future. The
  plain arm takes it, and the build fails at the coercion `|a| a`, the future not implementing the
  key's trait. `#[guards]` has the same limit.
- **Why:** D21 gives `with` one spelling in both places, and the wrapper's pass-through keeps the
  providers-list spelling compiling inside an `into` list.

### 2. `with` builds a singleton in an `into` list and per execution in `#[guards]`

- **Design:** §4: a `with` entry registers a singleton contribution. §7: `#[guards(with = ..)]`
  builds per execution.
- **M:** as designed. The consequence: `with = |u: Ext<CurrentUser>| RoleGuard::require(u)` works
  in `#[guards]`, and in `into AnyGuard<Http>: [..]` it is refused at `wire()` as a singleton that
  needs an execution (§6.2). A per-execution contribution is written against the value API,
  `.execution(..)` or `.try_execution(..)`, in a hand-written `Module`.
- **Why logged:** the design states both scopes but not that one spelling carries both, and
  §6.2's report names the scope, not the attribute the user copied the closure from.

### 3. `value = expr?` in an `into` list is a compile error

- **Design:** silent. A providers list lowers `expr?` to `try_value`; `Contribute` has no
  `try_value`.
- **M:** a span error on the expression: "a contribution has no fallible value form; write
  `with = || expr` without the `?`, and an `Err` fails `connect`".
- **Why:** lowered as written, the `?` sits in `register`, which returns `()`, and rustc's error
  asks for a return type the user cannot change. `with = || expr` reaches `try_singleton` through
  the ranking, and its `Err` reports at `connect` as `ConnectError::Construct { reason: Errored }`
  rather than at `wire()`, where a providers list's `try_value` reports.
- **Limit:** a `with` closure is `'static`, so a configured module's fallible value built from its
  own fields, `value = Plugin::new(&self.cfg)?`, has no `#[module]` spelling. Request R2 would give
  it one.

### 4. A malformed `into` item is a span error naming the three forms

- **Design:** silent on the text.
- **M:** "an `into` entry is a type, `value = expr` or `with = |..| ..`", spanned on the item;
  when the item parses as an expression, "; an expression is contributed with `value = ..`"
  follows. It covers a call, `MetricsPlugin::new()`, which syn reads as a path with parenthesized
  arguments; a transport-scoped form, `http = AuthGuard` or `http(value = ..)`; and any key before
  `=` other than `value` and `with`. A `with` whose right side is not a closure reads "`with`
  takes a closure whose parameters are injection points, as in
  `with = |cfg: Dep<MetricsConfig>| MetricsPlugin::new(cfg)`". A closure parameter without its
  type gets the providers list's factory-parameter text. Every other type is accepted, a
  `macro_rules` `$t:ty` included.
- **Why:** wave 2 took any type, and `MetricsPlugin::new()` became
  `provide::<MetricsPlugin::new()>`, which rustc refuses with a message about `Fn` sugar.

### 5. The `as <role key>` error stays

- **Design:** D23 removes token-level role-key recognition from the lowering and leaves the
  providers-list error for `AuthGuard as AnyGuard<Http>` to M.
- **M:** kept, text unchanged. `as` lowers to `also_as`, a single binding under the role key. The
  core gives roles to contributions only, `dispatch` walks `entries`, one per contribution, and
  `wire()` reports the binding only when a contribution to the same key makes it a
  single/collection mix. Otherwise the guard is bound and guards nothing. The recogniser is now
  `written_as_role_key`, read by this diagnostic alone; no lowering depends on it.
- **Limits:**
  - An alias under another name, `X as HttpGuards`, passes unreported. Request R1 would report it
    at `wire()`.
  - A user's own type named `AnyGuard`, `AnyInterceptor` or `AnyErrorHandler`, or `dyn` of a trait
    named `ErasedGuard`, `ErasedInterceptor` or `ErasedErrorHandler`, is refused after `as`
    although it is not a role key. Renaming the import binds it.
- **Why:** a guard that guards nothing compiles, wires and serves, and a compile error at the entry
  is the earliest report available. A false refusal needs a type sharing a role key's name.

### 6. No `#[module]` spelling marks a role key no mounted handler reads

- **Design:** §7: `enhancer::<R>()` is the explicit mark for a role key of a transport with no
  mounted handler. Unmarked, an `Auto` entry needing an execution is refused as a singleton
  (§6.2). §4: every `into` list lowers to `contribute`.
- **M:** as designed, and an `into` list cannot mark. A module contributing global enhancers for a
  transport the app mounts no handler of is refused at `wire()` when one of its `Auto` enhancers
  needs an execution: a shared auth module wired into a worker binary, or a test app without
  controllers. `mark_role_contributions` marks every contribution under a key that any
  `enhancer`-written contribution carries, so one hand-written `m.enhancer::<K>()` contribution
  anywhere in the graph marks the `into` list's too. Otherwise the module implements `Module` by
  hand.
- **Option, if a spelling is wanted:** `into enhancer AnyGuard<Http>: [..]`, lowering to
  `m.enhancer::<K>()`, its `Role` bound refusing a key that is not one.

## Superseded wave 2 entries

- **[M 2]** (a role key recognised by how it is written, lowered to `enhancer`) → every `into` list
  lowers to `m.contribute::<K>()`, and the core gives the role by `TypeId` at freeze (D23). Both of
  its limits are gone. The recognition survives for entry 5 alone.
- **[M 3]** (`X as <role key>` an error) → stands; entry 5 records the decision under D23.
- **[M 4]** (an `into` list takes types only) → `value = expr` and `with = closure` (D21).
- **[M R1]** (doc comments showing `contribute` for a global enhancer) → applied in
  `binding/contribute.rs`.
- **[M R2]** (a diagnostic on `Role`) → the macro no longer writes `m.enhancer`; the text in
  `transport/mod.rs` serves a hand-written `enhancer` call.
- **[M R3]** (the signature the expansion relies on) → the expansion names no `enhancer`; what it
  names is listed below.
- `DIVERGENCES.md` Wave 2 §2's bullet "`into K: [A, B]` lowers to `m.enhancer::<K>()` when `K` is
  written like a role key", §3's "two limits of token-level recognition" and §5's "role-key
  recognition looks through parentheses and the invisible group" → entry 5; the group look-through
  also serves the `into` item's call check.

## What the expansion names

- `m.contribute::<K>()` with `provide::<A>(|a| a)` and `value(Arc<K>)`, reachable on the `Plain`
  builder through the shared `impl<.., M>`.
- `::ulo::__private::Arc`, a re-export of `std::sync::Arc` added for `value =`.
- `::ulo::__private::factory::{Probe, FallibleContribution, PlainContribution}`, added for `with =`.
  Their impls call `Contribute::try_singleton::<Args, F, T, E>` and `Contribute::singleton::<Args, F>`
  by turbofish, so the order of those generic parameters is part of what the expansion relies on.
  The coercion closure `|a| a` is written in the expansion, where the built type unsizes to `K`.
  It is typed through the associated type `Built`, which the ranking resolves.

## Requests for core files

### R1 (W). A wiring refusal for a single binding under a role key

A single binding whose key is a role key the graph records, a mounted handler's or one marked
through `enhancer`, including the second key `also_as` writes, is read by no transport. Refusing
it at `wire()` reports the alias case of entry 5, `X as HttpGuards`, and the value API's
`m.provide::<X>().also_as::<AnyGuard<Http>>(|a| a)`, which no macro sees. The record's
`BindingKind::Single` against the role set `mark_role_contributions` builds decides it. Suggested
text: "`AnyGuard<Http>` is a role key, and a single binding under it is read by no transport;
contribute it with `into AnyGuard<Http>: [X]` or `m.contribute::<AnyGuard<Http>>()`".

### R2 (W, optional). `Contribute::try_value`

`try_value<E: Into<BoxError>>(self, value: Result<Arc<U>, E>)`, recording an `Err` as
`ModuleDef::try_value` does, under the collection key. `value = expr?` would then lower to
`.try_value(Result::map(expr, |v| -> Arc<K> { Arc::new(v) }))`, reporting at `wire()` like a
providers list's `expr?`, and entry 3's refusal and limit would go.
