# Divergences: area A (keys, sites and the resolver)

Every place area A's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what A wrote, and why. All await the user's
sign-off. Requests for other areas follow the entries.

## Entries

### 1. `short_type_name` keeps the last segment of every path

- **Design:** silent; `BUILD_PLAN.md` leaves the shortening to A. §10.1's sample prints
  `dyn Mailer`, `dyn Repo + Send + Sync` and `Dep<RequestHead>`, and one heading prints
  `fw_http::RequestHead`.
- **A:** every `a::b::C` run in a `type_name` is cut to its last segment, inside generic arguments
  too: `alloc::sync::Arc<my_app::db::PgPool>` reads `Arc<PgPool>`, and
  `dyn my_app::Repo + core::marker::Send + core::marker::Sync` reads `dyn Repo + Send + Sync`. A
  closure segment keeps the item enclosing it, as in `main::{{closure}}`. References, slices,
  tuples, function pointers and the `::Out` after a qualified path are copied as written.
- **Why:** it matches the sample's spelling everywhere but that one heading, which reads
  `RequestHead`. The cost is that two types whose last segments match, `a::Config` and
  `b::Config`, print alike in every diagnostic, `Key`'s `Debug` included. Their `TypeId`s still
  differ, and `KeyName::key()` compares exactly.

### 2. `Key` and `KeyName` display text

- **Design:** §3.1: `"PgPool"`, `"PgPool @ Replica"`, `"dyn Plugin (collection)"`.
- **A:** those three forms, both sides of `@` shortened by entry 1. `Key` writes the first two;
  `KeyName` adds ` (collection)` for a collection key. Both honour width and alignment, as in
  `{:<20}`, through `Formatter::pad`.
- **Why:** the design's forms. Padding lets a report align keys the way §10.1's sample aligns
  module names.

### 3. A collection nothing contributes to reads as empty

- **Design:** silent on `Many<T>` and `Resolver::entries` over a key with no contributions.
- **A:** an empty `Many` and an empty `Entries`, not `NotFound`, so `Option<Many<T>>` answers
  `Some` with no items. When nothing contributes and the reading module sees a single binding or
  an input under the key, the read is `WrongKind { expected: Collection, found: Single }`.
- **Why:** collections are app-wide and open to contribution, and a transport reads
  `entries::<AnyGuard<T>>()` on every call, where an app with no global guard is the common case.
  Whether `wire()` reports a `Many` site with no contributions is area C's decision; at runtime
  the read is empty either way.

### 4. A runtime lookup of a key with two sources

- **Design:** §8.2: "The root's table is free of ambiguity once wiring passes, so no lookup
  variant is needed for it." Silent on the other modules' tables, where the wiring pass reports
  an ambiguous key only for a site that reads it.
- **A:** `ModuleRef::get`, `exec.get` inside an execution opened in a non-root module, and
  `by_key` meeting such a key answer `LookupError::AmbiguousModule { module, candidates }`, with
  `module` the key's type name and `candidates` the modules exporting it. `Option<S>` propagates
  it.
- **Why:** `NotFound` would misdescribe a key two modules bind, and `Option<S>` would turn it into
  `None`. `AmbiguousModule` is the existing variant that names candidate modules. A dedicated
  variant is the cleaner shape; R4 asks area E for one.

### 5. The kind a `WrongKind` key carries

- **Design:** `WrongKind { key: KeyName, expected, found }`; silent on the kind inside `key`.
- **A:** the kind the key has in the graph, `found`. A collection read as a single prints
  `dyn Plugin (collection)`; a single read as a collection prints `PgPool`.
- **Why:** the name describes the binding as it exists, and `expected` already carries the read's
  kind.

### 6. `by_key` checks existence before type

- **Design:** `WrongType` for an erased key asked for a `T` it does not hold (§3.1, §10.2);
  silent on the order of checks.
- **A:** the key is located first and its type checked second, both before anything is built. A
  key no visible module binds is `NotFound` whatever `T` is. A wrong `T` on a bound key is
  `WrongType` and constructs nothing.
- **Why:** a missing key is the more basic fault, and a lookup bound to fail should not run a
  constructor first.

### 7. `WrongType` outside `by_key`

- **Design:** §10.2: "`WrongType` is reachable from one surface only, `Resolver::by_key::<T>(key)`".
- **A:** `dep`, `many`, `Entry::resolve` and a `Dep` read of an input answer `WrongType` when the
  stored instance does not hold the key's type after the binding's recorded coercion. With
  consistent records this does not happen. It can when records disagree: an override written for
  an `also_as` key that replaces the shared binding's recipe with a value of that key's type
  leaves reads of the primary key holding the wrong type.
- **Why:** rule 9 forbids a panic, and `WrongType` names the key and the type asked for.

### 8. What `ExecutionRequired` names outside a binding

- **Design:** `ExecutionRequired { key: KeyName }`; silent on the key for reads that are not
  bindings.
- **A:** `Ext<T>` names `T`, `ExecutionRef` names `ExecutionRef`, and an input names its own key.
  An input read with no execution is `ExecutionRequired`, not `NotFound`, so
  `Option<Dep<RequestHead>>` read outside an execution propagates it.
- **Why:** an input exists only inside an execution, and §3.2 lists `ExecutionRequired` among the
  errors `Option<S>` propagates.

## Requests for other areas

### R1. Area D, `execution/extensions.rs`: store inputs in the `Instance` shape

- **Item:** `Inputs::{insert, get, get_erased}`.
- **Request:** `insert` stores `instance_of(Arc::new(value))`, an `Arc<dyn Any>` wrapping
  `Arc<T>`, and `get::<T>` reads it back with `downcast_instance::<T>`.
- **Why:** `Resolver::dep_qualified::<T: ?Sized, Q>` reads an input through `get_erased` and has to
  produce an `Arc<T>` with `T` unsized in its signature. `downcast_instance` does that for the
  `Instance` shape. A bare `Arc<dyn Any>` holding `T` has no safe conversion, because
  `Arc::downcast` needs `T: Sized`. Stored any other way, every `Dep<T>` read of an input answers
  `WrongType`.

### R2. Area D, `app/shared.rs`: what `obtain` returns

- **Item:** `AppShared::obtain`.
- **Request (coordination):** `obtain` returns the binding's instance as its recipe builds it. It
  receives a `BindingId`, not the key read, so it applies no `also_as` coercion. The resolver
  widens by the key it looked up: a direct downcast first, then the contribution's `into_primary`
  or the `also_as` entry for the requested type, following an alias to its target. An `obtain`
  that widens a contribution to its collection type itself also works, because the direct
  downcast then succeeds. For an alias, `obtain` resolves the target with
  `graph.lookup(alias origin, target)`, the lookup the resolver's widening also uses; that
  recursion through `obtain` is async-fn recursion and needs a `Box::pin` (E0733).

### R3. Area B, `binding/mod.rs`: a coercion handed another type

- **Item:** `coercion::<T, U, F>`.
- **Request:** when the instance is not an `Arc<T>`, the closure returns it unchanged instead of
  panicking.
- **Why:** the resolver applies a `Coercion` only after a direct downcast to the requested type
  fails, and answers a failed second downcast with `WrongType` (entry 7). It picks an `AlsoAs` entry
  by its key's `TypeId` alone, since a keyed module's requalified export reaches the binding under
  another qualifier.

### R4. Area E, `error/mod.rs`: a variant for an ambiguous key, if the user prefers one

- **Item:** `LookupError`.
- **Request:** for the user's call on entry 4, a `LookupError::Ambiguous { key: KeyName, sources:
  Vec<ModuleName> }`, which the resolver would answer in place of `AmbiguousModule`. Until then,
  `AmbiguousModule.module` carries a binding key's type name as well as a module type's, and its
  `Display` text has to read sensibly for both.

### R5. Area C: what the resolver reads from the graph

- **Items:** `Graph::{lookup, collection}`, `Visible`, `FrozenBinding::{origin, record}`.
- **Request (confirmation):** the resolver relies on three properties. An `also_as` key and a
  requalified export each map to the original binding's `Visible::Binding(id)`. `collection`
  returns an empty slice for a key nothing contributes to. An alias record keeps
  `Recipe::Alias { target }`, with `target` resolvable in its origin module's table.
