# 0028 — A type's DI token is its full `type_name`, produced by one function

Status: accepted

## Context

DI container keys are strings, and lookup is exact string equality. A type-derived token could be
built in two formats, depending on where it was built:

- **Full canonical path.** `std::any::type_name::<T>()` — `sqlx_core::pool::Pool<sqlx_postgres::database::Postgres>`.
  Used by provider registration in the macros, `resolve::<T>()`, `ModuleRef::get`,
  `provider_factory!` closure dependencies, explicit `#[inject(Type)]` tokens, and
  `DynamicModule::export::<T>()`.
- **Bare last segment plus full-path parameters.** `Pool<sqlx_postgres::database::Postgres>` —
  produced by the inject macro's generic branch, and matched by hand at the registration sites that
  serve generic types: `toni-config`'s `ConfigService<{}>`, core's `Extension<{}>`.

The two formats met nowhere. `#[inject] pool: Pool<Postgres>` could not find toni-sqlx's
registration; `resolve::<ConfigService<T>>()` could not find toni-config's; a factory closure
depending on a generic could not find either. Which spelling a user wrote decided whether startup
succeeded: `PgPool` (an alias, canonicalized by `type_name`) worked where `Pool<Postgres>` (the
written generic, bare-base) aborted. The bare-base pairs that did work — `ConfigService`,
`Extension<T>` — worked only because a human kept two hand-written `format!` calls in lockstep with
the macro's output.

Commit `40ed40b` is where the split began: it moved tokens from written-name strings to `type_name`
but left the generic branch's base un-migrated. Two smaller defects rode along. The non-generic
branch emitted `type_name` over the written type's *last segment only*, so a field written
`my_mod::Type` compiled only if `Type` happened to be in scope — and silently produced the wrong
token if a *different* `Type` was. And the format knowledge had no owner: five places (the inject
macro, the provider macros, core runtime lookups, toni-config, `Extension`) each encoded their own
copy of what a token looks like.

## Decision

One function owns the format: `toni::di::token_of::<T>() -> String`, the type's fully-qualified
`type_name`, base and generic parameters alike.

Every site that turns a type into a container key calls it — macro-generated registration and
injection, the by-type lookups, exports, and the library provider factories that used to hand-format.
The macros emit `token_of::<T>()` with the *full written type*: it resolves in the caller's scope
because it is the field's own type, and the compiler canonicalizes qualified paths, aliases, and
generics uniformly. No macro, and no library, string-formats a token.

Module identity keys derive from the same function where they carry the same information:
`#[module]` identities were already `type_name::<Self>()`, and `ConfigModule<T>::get_id` now is too,
so a future lookup-modules-by-type cannot reproduce the token mismatch in the module namespace.
(`GraphQLModule`'s identity originally kept its hand-built id, excluding the context-builder
parameter, because folding `Ctx` in would change which modules dedup. Superseded: the identity now
includes the full type and a config fingerprint — ADR-0029.)

`type_name` output is not guaranteed stable across compiler versions, and does not need to be: a
token never leaves the process, and one binary computes both sides of every comparison.

## Consequences

- The written spelling of a type no longer matters: `PgPool`, `Pool<Postgres>`, and
  `my_mod::Pool<Postgres>` produce the same token. Injecting a generic-typed handle from any DB
  integration works as the docs already described.
- `resolve::<T>()` and `ModuleRef::get` reach every type-registered provider, including generics
  (`ConfigService<T>`); `DynamicModule::export::<T>()` exports a token something actually registers.
- A library that registers a provider for a generic type writes `token_of::<Service<T>>()` once
  instead of maintaining a `format!` in lockstep with macro internals.
- Tokens hand-written as strings against the old generic format
  (`#[inject("ConfigService<my_crate::AppConfig>")]`) stop matching. The failure is a startup
  "dependency not found" naming both tokens.
- `integration-tests/tests/integration/token_format.rs` pins the invariant: one registration, every
  lookup path, token equality observed as resolution success.
