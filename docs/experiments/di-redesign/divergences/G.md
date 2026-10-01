# Divergences: area G (the macros)

Every place area G's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what G wrote, and why. All await the user's
sign-off. Requests for other areas, and the changes made to code the spine wrote in G's files,
follow the entries.

## Entries

### 1. A provider that is not `Construct` is a trait-bound error, not a macro error

- **Design:** §10.3 lists, among macro errors spanned with `syn::Error`, "a provider in
  `#[module]` that doesn't implement `Construct` together with the hint 'add #[injectable] or
  bind it with a factory'".
- **G:** `UserService` in a providers list lowers to `m.provide::<UserService>()`, spanned at the
  entry, so the unsatisfied `T: Construct` bound is reported on the entry. The hint is not there:
  `Construct` carries no `#[diagnostic::on_unimplemented]` (request R1).
- **Why:** a proc macro sees tokens, not trait impls, so it cannot tell whether a type implements
  `Construct`. The hint can only come from the trait's own diagnostic. A wrapper trait in
  `__private` with a blanket impl over `Construct` would not reliably surface its own message,
  since rustc reports the unsatisfied leaf bound.

### 2. A missing role names the handler in a note, not in the message

- **Design:** §7: "a compile error at the attribute naming the handler", with a message like
  "`AuthGuard` is not a guard for `Rpc`, needed by `get_rpc`; implement `Guard<Rpc>`".
- **G:** each entry is registered through a local fn named after the handler, whose bound is the
  role for the handler's transport. The error is reported on the entry inside the attribute with the role
  trait's own message, "`AuthGuard` is not a guard for `Rpc`", its note "implement `Guard<Rpc>`
  for `AuthGuard`", and rustc's note "required by a bound in `get_rpc`", which points at the
  handler's name.
- **Why:** an `on_unimplemented` message can name only types, never a handler, and a per-handler
  wrapper trait meets the blanket-impl problem of entry 1. A fn named after the handler is the one
  place rustc prints a name the macro chooses. The spine's `__private::assert_guard` (and its two
  kin) asserted the role a second time beside `spec.guard::<G>()`, which would report each missing
  role twice; the local fn makes the registering call itself carry the bound.

### 3. A controller-level `value = expr` is built once per handler

- **Design:** §7 and [26]: "by value: built once, shared".
- **G:** on a method the value is built once when `mount` runs and shared by every call to that
  handler. On the impl it applies to every handler, and each handler's mount code evaluates the
  expression, so a controller with three handlers builds three values. A rate limiter declared
  there limits each handler separately.
- **Why:** each handler's tiers are built by its own `__ulo_mount_<name>`, written by the
  transport, with no channel from `Controller::mount` to pass a shared value through, and
  `EnhancerSpec<T>` stores the erased value per transport. A guard whose state every handler shares
  is declared by type and bound as a singleton, which is the design's sharing mechanism. The public
  docs on `#[guards]` say so.

### 4. A method-level entry scoped to another transport is an error

- **Design:** silent on transport-scoped entries on a method.
- **G:** `#[guards(http = AuthGuard)]` on an RPC handler is a compile error on the key: "`get_rpc`
  is a `rpc` handler, so an entry scoped to `http` applies to nothing; write it unscoped, or on the
  handler it belongs to".
- **Why:** a method has one transport, so a scoped entry on it is redundant when it matches and
  dead when it does not. Dropping a guard written on the handler itself is the failure [25] exists
  to prevent.

### 5. A controller-level entry scoped to a key no handler has is dropped without an error

- **Design:** silent.
- **G:** `#[guards(htpp = AuthGuard)]` on an impl, a misspelled key, matches no handler and applies
  to nothing, with no error.
- **Why:** `#[routes]` expands before the transport attributes and does not know which keys its
  handlers have; each `__enhancer_specs!` call sees one handler. Closing it needs the transport
  protocol to expose each handler's key to the `impl Controller` that `#[routes]` writes, for
  example a `const __ULO_KEY_<name>: &str` written by the transport and compared in a `const`
  assertion. That is a change to the protocol transport crates write against, left for the
  transport race. Restricting keys to a fixed list in the macro would refuse a user-written
  transport, which §6.3 allows.

### 6. `with` closures: an `async` one is taken as written, and a return type is kept

- **Design:** §7 writes `with = |u: Ext<CurrentUser>| RoleGuard::require(u, Role::Admin)`;
  spine entry 42 wraps the body as `async move { .. }`.
- **G:** the body is wrapped unless the closure is already `async |..| ..` or its body is already
  an `async` block, which are handed to `*_with` as written. `|u: X| -> RoleGuard { .. }` keeps its
  type on a binding inside the block, `async move { let built: RoleGuard = { .. }; built }`.
- **Why:** wrapping an already-asynchronous closure would make its output a future, which is not
  a guard, and the error would point at a role the user did implement. An `async` block cannot
  carry a return type, so the type moves inside it.

### 7. In a providers list, a bare path is a type

- **Design:** §4 lists `Type`, `Type as dyn Trait`, `expr?` and `expr` without saying how a
  path that could be either is read.
- **G:** an entry that parses as a plain type path followed by `,`, `as` or the end of the list is
  a type: `UserService`, `db::Pool<Pg>`. A path with call parentheses, `Config::default()`, is an
  expression. A constant, `LIMITS` or `Limits::DEFAULT`, reads as a type and fails at
  `provide::<..>()`; it is bound by value with a block, `{ LIMITS }`. The `#[module]` docs say so.
- **Why:** the tokens of a unit path are the same for a type and a constant, and the type reading
  is the one §4 leads with. syn parses `Config::default()` as a type (the `Fn()` sugar), so a path
  with parenthesized arguments is excluded from the type reading.

### 8. Any attribute outside the inert list makes a method a handler

- **Design:** §7 and spine entry 41: a method carrying a transport's handler attribute is a
  handler.
- **G:** as the spine's `#[routes]` doc states it: a method carrying any attribute other than the
  language's own (`doc`, `allow`, `warn`, `deny`, `expect`, `cfg`, `cfg_attr`, `inline`,
  `must_use`, `deprecated`, `track_caller`) and the enhancer attributes is a handler. A helper
  method in a `#[routes]` impl carrying, say, `#[tracing::instrument]` is taken for a handler and
  fails with "this handler's transport attribute did not read its enhancers". The `#[routes]`
  docs tell the user to put such helpers in a separate impl block.
- **Why:** `#[routes]` expands before the transport attributes and cannot tell a transport's
  attribute from another crate's. Enhancer attributes on a method that is not a handler are a
  compile error naming the method, rather than the generic marker error.

### 9. A field or parameter that is not a site reports twice

- **Design:** §5 and §12: such a site "produces the `Site` diagnostic".
- **G:** the error is reported on the field or parameter twice: once where `Construct::sites`
  declares it, once where `Construct::construct` reads it. Both carry the `Site` message and point
  at the site's type. An `Ext` or `ExecutionRef` in an explicit singleton reports once, at the
  declaration.
- **Why:** `sites` and `construct` are separate functions, and each needs `T: Site` to compile.
  Declaring and asserting in one `__private::field` call keeps it to two; the spine's separate
  declaration and `assert_site` made it three.

### 10. The order of a generated `register`

- **Design:** §4's expansion writes imports, then providers, then exports. Silent on controllers
  and secrets.
- **G:** `global`, the `Secret<_>` fields, the imports, the providers, the controllers, the
  exports. Within each list the order written.
- **Why:** a controller is a binding, so its place fixes its declaration index, the connect-order
  tie-break inside a module (§9.2). Placing controllers after providers means a module's services
  are declared before what dispatches to them.

### 11. The `__handler` protocol as built

- **Design:** spine entry 41 defines the protocol; `BUILD_PLAN.md` leaves the scope keys to G
  until transport crates exist.
- **G:** for transport authors:
  - `#[routes]` appends `#[::ulo::__private::__handler(name, controller(..), method(..))]` as the
    method's last attribute. A transport attribute finds it among the attributes it receives,
    wherever its own attribute sits, removes it, and passes its tokens unchanged to
    `::ulo::__private::__enhancer_specs!(<Transport>, "<key>", <tokens>)`.
  - `__enhancer_specs!` is a block expression evaluating to `(EnhancerSpec<T>, EnhancerSpec<T>)`,
    controller tier then method tier, each built in a local, as F's request R4 asks.
  - The transport writes `fn __ulo_mount_<name>(m: &mut ::ulo::Mount<'_>)`, `<name>` being the
    method's identifier without `r#`; `impl Controller` calls these in method order.
  - A scope key is any identifier, matched against the string literal by name. `value` and `with`
    cannot be keys. No list of known keys is checked (entry 5).
- **Why:** a transport attribute that does not sit last still receives `__handler`, since every
  attribute after it is part of its input; a raw identifier cannot be pasted into a fn name.

## Requests for other areas

### R1 (B). `crates/ulo/src/construct.rs`: a diagnostic on `Construct`

Add to `trait Construct`:

```rust
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a type the container builds",
    label = "this is bound with `provide`, which builds it through `Construct`",
    note = "add #[injectable] to the type, or bind it with a factory: `m.singleton(|..| async { .. })`"
)]
```

It carries §10.3's hint for a providers entry that does not implement `Construct` (entry 1).
`#[module]` spans the `provide::<T>()` call at the entry, where the message is reported.

## Changes to code the spine wrote in G's files

None changes a contract another area calls.

- `__private::assert_site` is replaced by `__private::field` and `__private::param`, which declare
  the site and assert `Site + AllowedIn<Sc>` in one call (entry 9). A tuple struct's field is
  declared with `field("<index>")` rather than a positional `site`, so the wiring report names the
  index even with a defaulted field between two sites.
- `__private::{assert_guard, assert_interceptor, assert_error_handler}` are removed; the role is
  checked by a local fn per entry (entry 2).
- `__private::IntoConstructed` has an `on_unimplemented` naming the two shapes a constructor may
  return.
- `__private::factory`'s probe methods carry `#[track_caller]`, so the binding records the
  providers entry's location, not a line in `__private.rs`.
- `ConstructImpl` gained `construct_span`, which spans the generated `construct` at the
  constructor. The parameters of the generated `sites`, `construct` and `register` are
  mixed-site, so a constructor parameter named `r` cannot shadow the resolver.
- `routes::Handler` lost `method_tier`, which nothing read, and `shared::attrs::has` is removed
  for the same reason. `shared::{combine, check_factory_params, ulo_at, sites_param,
  resolver_param}`, `shared::sites::bindings` and `module_attr::module_def_param` are added.

**R1 applied by the orchestrating session** (area B had finished): `Construct` carries
`#[diagnostic::on_unimplemented(message = "`{Self}` is not a type the container can construct",
label = "the container cannot build this", note = "add #[injectable] to the type, or bind it with a
factory")]` in `crates/ulo/src/construct.rs`. The wording is unverified until the first compile.
