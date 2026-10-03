# Divergences: race 2a, area MX (the macros and the handler protocol)

Every place MX's code departs from `transports/DESIGN.md` or `DESIGN.md`, or fills what they and
the spine leave open. Each entry gives what the design says, what MX wrote, and why. All await the
user's sign-off. The points BUILD_PLAN_2A.md leaves to MX's log follow the entries, then the
review of the hand-moved `__handler` grammar, then requests.

Files changed: `crates/ulo-handler-codegen/src/{params,body,reply,keys,shared,emit}.rs`,
`crates/ulo-handler-codegen/src/protocol/mod.rs` (doc), `crates/ulo-macros/src/routes/mod.rs`,
`crates/ulo-macros/src/enhancers/mod.rs`, `crates/ulo-macros/src/lib.rs` (doc). No `todo!()` is
left in either crate.

## Entries

### 1. A handler with a type or const parameter is refused

- **Design:** §2.3 mentions a handler with a type or const parameter only to exempt its opaque
  return types from `+ use<>`. The spine's `HandlerSig::is_generic` doc says such a handler's
  parameter checks are "read from the mount function rather than from a free constant".
- **MX:** `MountFn::emit` writes, for such a handler, `compile_error!` spanned at the handler's
  name: "`get` declares a type or const parameter; a handler is not generic, since the generated
  call names each parameter's type outside the method". The three owed items are still written,
  with an empty mount function and `__ULO_CHECKS_<name>: () = ()`, so `#[routes]`'s reads resolve
  and the refusal is the one error.
- **Why:** the call closure, the dependency reads and the checks constant all name each parameter's
  type inside the mount function or the impl, where the method's own type parameters are not in
  scope, and nothing at the call could infer them. Left alone, the expansion fails with "cannot find
  type `T` in this scope" inside generated code. The spine's doc sentence described a mechanism
  that cannot exist and is corrected.

### 2. A generic controller's handlers capture the impl's parameters: `use<T, ..>`

- **Design:** §2.3: the transport attribute appends `+ use<>` to every opaque type in a handler's
  return position. Silent on a controller with type or const parameters.
- **MX:** `#[routes]`, on an impl with a type or const parameter, rewrites each handler's opaque
  return types to `+ use<T, N, ..>` naming the impl's type and const parameters (lifetimes
  omitted), under the same exemptions as entry 3. The transport attribute's
  `rewrite_opaque_returns` then finds a `use<..>` and leaves the type as written. The rewrite is a
  new public function, `ulo_handler_codegen::reply::rewrite_opaque_returns_in(sig, impl_generics)`;
  `rewrite_opaque_returns(sig)` is that function with empty generics.
- **Why:** `use<..>` must name every type and const parameter in scope, the impl's included, so
  `+ use<>` on a generic controller's handler does not compile. The transport attribute sees the
  method alone; `#[routes]` sees the impl. Passing the impl's generics through `__handler` would
  change the protocol's grammar, a frozen contract.

### 3. What "names a lifetime" means for the `use<>` rewrite

- **Design:** §2.3: an opaque type that "already names a `use<..>` or a lifetime" is left as
  written.
- **MX:** an opaque type is left as written when a bound is a `use<..>`, or a bound names a
  lifetime other than `'static` anywhere inside it (`+ 'a`, `Item = &'a str`, `'_`), or holds a
  reference with an elided lifetime (`Item = &str`). Lifetimes under a `for<..>` bound and elided
  ones in `Fn(&str)` or `fn(&str)` sugar are late-bound and do not count. Each nested opaque type,
  `impl Stream<Item = impl Serialize>`, is judged and rewritten on its own.
- **Why:** `'static` is no capture, and `Item = &'static str` with `use<>` compiles, so counting it
  would leave a `&self` handler's reply borrowing the controller. An elided reference in return
  position takes an input lifetime, which `use<>` would refuse to drop.

### 4. `emit::mount_param_ident()`, for the X15 call

- **Design:** §4.1: each `fw_ws::message` mount function calls
  `<Self as GatewayConfig>::mount_gateway(m)` under `Mount::once::<K>(..)`.
- **MX:** a new public function `emit::mount_param_ident() -> Ident` names the mount function's
  `&mut Mount<'_>` parameter (`__ulo_m`, mixed-site), as `call_ident()` names the call closure.
  A WebSocket transport's `handler_value` is then a block that calls
  `<m>.once::<Self>(|m| <Self as GatewayConfig>::mount_gateway(m))` before building its value.
  `MountFn::handler_value`'s doc says so.
- **Why:** `MountFn`'s frozen fields give a transport no statement slot in the mount function, and
  the handler value is the one expression a transport writes there. A field for statements would
  reshape a cross-area contract. Nothing in race 2a calls it.

### 5. The receiver forms accepted

- **Design:** §2.3: `&self`, or `self: Arc<Self>`.
- **MX:** `&self`, `&'a self` and `self: &Self` read as `Receiver::Ref`; `self: Arc<Self>` and
  `mut self: Arc<Self>`, with any path whose last segment is `Arc` and whose one argument is `Self`,
  as `Receiver::Arc`. Anything else, `&mut self` and `self` included, is a span error on the
  receiver: "a handler takes `&self`, or `self: Arc<Self>` for a reply that outlives the call".
- **Why:** `self: &Self` is `&self` spelled out. The `Arc` test reads a spelling, which the spine's
  `todo!` text specified: an alias of `Arc` is refused, and the generated call passes
  `::ulo::Dep::into_arc(..)`, which is `std::sync::Arc<Self>`.

### 6. The call extracts every parameter before resolving the controller

- **Design:** DESIGN §7 step 3 builds the interceptors, then the controller, then runs the handler;
  silent on where parameter extraction sits relative to the controller.
- **MX:** as the spine's `emit` module doc lays out: parameters extract in order, then
  `controller::<Self, T>(&cx)` resolves the controller, then the method runs. A failed extraction
  builds no per-execution controller.

### 7. Not filled: a handler behind `#[cfg]`

- **Design:** silent.
- **MX:** unchanged from the spine. An attribute macro receives its item with inner `#[cfg]`
  attributes unevaluated, so `#[routes]` treats a `#[cfg(feature = "x")]` method as a handler and
  writes references to its `__ULO_KEY_*`, `__ULO_CHECKS_*` and `__ulo_mount_*`. With the feature
  off, the method and its transport attribute are stripped and those references fail to resolve.
- **Proposed fix, needing sign-off:** `#[routes]` copies each handler's `cfg` attributes onto its
  checks read and its mount call, and the key assertion becomes a `const` block of `#[cfg]`-gated
  statements rather than one `key_in` over an array literal. Both need the handlers' `cfg`
  attributes in `keys::assertions`, whose public signature takes bare identifiers.

## Points left to MX's log

### Spans

| Item | Spanned at |
|---|---|
| `__ULO_KEY_<name>` | the whole item at the handler's name |
| `__ULO_CHECKS_<name>` | its name at the handler's name; each pairwise `assert!` at the pair's second parameter's type |
| `__ulo_mount_<name>` | its name at the handler's name; each shared value's role bound at the handler's name, so a value lacking the role fails E0277 at `Controller::mount`'s call, which `#[routes]` spans there too |
| a parameter's extraction and its dependency read | the parameter's type, where `Param`'s `on_unimplemented` lands |
| the method call inside the call closure | the method's name |
| `Metadata::controller(..)` / `method(..)` | each `#[meta]` expression |
| `Arc::new(expr)` in `Controller::mount`, and its `*_arc` registration | each impl-level `value` expression |
| the free key assertion, or the generic form's associated constant and its read in `mount` | the key token |
| the free `__ULO_CHECKS_<name>` read, or the generic form's read in `mount` | the handler's name |
| a generic handler's `compile_error!` | the handler's name |

The probe call, the call closure, the dependency block and the metadata block are at the call
site; the locals they bind (`__ulo_cx`, `__ulo_argN`, `__ulo_this`, `__ulo_out`, `__ulo_meta`,
`__ulo_dependencies`, `__ulo_m`, `__ulo_shared`, `__ulo_call`) are mixed-site, so no name in a user
expression interpolated beside them can shadow or reach them.

### Message texts

- Key assertion: "`htpp` is not the key of any handler's transport in this impl (handlers: get,
  get_rpc)", the design's sample; an impl with no handlers reads "(no handlers)". Handler names and
  the key are printed without `r#`.
- Body consumer: "`user` and `login` both consume the body; a handler reads the body once", the
  design's sample. A parameter bound by a plain identifier is named by it, without `r#`; a
  destructuring pattern is named by its tokens as `to_string` prints them (`Path (id)`), with `{`
  and `}` doubled, the message being `assert!`'s format string.
- Receiver and generic handler: entries 5 and 1.

### The generic-controller form of the checks constant

A controller with any generic parameter, a lifetime included, takes the generic form, since a free
constant cannot name its type. Per scoped key, `#[routes]` adds the associated
`const __ULO_KEYS_CHECK_<key>: () = assert!(key_in(..), "..");` and writes no free constant.
`Controller::mount` opens with `let () = Self::__ULO_KEYS_CHECK_<key>;` per key and
`let () = Self::__ULO_CHECKS_<name>;` per handler, before the shared values are built. Both
evaluate when `mount` is instantiated for a concrete type, as probe P21d_fails shows.

## Review of the hand-moved `__handler` grammar

`#[routes]` writes `__handler` through `HandlerTokens::to_tokens` and `EnhancerAttr`/`Entry`'s
`ToTokens`; every transport reads it through their `Parse` impls. Checked by reading:

- Every entry form prints to tokens its parser reads back to the same form: `Type`, `key = Type`,
  `value = expr`, `with = closure`, `with(<scope>) = closure`, `key(value = expr)`,
  `key(with = ..)`, `key(with(<scope>) = ..)`. A scope word prints from `ScopeArg::word` with its
  span.
- `Token![=]`'s peek also matches `==` and `=>`; both parsers exclude them before taking a
  `key = ..` form.
- `meta(controller(..), method(..))` round-trips with either list empty; `parse_args` refuses
  leftover tokens.
- A handler's raw identifier round-trips through `Ident::parse_any`, and every generated name uses
  its `unraw` text.
- An impl-level `value`'s position is counted the same way in `shared::shared_values` and in
  `__enhancer_specs!`'s own counter: every `value` entry across the impl's three attributes in the
  order written, entries scoped to other transports included.
- The mixed-site `__ulo_shared` the transport attribute declares passes through
  `__enhancer_specs!` as an `Ident` with its span, so the `Arc::clone(&__ulo_shared.0.N)` it
  writes resolves to the mount function's parameter.

One change: `__enhancer_specs!`'s shared-value statement wrote `#shared.0.#index` in its template,
where the lexer reads `0.` as a float literal, relying on rustc's parser to split a float token in
field position. Both fields are now interpolated `Index` tokens, which removes the dependence.

The providers grammar (`with`, `with(<scope>)`, the bare closure, `K: with`) was checked against
DESIGN §4 and the core's `__private::factory` and `ModuleDef` signatures; it needed no change
beyond the spine's entries 24 and 25.

## Requests

None. Notes for HM, the one area calling into MX in race 2a:

- `MountFn::emit` refuses a generic handler itself, so the HTTP attribute needs no check of its own.
- `handler_value` names `emit::call_ident()`; the mount function binds it to `call_closure`'s
  closure before evaluating the value, so the attribute does not call `call_closure` as well.
- On a generic controller, `#[routes]` has already written `use<T, ..>` before the HTTP attribute
  runs; `rewrite_opaque_returns` leaves those types as written.
