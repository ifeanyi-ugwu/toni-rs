# Divergences: wave 2, agent M (the macros)

Every place M's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see. Each entry gives what the design says, what M wrote, and why. Requests for core files follow
the entries.

## Entries

### 1. The controller-level `value` refusal: message, scope and span

- **Design:** §7 and §12: a controller-level `value = expr` is a compile error, "declare it per
  method, or bind it by type for shared state". This supersedes G entry 3.
- **M:** `#[routes]` refuses every `value` entry among the impl's `#[guards]`, `#[interceptors]`
  and `#[error_handlers]`, one error per entry. The message is "a `value` entry on the impl would
  be built once per handler rather than shared; declare it per method, or bind it by type for
  shared state". The transport-scoped form on the impl, `#[guards(http(value = ..))]`, is refused
  with the unscoped one. The error spans the entry from its first token, the scope key or `value`,
  to the end of the expression; in the scoped form the closing parenthesis falls outside the span.
- **Why:** the signed text says what to do and not what is wrong, and the reason clause in front
  of it supplies the cause. A scoped value is mounted by each handler of that transport, which is
  the same per-handler build the refusal exists for.

### 2. A role key is recognised by how it is written

- **Design:** §7 and §14.18: a global contribution to a role goes through `m.enhancer::<R>()`, and
  `contribute` records a provider contribution whatever its key. The design does not say how
  `#[module]` tells a role key from any other collection key.
- **M:** in `into K: [A, B]`, `K` is a role key when it is a path whose last segment is `AnyGuard`,
  `AnyInterceptor` or `AnyErrorHandler`, or a `dyn` type with an `ErasedGuard`,
  `ErasedInterceptor` or `ErasedErrorHandler` bound, looking through parentheses and the invisible
  group a `macro_rules` `$t:ty` produces. A role key lowers to `m.enhancer::<K>()`, any other key
  to `m.contribute::<K>()`.
- **Limits:**
  - An alias of a role key under another name, `type HttpGuards = AnyGuard<Http>`, lowers to
    `contribute`. Its entries are provider contributions, scoped as providers (§7), so an `Auto`
    enhancer in one that needs an execution is refused at `wire()` (§6.2) instead of being
    inferred per execution.
  - A type the user named `AnyGuard`, or `dyn ErasedGuard<Http> + Send`, which is a different type
    from the role key, lowers to `enhancer` and fails its `Role` bound at compile time. The failure
    is loud; request R2 gives it a message.
- **Why:** a proc macro sees tokens, not types. Reading the last segment accepts any path to the
  three aliases, `AnyGuard<Http>` and `ulo::AnyGuard<Http>` alike; `use ulo::AnyGuard as G` is an
  alias in the sense of the first limit.

### 3. `X as <role key>` in a providers list is an error

- **Design:** silent.
- **M:** `AuthGuard as AnyGuard<Http>`, with the role key recognised as in entry 2, is a span error
  on the role key: "a role key takes contributions, and `as` binds a single instance; a global
  enhancer is written `into AnyGuard<Http>: [AuthGuard]`".
- **Why:** `as` lowers to `also_as`, a second single key. `dispatch` walks
  `Resolver::entries::<AnyGuard<T>>()`, one entry per contribution (§7), so a guard bound that way
  is not among them, and beside a contribution to the same key it is a single/collection mix
  (§4). Either way the entry reads as a global guard and does not guard.

### 4. No `#[module]` spelling for a contribution by value or by factory

- **Design:** §4's `into dyn Plugin: [MetricsPlugin, TracingPlugin]` lists types. §7 writes a
  global interceptor by value against the value API,
  `m.enhancer::<AnyInterceptor<Rpc>>().value(Arc::new(Tracing::default()))`, and gives it no
  `#[module]` form.
- **M:** unchanged: an `into` list takes types only, for role keys and other collection keys
  alike. A module that needs a contribution by value or by factory implements `Module` by hand.
- **Why:** two spellings fit and the choice is a grammar decision:
  - the providers-list rule inside `into` lists: a bare path is a type, anything else an
    expression lowered to `.value(expr)`, which takes `Arc<K>`, so the user writes `Arc::new(..)`;
  - the enhancer-attribute grammar, `into AnyInterceptor<Rpc>: [value = Tracing::default()]`, with
    the macro writing the `Arc::new`.

## Requests for core files

### R1 (W). Doc comments that still show `contribute` for a global enhancer

- `crates/ulo/src/transport/mod.rs:105`: "The role key for global guards:
  `m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a)`."
- `crates/ulo/src/binding/contribute.rs:21-26`: "a binding under a role key such as
  `AnyGuard<Http>` registers that role", and the example
  `m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);`.

Under §7 both are written through `m.enhancer::<AnyGuard<Http>>()`, and a contribution through
`contribute` is a provider contribution whatever its key.

### R2 (W). A diagnostic on `Role`

`#[module]` writes `m.enhancer::<K>()` for any `K` written like a role key (entry 2). When `K` is
not one, the error is the `Role` bound's. Suggested:

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a role key",
    label = "a global enhancer is contributed under a role key",
    note = "the role keys are `AnyGuard<T>`, `AnyInterceptor<T>` and `AnyErrorHandler<T>` for a `T: Transport`"
)]
```

### R3 (W). The signature the expansion relies on

The expansion is `m.enhancer::<K>().provide::<A>(|a| a);`, with `m` the `&mut ModuleDef<'_>`
parameter of `register`. It needs `ModuleDef::enhancer<R: Role + ?Sized>(&mut self) ->
Contribute<'_, R, ()>` and `Contribute::provide` reachable for `R = AnyGuard<T>`, as the brief
states. The expansion names no other new item: neither `Role` nor `enhancer` is written as a path.
