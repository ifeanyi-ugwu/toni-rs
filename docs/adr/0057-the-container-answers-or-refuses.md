# 0057 — The container answers an ambiguous question, or refuses it

Status: proposed

A key is single-bound unless declared multi, and a second single binding, or a single binding beside
an `into` contribution, is refused naming both. A runtime lookup that widens past its module reaches
only exported and global keys. Lifecycle hooks run in construction order and shutdown runs in its
exact reverse, construction order being dependency order with ties broken by declaration order.

## Context

Three questions reach the container with more than one possible answer. At each, map mechanics pick
one answer, by insertion or by iteration order, and nothing is reported.

**Two bindings under one token.** The second replaces the first and `create` succeeds. Where the
framework fixes the token the cost is worse: two `APP_GUARD` registrations in one module run the
surviving guard twice and the other never, and the same pair split across two modules does the same.
Registering a global guard per module is the shape Nest documents, and here the second registration
removes the first. ADR-0029 already refuses one case, two global modules exporting one token, naming
both keys. Nothing refuses two providers in one module, two imports exporting one token, or any
`APP_*` registration after the first.

"Many under one token" is also a real intent, and the framework carries it as a side channel: a
multi provider is a separate kind with its own code path per declaration kind, not a property of a
binding. A clash can only be told from a declared collection once the collection is ordinary token
semantics.

**Which module answers.** A token held by two modules resolves to one of them at three sites: the
loader's walk of an import set, `ModuleRef`'s non-strict lookup, and the application context's
lookup by type. Renaming one module's builder id, declaration order unchanged, flips which value an
injection site receives. Separately, `ModuleRef`'s `.global()` searches every provider of every
module, exported or not, so a provider in a module that exports nothing and is not global is
reachable from a module that never imports it. The method is named after the global registry that
`#[module(global: true)]` fills, and it reaches past that registry.

**The order hooks fire in.** `on_module_init` and `on_application_bootstrap` walk the container's
module map in hash order, and each module's providers come from another hash map. A provider's hook
can run before the hook of a provider it injects, and adding one unrelated provider to a module
flips two existing hooks. The three shutdown phases walk the same maps forwards, so teardown is not
the reverse of construction. Construction itself is ordered: each module's dependency graph sorts
its providers topologically, and modules are sorted before loading. The order exists one layer below
the hooks and is not reused.

## Decision

**A key is single-bound unless declared multi.** A key is a type (ADR-0059): the type a binding
answers as, `dyn Trait` included, a marker type, or, for an unnamed collection, its element type. A
second single binding under one key fails `create`, naming both declarations. A key is declared
multi by contributing to it with `into` (ADR-0058), and a single binding and an `into` contribution
on one key fail `create` naming both. A multi key collects its contributions in declaration order
and injects them as `Vec<Arc<T>>` for element type `T`. The unnamed collection of a role type is
that transport's global set, and the markers `AppGuards`, `AppInterceptors` and `AppErrorHandlers`
name the sets every transport runs, so two global guards both run. Any registration surface that
takes a key obeys the same rule.

**A widening lookup is a fallback into the global registry.** `ModuleRef`'s `.global()` becomes
`.or_global()`: the current module first, then exported and global keys, and nothing a module kept
private. A key that two imports export into one module fails `create` naming both, and where
startup cannot see the ambiguity, a lookup answers `ResolutionError::AmbiguousModule` rather than
picking one.

The framework keeps two meanings of *global* and only two. *Visibility*: a module's exports reach
every module. *Application*: an enhancer runs on every dispatch target. Search is not a third
meaning; after the narrowing it is a fallback into the first, which is what the new name says.

**Lifecycle order is a contract.** Construction order is dependency order, and ties break by
declaration order: walk the declaration list in order, and before emitting a provider emit
everything it injects, so a module written in a valid order gets back exactly what it declared.
Hooks follow construction order. Shutdown runs in the exact reverse. A module's own hooks sit
outside construction order, the loader building no module: in every phase they run ahead of every
provider's, at startup and at shutdown. The container's module map and each module's provider,
provider-instance and controller maps become insertion-ordered maps with the same hasher. A hash
map promises no order, which is correct for a pure lookup and wrong once iteration carries a
contract; the maps keyed by `TypeId` are lookups only and stay as they are.

This record refines ADR-0029, which decided the global-export clash: that refusal is one case of the
rule above.

## Consequences

- Two providers under one key, in one module or across two imports, fail `create` naming both.
- Two contributions to `AppGuards`, in one module or in two, both run in declaration order, and so
  do two to `AppInterceptors`.
- A provider in a module that exports nothing is unreachable from outside it, `.or_global()`
  included.
- A provider's `on_module_init` runs after the init of every provider it injects, and its shutdown
  hooks run before theirs. Adding an unrelated provider changes neither.
- `ModuleRef`'s `.global()` is renamed with no alias, the crate being unpublished.

## Roads not taken

**Last-wins as an override mechanism.** Nothing documents it, and it cannot express the `APP_GUARD`
case, where the intent is two guards, not a replacement.

**Refusing only the framework-fixed keys.** It leaves every user key open to the same unreported
replacement, and it still needs multi to work as ordinary key semantics before the fixed keys can
be collections.

**Keeping `.global()`'s reach and renaming it `.anywhere()`.** A provider a module did not export
should not take part in another module's lookups, and a real need to search everywhere earns its own
API when something asks for one. Nothing in the tree asks: the one caller of the widening resolves a
provider that a global module exports.

**Leaving lifecycle order unspecified and documenting that.** It asks every application to encode
an order the container already computes.
