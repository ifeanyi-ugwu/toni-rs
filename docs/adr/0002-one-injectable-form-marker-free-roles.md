# 0002 — One `#[injectable]` provider form; enhancer roles detected from trait impls

Status: accepted

## Context

Declaring a provider used to require restating the same facts in several places. The struct was
threaded through the macro attribute — `#[injectable(pub struct Foo { .. })] impl Foo { .. }` — so the
declaration lived half in an attribute argument and half in a normal item. An enhancer needed *both*
its trait impl (`impl Guard<HttpContext> for Foo`) *and* a marker attribute (`#[guard(http)]`) naming
the role and transport the impl already stated. A field-injected provider also wrote
`#[derive(Clone, Injectable)]`, naming `Clone` explicitly.

Every one of these is a second declaration that can drift from the first, and each is a thing a
newcomer must learn beyond plain Rust. The trait impl, the struct, the `Clone` requirement — the
compiler already knows them.

## Decision

A provider is a plain struct plus its normal `impl`s. One attribute, no restatement.

- `#[injectable]` is an attribute on the struct. `#[inject]` fields are dependencies, `#[default(expr)]`
  fields are owned state, `#[injectable(scope = "request" | "transient")]` sets scope. Because it is an
  attribute (it re-emits the item) rather than a derive, it supplies the `Clone` impl the container
  needs — the struct carries no derive ceremony.
- A constructor is a `fn` tagged `#[new]`; a lifecycle hook is a `fn` tagged `#[on_module_init]` (etc.).
  See [0001](0001-dispatch-not-detect-autoref-bridge.md) for how these reach the struct macro.
- An enhancer is a provider that implements the role trait — `impl Guard<HttpContext> for Foo` *is* the
  declaration. The role is detected from the impl at registration (the `toni::__detect` probes); there
  is no `#[guard]` / `#[interceptor]` / `#[pipe]` / `#[error_handler]` / `#[middleware]` marker. The
  context type in the trait (`HttpContext` vs `WsContext` …) names the transport, so that isn't
  restated either.

Why `#[injectable]` is an attribute, not a `#[derive]`: a derive receives the struct with its
`#[derive(...)]` list stripped — it can neither see a sibling `Clone` nor add one without risking a
conflicting impl. An attribute re-emits the whole item, so it can add `Clone` only when absent. The
"one word" goal is reachable only via the attribute form.

## Consequences

**Good.** The declaration is plain Rust: a struct, its fields, its trait impls. "How do I make a
guard?" is answered by "implement `Guard`" — the framework-specific surface is `#[injectable]`,
`#[inject]`, and the application sites (`#[use_guards(..)]`), nothing more. Drift between marker and
impl is impossible because there's no marker.

**Failures move to the use site, loudly.** Referencing a type as a role it doesn't implement is a
compile error where it's used, rather than a silent non-registration. (See
[0001](0001-dispatch-not-detect-autoref-bridge.md) for the detection trap that makes "silent" the
alternative to guard against.)

**Breaking change.** The struct-in-attribute form, the marker attributes, the `Injectable` derive, and
the explicit `guard`/`interceptor`/… arguments to `provider_factory!` are removed. Migration is
mechanical and was done across the codebase when this landed (PR #91).

**Known gaps.** `#[injectable]` does not support generic structs (rejected with a clear error). For
`provider_factory!` under a string/const token with no `-> T` annotation or type hint, the produced
type can't be named, so enhancer-role detection can't run for it — singletons need nothing; the gap is
narrow and documented at the call site.
