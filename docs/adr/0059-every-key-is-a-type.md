# 0059 — Every key is a type

Status: proposed

A key to the container is a type. A type is its own key, and a second slot for a type, or a named
slot for a role, is a marker type that names what it holds through the `Key` trait. Strings are not
provider keys. A bare path in a key position is always a type, and no key position has to tell a
type from a const.

## Context

**The key kinds were copied from a language where a class is a value.** NestJS keys a provider by a
class, a string or a symbol, and all three are values an argument can hold. ulo took all three: a
type's own key, a string, and `Token<T>`, a named const standing in for the symbol. TypeScript's
`emitDecoratorMetadata` records each constructor parameter's class, and Nest reads a class key off a
type annotation; an interface is erased before runtime, so a provider behind one takes a string or a
symbol. In Rust a trait object is a type, `dyn Trait`, with a key of its own. Rust's compile-time
tools are limited on stable: `type_name` cannot be called in a `const` and its output is diagnostic,
`TypeId` cannot be compared in a `const`, and a `&'static str` const parameter, `Named<"db">`, is
not stable.

**A bare path is a type or a const, and a macro cannot tell which.** `tokens::AUTH` and
`guards::AuthGuard` are both paths. A const is only a value and a braced struct's name only a type;
a tuple struct's name is also a value, its constructor, and a unit struct's a constant of its type.
An expansion reading the path as a type fails on a const, one reading it as a value fails on a
braced struct, and on a tuple or unit struct the value reading compiles and names the wrong thing.

**The value API first written for ADR-0058 checked keys with five traits.** `DeclarationToken`
checked what a declaration's key could be, `Holds` and `HoldsPerExecution` what it could hold,
`CollectionKey` what a collection's key could be, and `Injects` what a field's key could fill. They
ranged over strings, `Token<T>`, `Many<T>` and the macro-emitted `Of<T>` and `OutputKey<R>`. The
lookups take `IntoToken`, implemented for strings and `Token<T>`.

**Other containers key by type and qualify by name.** Guice, Dagger, Spring and .NET's keyed
services take the provided type as the primary coordinate and a name or annotation as a secondary
one. Guice advises using `@Named` strings sparingly because the compiler cannot check them, and a
custom qualifier annotation turns a typo into a compile error. In Rust, the `typemap` crate's
`pub trait Key: Any { type Value: Any; }` makes a marker type a typed slot, and the `more-di` crate
keys named services by zero-sized marker types instead of strings.

## Decision

**A key is a type.** A binding is stored under `token_of::<K>()` (ADR-0028), and a type is the only
thing a key position reads.

**A type is its own key, with nothing to write.** `#[inject] db: Arc<Db>` fills the field from
`Db`'s slot, a keyless `provide!(async || ..)` binds under the type it builds, and
`resolve::<Db>(&ctx)` looks up `Db`'s slot. A foreign type, such as `sqlx::Pool<Postgres>` or
`ConfigService<AppConfig>`, is reached the same way.

**A named slot is a marker type.**

```rust
pub trait Key: 'static {
    type Value: ?Sized + 'static;
}

pub struct Replica;
impl Key for Replica { type Value = Db; }

key!(pub Replica: Db);                    // the declaration above, in one line
key!(pub Auth: dyn Guard<HttpContext>);   // a role slot
```

A marker names what its slot holds. `provide!(Replica => ..)` takes a value, factory or type whose
product is a `Db`, `#[inject(Replica)] replica: Arc<Db>` fills a field from it, and a mistyped
marker is a compile error. A marker names a foreign type as easily:
`key!(pub Main: sqlx::PgPool)`.

**A role slot is a marker whose `Value` is an enhancer trait object.** `Auth` above holds
`dyn Guard<HttpContext>`. What is provided under it must coerce to `Arc<dyn Guard<HttpContext>>`,
which the macro checks where the concrete type is known, and the binding takes the role. A marker
over any other trait object, such as `dyn Logger`, holds data. A type's own enhancer role comes
from its trait impls (ADR-0002).

**A position that reads what a slot holds needs `Key`.** `provide!`'s key side, `#[inject(K)]` and
the `_key` lookups and constructors read `K::Value` and need `K: Key`; a `dyn Trait` written in a
macro position is lowered to the trait object's own slot and needs none. `#[injectable]`,
`#[controller]` and `#[websocket_gateway]` implement `Key` for their type with `Value = Self`, and a
plain type takes `#[derive(Key)]`. A blanket `impl<T> Key for T` would conflict with every marker's
impl (E0119), which is why the reflexive impl comes from the attribute or the derive. `exports:`,
`.export::<K>()`, the `#[use_*]` attributes and `alias`'s target read only the slot's key, and take
any type, a foreign one included.

**`dyn Trait` in a key position is that trait object's slot.**
`provide!(dyn Logger => ConsoleLogger)` binds it, and `#[inject] logger: Arc<dyn Logger>` reads it.

**A collection is a binding declared with `into`, under any key.**
`provide!(into dyn Plugin => A {})` contributes to the unnamed collection under `dyn Plugin`, and
`provide!(into LegacyPlugins => A {})` to a marker's, whose `Value` is the element type. A key is
single-bound unless a binding under it is declared with `into` (ADR-0057).

**Strings are not provider keys.** A set of names chosen at runtime is one binding holding the set,
such as a `TenantPools(HashMap<String, Pool>)` built from configuration and bound under its own
type. A slot shared by two crates is named by a marker exported by one of them, or by a crate both
depend on. A module is still found by its identity string (ADR-0029).

**Every key position reads a type.** `provide!`'s key side, `#[inject(..)]`, `into`, `alias`,
`exports:` and the `#[use_*]` attributes each read a bare path as a type, with generic arguments
written with or without a turbofish. `provide!`'s source side keeps `value`, `factory` and `alias`,
which name the non-default readings of what is bound rather than kinds of key.

**The value API and the lookups take a key two ways.** A type's own slot, a trait object's
included, is addressed by the type as a type parameter, and a marker's slot by the marker through
a `_key` form: `resolve::<T>` and `resolve_key::<K>`, and likewise for `get`, `get_from` and
`ModuleRef`'s lookups and for the value API's constructors. A module exports a slot as
`exports: [K]` or with `.export::<K>()`.

**An integration names a second connection with a marker.** Each named constructor
(`for_root_named(name, ..)`, and the `postgres_named(name, ..)` family of sqlx and diesel) becomes
a keyed one (`for_root_keyed::<K>(..)`, `postgres_keyed::<K>(..)`), with `K: Key<Value = Conn>`,
and the module's identity base takes `token_of::<K>()` in place of the name.

## Consequences

- `Token<T>`, `IntoToken`, string keys, `get_by_token`, `get_from_by_token`, `resolve_by_token`,
  `export_token` and the unread `APP_MIDDLEWARE` are deleted. The value API is built without
  `Many<T>`, `OutputKey<R>`, `Of<T>`, `DeclarationToken`, `CollectionKey` or the `token` keyword.
  `Holds` becomes a check of what is provided against `K::Value`: the type equality
  `K: Key<Value = ..>` for a slot holding the declared type, the cast for one holding a trait
  object. `HoldsPerExecution` becomes `PerExecution`, implemented for the guard and interceptor
  trait objects, and `Injects` a check of a field's shape against `K::Value`.
- A named slot costs a marker type, two lines or one `key!` line, where a string was one literal.
- A key cannot be chosen at runtime, and two crates cannot share a slot by agreeing on a string.
- A foreign type is bound keyless and injected by bare `#[inject]`. The orphan rule keeps a user
  crate from implementing `Key` for it, so it cannot be written on `provide!`'s key side or in
  `#[inject(K)]`, and a marker names it there: `key!(pub Main: sqlx::PgPool)`.
- A marker's own slot and the slot it names are one string, `token_of::<K>()`. Providing the marker
  itself beside a binding under it fails `create` as two bindings under one key (ADR-0057), and
  `resolve::<K>(&ctx)` on a bound marker answers `TypeMismatch`.
- Storage is unchanged: a key's string is `token_of::<K>()`, a per-binary `type_name` (ADR-0028).
- `APP_GUARD` and `APP_INTERCEPTOR`, which apply on HTTP alone, are replaced by the marker types
  `AppGuards` and `AppInterceptors`, and `AppErrorHandlers` is added; the three name the
  every-transport sets of ADR-0057, each holding a framework trait object whose trait requires its
  role on all four transports.
- The diagnostics and docs that send a second connection to `for_root_named` and
  `#[inject("<name>")]` name the keyed constructor and `#[inject(K)]` instead.

## Roads not taken

**Nest's three key kinds.** A string key is unchecked, and a const beside a type is the ambiguity
this record removes.

**A position rule**, reading a bare path as a token on `provide!`'s key side and in
`#[inject(..)]` and as a type elsewhere, with a token after `token`. It is machinery around the
same ambiguity, and a key that is always a type needs none of it.

**A const generated beside each type.** `#[injectable]` could emit a const named `Db`, standing for
`Db`'s slot, into the value namespace a braced struct leaves free, making `Db` a value in every
position. A tuple or unit struct's value name is taken, a const cannot be generic, and a foreign
type, a trait object or a primitive gets no const, so each would still need a second spelling.

**Keys as values only.** A type's own key would be written `Of::<Db>::new()` at every site, or come
from the generated const above.

**A string escape hatch.** A string literal is not a path and raises no ambiguity, but it readmits
the unchecked key, whose misspelling is found only at startup.

**`Named<"db">`**, a named slot without a type declaration, needs an unstable const parameter.
