# 0008 — The root module is any `ModuleMetadata`; the `ModuleDefinition` enum is removed

Status: accepted

## Context

From the first commit, the factory entry points (`create`, `create_with`,
`create_application_context`, `create_application_context_with`) took
`impl Into<ModuleDefinition>`, where `ModuleDefinition` was a public enum with exactly one variant:

```rust
pub enum ModuleDefinition {
    DefaultModule(Box<dyn ModuleMetadata>),
}
```

The name and shape transliterate NestJS's union type from `@nestjs/core`:

```ts
export type ModuleDefinition = ForwardReference | Type<unknown> | DynamicModule | Promise<DynamicModule>;
```

That union earns its existence in TypeScript: a module class (a constructor function) and a dynamic
module (a plain object literal) share no interface, so accepting either requires a union plus a
runtime `isDynamicModule()` branch. The enum was the Rust slot for those variants — scaffolding for
a future in which the other union members would arrive.

That future cannot arrive, because each anticipated variant is dead in Rust. The mechanics behind each
are worked through in the companion explainer,
[Why a trait suffices where NestJS needs a module union](../explainers/module-definition-and-the-nestjs-union.md);
the summary:

- **`DynamicModule`** — already implements `ModuleMetadata`, so it fits in the existing
  `Box<dyn ModuleMetadata>`. The polymorphism the TS union provides is exactly what `dyn` dispatch
  provides; a dedicated variant would produce `match` arms with identical bodies, re-introducing at
  the enum level a distinction the trait erased.
- **`ForwardReference`** — solves a JavaScript load-order problem. With circular ES-module imports,
  one class binding is `undefined` at decorator-evaluation time, so Nest defers the reference inside
  a closure. In Rust, types exist at compile time and the macro-generated `imports()` constructs
  imported modules inside a method body — deferred by nature, a `forwardRef` closure for free. The
  remaining hazard, infinite traversal of a cyclic module graph, is broken by the scanner's
  visited-name guard (`ctx_registry` in `scan_for_modules_with_imports`).
- **`Promise<DynamicModule>`** — async module construction. The capability (do async IO before the
  app serves traffic) already exists one layer down: `ProviderFactory::build` and the lifecycle
  hooks are async, which is where every integration crate connects (`for_root` only stores
  configuration). If awaiting during graph construction were ever needed, the lever is making
  `ModuleMetadata::imports()` async — a trait change on the import path, which never passed through
  the enum. At the root, the caller can `.await` before calling `create`.

Meanwhile the enum had costs. The scanner destructured it with an irrefutable `let` — an enum that
can be destructured irrefutably carries zero information — and re-wrapped each import just to
unwrap it on the next loop iteration. Every `#[module]` expansion emitted a `From<X> for
ModuleDefinition` impl, plus two hand-written ones for `BuiltinModule` and `BroadcastModule`. Worst,
`impl Into<ModuleDefinition>` made the entry point a closed set — only types with a `From` impl
qualified — and `DynamicModule` was not on the list, so a `DynamicModule` root (a worker that needs
only `RedisBroadcastModule::for_root(url)`, say) required hand-constructing the enum around a box.

## Decision

The entry points state their actual requirement directly:

```rust
pub async fn create(module: impl ModuleMetadata + 'static) -> ToniApplication
```

The enum, the macro-generated `From`, and the hand-written `From` impls are deleted. The scanner
walks `Box<dyn ModuleMetadata>`. `ModuleMetadata` is re-exported at the crate root so the bound is
nameable without the `traits_helpers` path.

## Consequences

**Open set.** Anything implementing `ModuleMetadata` is a valid root — macro-generated modules,
`DynamicModule`, future module kinds — with no per-type registration. `create_application_context(
RedisBroadcastModule::for_root(url))` compiles; the enum-wrapping workaround in the redis-broadcast
integration test is gone.

**Less macro output.** `#[module]` no longer emits a `From` impl per module.

**Breaking, narrowly.** Code naming `ModuleDefinition` (the type was public at
`toni::module_helpers::module_enum`) must pass the module value directly instead. Ordinary call
sites — `ToniFactory::create(AppModule)` — compile unchanged.

**Boundary for the future.** New kinds of module belong behind `impl ModuleMetadata`, not behind a
new entry-point wrapper type. If a capability cannot be expressed as a `ModuleMetadata`
implementation, the trait — not the entry-point signature — is the thing to change.
