# Divergences: wave 5, core (the scope of a contribution declared by closure)

Every place the core's code departs from the brief, or fills a gap a user would see. Each entry
gives what the brief says, what the core wrote, and why. Requests for other agents' files follow
the entries.

The decision is RESPONSE.md's fifteenth response: `with = closure` in an `into` list takes its
scope from the contribution's role, decided at freeze.

Files: `binding/contribute.rs`, `binding/mod.rs`, `graph/scopes.rs`, `graph/wire.rs`, `scope.rs`.

## Entries

### 1. `Contribute::with` and `Contribute::try_with`

- **Brief:** two methods on the impl both marks share, shaped like `singleton` and
  `try_singleton`, both `#[track_caller]`, recording "declared by closure" rather than a scope,
  with the generic parameter order of the methods they mirror.
- **Core:** on `impl<'m, U, Q, M> Contribute<'m, U, Q, M>`, which the `Plain` and `Enhancer`
  builders share.
  - `with<Args, F>(self, factory: F, coerce: impl Fn(Arc<F::Output>) -> Arc<U> + ..)` returns
    `Handle<'m, F::Output, Contribution<Auto, Open>>`.
  - `try_with<Args, F, T, E>(self, factory: F, coerce: impl Fn(Arc<T>) -> Arc<U> + ..)` returns
    `Handle<'m, T, Contribution<Auto, Open>>`.
  - The bounds are `singleton`'s and `try_singleton`'s. The recipe is `Recipe::Factory`, as
    theirs is.
  - The record carries `scope: ScopeKind::Auto` and `scope_by_role: true` (entry 2).
  - `push_factory` was split into `factory_record`, which builds the record, and the push.
    `push_by_role` builds the record the scoped methods build, sets the flag and pushes it.
- **Checked:** `__private.rs` already calls `with::<Args, F>(factory, coerce)` and
  `try_with::<Args, F, T, E>(factory, coerce)`, and `cargo check -p ulo` compiles both calls.

### 2. "Declared by closure" is a flag beside `ScopeKind::Auto`, not a `ScopeKind` variant

- **Brief:** a `ScopeKind` variant, with an arm in every exhaustive match, or a flag beside
  `ScopeKind::Auto` with the reason logged.
- **Core:** `BindingRecord::scope_by_role: bool`, `false` from `BindingRecord::new`. While it is
  set, `scope` holds `Auto`. Freezing writes the scope and clears the flag (entry 3).
- **Why a flag:** `ScopeKind` is public. It is re-exported at the crate root, is the type of
  `Scope::KIND`, and is the `declared` field of the public `WiringError::ScopeViolation`. It is
  not `#[non_exhaustive]`. The state exists only between `register` and the end of freeze, so a
  variant for it is one no public value ever carries. A downstream exhaustive `match` would still
  need an arm for it, and adding a variant to an exhaustive public enum is a breaking change.
  Inside the crate, `needs_execution` and `check_scopes` would each carry an arm that never runs.
  The flag keeps the state crate-private.
- **Why `Auto` while the flag is set:** it is the scope the handle names (entry 4), and
  `factory_record` writes `S::KIND` for the handle's marker. No pass reads it before freeze
  resolves it: the only pre-freeze reader of `scope` is the override split in `replace_recipe`,
  which handles single bindings, and a contribution is never one.

### 3. The scope is resolved in place at the end of freeze

- **Brief:** after `mark_role_contributions`, turn each record's scope into `PerExecution` for an
  enhancer or `Singleton` for a provider, before the needs-execution and scope passes, preferably
  in place; a lazy load resolves its own new records the same way.
- **Core:** `scopes::resolve_closure_scopes(graph, first_binding)`, called by `freeze` on the
  line after `mark_role_contributions`. For each flagged binding from `first_binding` on, it
  writes `record.scope` from `binding.role` and clears the flag.
  - `Role::Enhancer` gives `ScopeKind::PerExecution`. `Role::Provider` and `Role::Controller` give
    `ScopeKind::Singleton`. A contribution never has the controller role; the arm exists for the
    exhaustive match.
  - Nothing after freeze sees a flagged record. `needs_execution`, `connect_order`, `connect` and
    `obtain` read `record.scope` or `effective` as before and are unchanged.
- **Why the role is final there:** `scopes::assign_roles`, the one role pass after freeze,
  resolves `EnhancerSpec` keys through the visibility tables. Those hold no collections, so it
  marks single bindings only.
- **Lazy load:** `wire_lazy` freezes through the same `freeze`, which resolves the load's new
  records with the same rule. Base records were resolved by the base wiring and are skipped. A base
  contribution's role can still move in a lazy load, and the move only happens in a load that
  fails:
  - A base contribution moves to the enhancer role only when the load adds its key's type to the
    role-key set, which only an `enhancer` contribution does, and that builder has no `qualified`.
  - An unqualified base contribution under that type is in the same collection, and `refusal`
    refuses the load as `LoadRefusal::Contribution` before freezing.
  - A qualified one is reported by `mark_role_contributions` as `QualifiedRoleContribution`.

### 4. The handle marker is `Auto`

- **Brief:** choose the marker that makes hooks and `.ready` behave sensibly; `Auto` is the
  candidate.
- **Core:** `Contribution<Auto, Open>`. `Auto` is `HookCapable`, so `.ready`, the four `on_*`
  hooks and `.timeout`/`.unbounded` exist on the handle.
  - On a provider contribution, resolved to `Singleton`, they run as on any singleton.
  - On an enhancer they are refused at `wire()` as `HooksOnPerExecution` (entry 5).
- **Rejected:**
  - `Singleton` would name a scope the enhancer case does not have.
  - `PerExecution` would refuse hooks at compile time on a provider contribution, which is a
    singleton.
  - A new marker needs a `Scope::KIND`, which needs the `ScopeKind` variant entry 2 rejects.

### 5. `HooksOnPerExecution` no longer requires the declared scope to be `Auto`

- **Brief:** with the `Auto` marker, hooks on a contribution that becomes a per-execution enhancer
  are refused at `wire()` by the existing `HooksOnPerExecution` rule.
- **Gap:** the existing rule tested `record.scope == ScopeKind::Auto` and `effective ==
  PerExecution`. Entry 3 writes `PerExecution` into the record, so the rule did not fire as
  written, and a hook on an enhancer declared by closure would have been accepted and never run.
- **Core:** `check_scopes` now tests `effective == Effective::PerExecution` and a hook or a
  readiness check, whatever the declared scope.
- **No existing record changes outcome.** An explicitly execution-scoped binding carries neither
  hooks nor a check, so only `Auto` bindings and enhancers declared by closure reach the new test:
  - Closure hooks and `.ready` need `HookHost`, which needs a `HookCapable` scope.
  - Trait hooks need the hook traits' `Construct<Scope: HookCapable>` supertrait.
  - An override replaces a recipe and keeps the original's scope and hooks.
- **Rejected:** keeping a mark on the record after freeze so the old test could see it. The brief
  prefers that no pass after freeze sees the declared-by-closure state.
- **Limit:** the rendered text is unchanged: "lifecycle hooks on `X` in M, which the scope pass
  inferred per-execution", help "hooks run on singletons only; move them to a singleton, or remove
  what makes `X` need an execution". The help's second clause is wrong advice for an enhancer
  declared by closure, which is per execution whatever it reads. `error/wiring.rs` is not this
  agent's (request R1).

### 6. The provider case reuses the singleton refusal and its help

- **Brief:** the provider case gives the refusal any singleton provider needing an execution gets.
- **Core:** a provider contribution declared by closure is a `ScopeKind::Singleton` record when
  the scope pass runs, so `check_scopes` reports `ScopeViolation { declared: Singleton, .. }` with
  the path, the entry a `singleton(..)` contribution gets.
- **Limit:** that entry's help reads "declare `X` #[injectable(execution)], or inject a factory".
  A closure contribution has no `#[injectable]`, and the `into` list has no per-execution
  spelling until `with(execution) = ..` exists. The fix for a provider collection is
  `.execution(..)` on the value API. A `singleton(..)` contribution prints the same help today
  (request R1).

### 7. Docs updated with the change

- `Contribute`'s struct doc gains the role rule for `with`; `with` and `try_with` carry their own
  docs.
- `Recipe::Factory` lists `with`.
- `scope.rs`: the module doc, `Auto`, and `HookCapable` name the declared-by-closure case.
- `graph/scopes.rs`: the module doc's table row "Auto with hooks, inferred per-execution" became
  "With hooks, built per execution", followed by a paragraph on contributions declared by
  closure. `check_scopes`'s doc states the invariant entry 5 relies on.
- `graph/wire.rs`: `freeze`'s doc names `resolve_closure_scopes`.

## Requests for other files

- **R1, `error/wiring.rs`:** `HooksOnPerExecution`'s doc ("hooks on an `Auto` binding the scope
  pass inferred per-execution") does not cover an enhancer declared by closure (entry 5). Its
  help's second clause, "or remove what makes `X` need an execution", applies to an `Auto` binding
  only. `check_scopes` can tell the two cases apart: a record whose `scope` is `PerExecution` and
  which carries hooks or a check is a contribution declared by closure. A field on the variant
  carrying that would let the help name the explicit form for that case, `singleton(..)` on the
  value API. `ScopeViolation`'s help for a provider declared by closure (entry 6) raises the same
  question.
- **R2, `binding/handle.rs`:** `HookHost`'s doc says hooks on an `Auto` binding the wiring pass
  infers per-execution are refused at `wire()`. Since `Contribute::with` returns an `Auto` handle,
  that sentence also covers a contribution declared by closure that turns out to be an enhancer.
