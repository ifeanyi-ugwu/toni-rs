# Divergences: race 2a, `cfg_attr` on a `#[routes]` handler method

`batch2a-macros.md` "Not covered" listed two forms that compiled with the predicate off and failed
with it on: a transport attribute inside `cfg_attr`, and an enhancer or `#[meta]` attribute inside
`cfg_attr` on a method. Both reach `#[routes]` unevaluated, as a `cfg_attr`, and expand after
`#[routes]` has run. `#[routes]` now reads a method's attributes through `cfg_attr`, at any depth,
and carries each predicate to where the generated code needs it.

Files changed: `crates/ulo-handler-codegen/src/{cfg.rs, emit.rs, keys.rs, protocol/mod.rs,
protocol/enhancers.rs}`, `crates/ulo-macros/src/{routes/mod.rs, enhancers/mod.rs, lib.rs}`,
`crates/ulo-http-macros/src/route.rs`.

## The signature

```rust
// ulo_handler_codegen::cfg (new items)
pub struct GatedAttr { pub predicates: Vec<TokenStream>, pub attr: Attribute }
impl GatedAttr { pub fn gates(&self) -> Vec<Attribute>; }
pub fn leaves(attrs: &[Attribute]) -> Vec<GatedAttr>;
pub fn take(attrs: &mut Vec<Attribute>, take: impl FnMut(&Attribute) -> bool) -> Vec<GatedAttr>;
pub fn gates_of(predicates: &[TokenStream]) -> Vec<Attribute>;
pub fn under(predicates: &[TokenStream], meta: TokenStream) -> TokenStream;

// ulo_handler_codegen::protocol
pub struct EnhancerAttr { pub role, pub entries, pub span, pub gates: Vec<Attribute> }   // `gates` new
pub struct MetaTokens { pub controller: Vec<MetaExpr>, pub method: Vec<MetaExpr> }       // was Vec<Expr>
pub struct MetaExpr { pub gates: Vec<Attribute>, pub expr: Expr }                         // new
impl HandlerTokens { pub fn to_meta(&self) -> TokenStream; }                              // new
pub fn take_meta(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<MetaExpr>>;               // was Vec<Expr>
pub fn is_meta(attr: &Attribute) -> bool;                                                 // new
pub fn meta_exprs(attr: &Attribute, gates: &[Attribute]) -> syn::Result<Vec<MetaExpr>>;   // new
pub fn outside_routes(attr_name: &str) -> syn::Error;                                     // new
```

`ulo-http-macros` is the one transport crate in the workspace. It reads the protocol only through
`take_handler_attr`, `MountFn::emit` and `__enhancer_specs!`, which all changed together, so it
needed one edit: its outside-`#[routes]` error now comes from `protocol::outside_routes`. A
transport crate written against the protocol the same way keeps working unchanged; one that builds
`EnhancerAttr` or `MetaTokens` by struct literal gains a field to fill.

## What each form lowers to

For

```rust
#[routes]
impl Users {
    #[ulo_http::get("/")]
    fn index(&self) {}

    #[cfg_attr(feature = "x", ulo_http::get("/beta"), guards(Deny), inline)]
    #[cfg_attr(feature = "x", meta(Tag))]
    fn beta(&self) {}
}
```

`#[routes]` writes (dumped from the macro's output, formatted, the `__handler` line wrapped):

```rust
impl Users {
    #[ulo_http::get("/")]
    #[::ulo::__private::__handler(index, controller(), method(), meta(controller(), method()))]
    fn index(&self) {}
    #[cfg_attr(feature = "x", ulo_http::get("/beta"), inline)]
    #[cfg_attr(feature = "x", ::ulo::__private::__handler(beta, controller(),
        method(#[cfg(feature = "x")] guards(Deny)),
        meta(controller(), method(#[cfg(feature = "x")] Tag))))]
    fn beta(&self) {}
}
impl ::ulo::Controller for Users {
    fn mount(m: &mut ::ulo::Mount<'_>) {
        let __ulo_shared = ::ulo::__private::Shared(());
        Self::__ulo_mount_index(m, &__ulo_shared);
        #[cfg(feature = "x")]
        Self::__ulo_mount_beta(m, &__ulo_shared);
    }
}
const _: () = <Users>::__ULO_CHECKS_index;
#[cfg(feature = "x")]
const _: () = <Users>::__ULO_CHECKS_beta;
```

and, inside `__ulo_mount_beta`, the transport's expansion of `__enhancer_specs!` and the metadata
block carry each entry's gates:

```rust
#[allow(unused_mut)]
let mut __ulo_method = ::ulo::EnhancerSpec::<::ulo_http::Http>::new();
#[cfg(feature = "x")]
{
    fn beta<__UloE: ::ulo::Guard<::ulo_http::Http>>(spec: &mut ::ulo::EnhancerSpec<::ulo_http::Http>) {
        spec.guard::<__UloE>();
    }
    beta::<Deny>(&mut __ulo_method);
}
// ..
#[cfg(feature = "x")]
__ulo_meta.method(Tag);
```

1. **Transport attribute inside `cfg_attr`.** `__handler` is appended inside a `cfg_attr` with
   the same predicates, after every attribute the method carries, so rustc expands it into place
   right after the transport attribute in exactly the builds that have one. Each predicate joins
   the handler's gates as `#[cfg(<predicate>)]`, beside its `presence_gates`, so its mount call,
   its `__ULO_CHECKS_*` read and its key in the key assertion are gated as a `#[cfg]` handler's
   are. Nested predicates join as `all(a, b)` on `__handler` and as one `#[cfg]` each in the gates.
2. **Enhancer inside `cfg_attr`.** `#[routes]` removes the `guards(..)`, `interceptors(..)` or
   `error_handlers(..)` from the `cfg_attr` and carries it in `__handler`'s method tier behind one
   `#[cfg(<predicate>)]` per enclosing predicate. `__enhancer_specs!` writes each entry as one
   block statement under those gates.
3. **`#[meta]` inside `cfg_attr`.** Each expression is carried as a `MetaExpr` with the same gates
   and written as a gated `__ulo_meta.method(expr);` statement.

A `cfg_attr` keeps what `#[routes]` does not take, in place and in order:
`#[cfg_attr(p, doc = "..", ulo_http::get("/m"), guards(Deny), inline)]` becomes
`#[cfg_attr(p, doc = "..", ulo_http::get("/m"), inline)]`, and a `cfg_attr` left empty is removed.

## Decisions

### 1. Which attribute is the transport attribute when `cfg_attr` hides it

- **Written:** an attribute outside `attrs::is_inert` decides, read through `cfg_attr`. One
  written outside any `cfg_attr` makes the method a handler in every build. When every such
  attribute sits inside a `cfg_attr`, the first in source order decides, and the method is a
  handler where its predicates hold.
- **Why:** `#[routes]` cannot tell a transport attribute from another attribute macro, such as
  `#[tracing::instrument]`, by name. The ungated-wins half keeps every method with an attribute
  macro outside `cfg_attr` classified as it was when `cfg_attr` counted as inert:
  `#[cfg_attr(feature = "trace", tracing::instrument)] #[ulo_http::get("/")]` stays an ungated
  handler. The first-wins half covers the common order, verb first. The alternative, gating on the
  disjunction of every gated attribute macro's predicates, hands `__handler` to a method with no
  transport attribute in a build where only the other macro's predicate holds.
- **Changed behaviour:** a method whose only attribute macros sit inside `cfg_attr` was a plain
  method before and is now a handler gated on the first one. A helper carrying
  `#[cfg_attr(feature = "x", tracing::instrument)]` in a `#[routes]` impl compiled before; it now
  fails with the feature on, with the unconsumed-`__handler` message, which ends "a helper goes in a
  separate impl block". The ungated `#[tracing::instrument]` on a helper already failed the same
  way, and the `#[routes]` doc already sends helpers to a separate impl block.

### 2. `#[cfg]` on a statement, not on a list element

- **Probed on rustc 1.88 and 1.98.1:** `#[cfg(..)]` on a block expression statement, on a
  method-call statement, and two stacked on one statement are all accepted, with the feature off
  and on. `#[cfg]` on an array, `vec!` or tuple element also compiles on 1.88.
- **Written:** statements. `__enhancer_specs!` already emits one statement (or a `let` plus a
  block) per entry, and the metadata block one statement per expression, so a gate goes on the
  statement without restructuring either. An entry's `let __ulo_enhancer_N = expr;` and its
  registration block are now wrapped in one outer block, so one gate covers both; the value is
  still bound outside the block holding the local fn, so the fn does not shadow a free fn the
  expression calls.

### 3. Gates travel inside `__handler`, per attribute

- **Written:** `EnhancerAttr::gates` and `MetaExpr::gates`, printed as outer attributes in front of
  the attribute or expression in the `__handler` tokens and parsed back with
  `Attribute::parse_outer`.
- **Why:** the transport attribute writes the code that registers the entry, so the gate must reach
  it. Emitting one `__handler` per combination of predicates instead grows as 2^n in the gated
  attributes. At the controller tier `gates` is always empty: rustc evaluates the impl's own
  `cfg_attr` before `#[routes]` reads it (`batch2a-macros.md` entry 3), which the scratch crate
  confirms again below.

### 4. The binding a gated entry registers into

- **Written:** `let mut __ulo_method` carries `#[allow(unused_mut)]` when every statement
  registering into it is gated; `let mut` when one is ungated; `let` when none applies.
- **Why:** a build compiling every gated statement out would otherwise warn `unused_mut`.

### 5. A gated enhancer or `#[meta]` on a method that is no handler is refused in every build

- **Written:** `#[cfg_attr(feature = "x", guards(Deny))] fn helper(&self)` is refused with
  "#[guards] on `helper`, which carries no transport attribute and so is not a handler", with the
  feature off as well as on.
- **Why:** the same refusal the ungated form gets, raised in the build that would otherwise compile
  it unread.

### 6. A misspelled transport key on a gated method entry fails in every build that has the handler

- **Written:** `__enhancer_specs!` checks method-tier keys at expansion, before rustc evaluates the
  gates it writes, so `#[cfg_attr(feature = "x", guards(htpp = Deny))]` on an ungated `http`
  handler fails with the feature off too.
- **Why:** an entry for another transport on one handler applies to nothing in any build.

### 7. The key assertion lists cfg_attr-gated handlers with the `#[cfg]` ones

- **Written:** the list label reads "behind `cfg`" in place of "behind `#[cfg]`":
  "`http` is not the key of any handler's transport in this impl (handlers behind `cfg`: a)".
- **Why:** a handler gated by the `cfg_attr` around its transport attribute is listed there and was
  never written with `#[cfg]`.

### 8. A verb-gated method is dead code where its predicate fails

- **Written:** nothing suppresses it. With the predicate off the method is a plain method, and
  `dead_code` fires unless something calls it.
- **Why:** a method that nothing calls in that build is what `#[cfg(feature = "x")]` on the method
  expresses, and the warning points there. Adding `#[cfg_attr(not(p), allow(dead_code))]` from the
  macro would hide a method that is dead for a reason the user should see. The `#[routes]` doc
  says so.

### 9. Standalone diagnostics

- **`#[guards]`, `#[interceptors]`, `#[error_handlers]`, `#[meta]`:** a method's marker inside
  `cfg_attr` below `#[routes]` no longer reaches the marker macro. The paths left are an item no
  `#[routes]` expands (a plain impl, a free fn, a `#[cfg_attr(p, routes)]` impl where `p` fails)
  and an attribute above `#[routes]`, including one inside a `cfg_attr` above it. The message now
  says that: "#[guards] is read by #[routes]: it goes on a #[routes] impl block, below #[routes],
  or on a method of one; no #[routes] read this one, so it is outside a #[routes] impl or above
  #[routes], which expands after it".
- **A transport attribute with no `__handler`:** besides a method outside any `#[routes]` impl,
  this is reached when every attribute macro on the method sits inside `cfg_attr` and the first is
  not the transport attribute, in a build where that first one's predicate fails.
  `protocol::outside_routes` writes the message for every transport crate: "#[get] goes on a method
  of a `#[routes]` impl, which hands it the handler's enhancers; if this method is in one,
  `#[routes]` took another attribute for its transport attribute: a method has one, and when every
  attribute macro on it sits inside `cfg_attr`, the first is taken".
- **`__handler` unconsumed:** reached when decision 1 made a method a handler on an attribute
  macro that is not a transport attribute supporting `#[routes]` in this build: a helper with an
  attribute macro, a transport crate without `#[routes]` support, or a transport attribute compiled
  out while another attribute macro decided. The message states the rule and the helper remedy:
  "no transport attribute read this handler's enhancers: #[routes] took this method for a handler
  because of an attribute macro on it, read through `cfg_attr` (one outside any `cfg_attr` counts in
  every build, otherwise the first inside one decides), and no transport attribute supporting
  #[routes] is present in this build; a helper goes in a separate impl block".

## Unsupported, and how it fails

- **A transport attribute inside `cfg_attr` beside an ungated attribute macro**, in either order:
  `#[cfg_attr(feature = "x", ulo_http::get("/a"))] #[passthru]` or the reverse. Decision 1 makes
  the method an ungated handler. With the feature on it works; with it off, compile errors: the
  unconsumed `__handler` message and E0599 for `__ulo_mount_a` and `__ULO_CHECKS_a`.
- **Every attribute macro inside `cfg_attr`, another one first:**
  `#[cfg_attr(not(feature = "x"), passthru)] #[cfg_attr(feature = "x", ulo_http::get("/a"))]`.
  The handler is gated on `not(feature = "x")`. Feature off: the unconsumed `__handler` message
  and the two E0599s. Feature on: the `outside_routes` message on `get`.
- **A helper whose attribute macros all sit inside `cfg_attr`:** a handler where the first one's
  predicate holds (decision 1, changed behaviour), failing there with the unconsumed `__handler`
  message and the two E0599s.
- **Two live transport attributes on one method**, gated or not: the first consumes `__handler`
  and the second reports `outside_routes`. Unchanged from before.
- **An enhancer or `#[meta]` inside an inner `#![cfg_attr(..)]` in the method body:** not read. An
  inner custom attribute is unstable, so rustc refuses it where the predicate holds.

Every case fails to compile; none mounts a handler wrongly.

## Verification

Scratch crate `cfgattr` (own `[workspace]`, path dependencies on `ulo`, `ulo-http`, `ulo-net`,
`ulo-tokio`, feature `x`, `#![deny(warnings)]`), run with the feature off and on, on rustc 1.98.1
and on 1.88. It wires a real `App` with two plain controllers and one generic one, binds
`ulo_http::Server` over a `Backend` that keeps the `AppService` it is handed, prints the mounted
handlers with their `Tag` metadata, and sends one request per route through `AppService::call`.
`Deny` is a guard answering `false` (403); `Fail` an interceptor returning `Err`, and `Teapot` an
error handler answering 418.

| Handler | Attributes | off | on |
| --- | --- | --- | --- |
| `/verb` | `cfg_attr(x, get)` | not mounted, 404 | mounted, 200 |
| `/guarded` | `get`, `cfg_attr(x, guards(http = Deny))` | 200 | 403 |
| `/tagged` | `get`, `cfg_attr(x, meta(Tag("tagged")))` | metadata `[]` | `["tagged"]` |
| `/intercepted` | `get`, `meta(Tag("plain"))`, `cfg_attr(x, interceptors(http = Fail), error_handlers(Teapot))` | 200, `["plain"]` | 418, `["plain"]` |
| `/value`, `/with` | `get`, `cfg_attr(x, guards(value = Deny))` / `guards(with = \|\| Deny)` | 200 | 403 |
| `/multi` | `cfg_attr(x, doc, get, guards(Deny), inline)`, `cfg_attr(x, cfg_attr(not(test), meta(..), allow(..)))` | 404 | 403, `["multi"]` |
| `/nested` | `cfg_attr(x, cfg_attr(not(test), get))` | 404 | 200 |
| `/off` | `cfg_attr(not(x), get)` | 200 | 404 |
| `/gated` | impl `cfg_attr(x, guards(http = Deny))`, method `cfg_attr(x, get)` | 404, assertion passes | 403 |
| `/gen` (generic `Gen<u8>`) | impl `cfg_attr(x, guards(http = Deny))`, method `cfg_attr(x, get, interceptors(Fail), error_handlers(Teapot))` | 404 | 418 |

Identical output on both toolchains. `/gen` answers 418 with the feature on: `Deny` refuses and
`Teapot`, the method-tier error handler, claims the `GuardRejected`.

Against known violations:
- The same scratch crate built with the feature on against the macros as they were before this
  change (`git stash` of `crates/`) fails with ten errors: five "#[get] goes on a method of a
  `#[routes]` impl", four "cannot find attribute" for `guards`, `interceptors`, `error_handlers`
  and `meta`, and the `Gated` key assertion "(no handlers)".
- Negative crate `neg`, one binary per case, built off and on, on 1.98.1 and 1.88:
  - `#[guards(htpp = Deny)]` on the impl: "`htpp` is not the key of any handler's transport in this
    impl (handlers: a)", both builds.
  - `#[cfg_attr(feature = "x", guards(htpp = Deny))]` on an ungated `get` handler: "`a` is a `http`
    handler, so an entry for `htpp` applies to nothing", spanned on `htpp`, both builds.
  - Ungated `#[guards(http = Deny)]` on an impl whose one handler is `cfg_attr(x, get)`: off, "`http`
    is not the key of any handler's transport in this impl (handlers behind `cfg`: a)"; on, compiles.
  - `#[ulo_http::get]` on a plain impl: the `outside_routes` message, both builds; inside
    `cfg_attr(x, ..)` on a plain impl: compiles off, the same message on.
  - `#[guards]` above `#[routes]`: the marker message, both builds; `#[cfg_attr(x, guards(..))]`
    above `#[routes]`: compiles off, the marker message on; the same on a plain impl's method.
  - `cfg_attr(x, guards(..))` and `cfg_attr(x, meta(..))` on a non-handler method of a `#[routes]`
    impl: "on `helper`, which carries no transport attribute", both builds.
  - With a passthrough attribute macro: `#[passthru] #[cfg_attr(x, get)]` and
    `#[cfg_attr(x, get)] #[passthru]` fail off and compile on;
    `#[cfg_attr(not(x), passthru)] #[cfg_attr(x, get)]` fails in both builds; a helper with
    `#[cfg_attr(x, passthru)]` compiles off and fails on; each with the messages under
    "Unsupported". `#[cfg_attr(x, passthru)] #[get]` and
    `#[cfg_attr(x, get)] #[cfg_attr(not(x), passthru)]` compile in both builds.
- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warnings both print are in `crates/ulo/src` (`private_interfaces` in `binding/`, dead code in
  `dependency/`, `graph/`, `lifecycle/`, `module/`, `redact.rs`, `timer.rs`), none in a file this
  change touches.

## Unrelated macro bugs found

None. The scratch crate is the first expansion of `#[routes]` and `#[ulo_http::get]` against the
real `ulo`; the plain, generic, value, closure, interceptor, error-handler and metadata paths all
compiled and dispatched without a change outside the `cfg_attr` work.
