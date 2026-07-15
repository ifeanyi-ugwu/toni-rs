# Why a trait suffices where NestJS needs a module union

The decision to accept any `ModuleMetadata` as the root module — and to delete the `ModuleDefinition`
enum — is recorded in [ADR 0008](../adr/0008-root-module-is-any-modulemetadata.md). This page explains
the mechanics behind that decision: why NestJS's `ModuleDefinition` union has no Rust counterpart, one
member at a time. It is background for anyone tempted to re-introduce the enum or port more of the
NestJS module surface.

## The NestJS union

NestJS declares what a module can be as a union type:

```ts
export type ModuleDefinition = ForwardReference | Type<unknown> | DynamicModule | Promise<DynamicModule>;
```

`NestFactory.create(...)` and `imports: [...]` both accept a value of this type. The union exists
because its members are values of physically different kinds:

```ts
// A static module — a CLASS. At runtime this is a constructor function.
@Module({ providers: [CatsService] })
class CatsModule {}

// A dynamic module — a plain OBJECT LITERAL. Just data.
const dbModule: DynamicModule = {
  module: DatabaseModule,
  providers: [{ provide: 'CONNECTION', useValue: conn }],
  exports: ['CONNECTION'],
};
```

A constructor function and an object literal share no interface. Accepting either requires the union,
and consuming either requires a runtime branch on the shape:

```ts
// shape of what @nestjs/core does internally
if (this.isDynamicModule(module)) {
  // object → read module.providers, module.imports off the object
} else {
  // class → read metadata off the decorator via reflection
}
```

The union exists because the values are structurally unrelated; the runtime branch exists because the
union exists.

## Member 1 — `Type` and `DynamicModule` collapse into one trait object

In toni, both kinds of module implement one trait, [`ModuleMetadata`](../../toni/src/traits_helpers/module_metadata.rs).
The trait is the shared interface TypeScript lacks. A macro-generated module and a runtime-built
[`DynamicModule`](../../toni/src/module_helpers/dynamic_module.rs) satisfy the same method set:

```rust
// #[module] generates this for a user struct
impl ModuleMetadata for AppModule {
    fn get_name(&self) -> String { "AppModule".to_string() }
    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> { Some(vec![/* … */]) }
    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> { Some(vec![/* … */]) }
    // …
}

// DynamicModule hand-implements the same trait
impl ModuleMetadata for DynamicModule {
    fn get_name(&self) -> String { self.id.clone() }
    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> { self.providers.lock().take() }
    // …
}
```

Behind `Box<dyn ModuleMetadata>`, the scanner calls `get_name()`, `imports()`, `providers()` without
knowing which concrete type it holds. The vtable does the job NestJS's `isDynamicModule()` branch does
— dispatch, not detection. The two systems line up like this:

```text
NestJS                                    toni
──────                                    ────
class CatsModule        ─┐                struct AppModule: ModuleMetadata  ─┐
                          ├─ no common                                        ├─ SAME trait
{ module, providers }   ─┘  interface     DynamicModule:    ModuleMetadata  ─┘
        │                                          │
        ▼                                          ▼
union type + runtime                      Box<dyn ModuleMetadata>
isDynamicModule() branch                  (vtable dispatch, no branch)
```

A `ModuleDefinition` enum with a dedicated `Dynamic(DynamicModule)` variant would re-introduce the
distinction the trait erases. Every consumer would match:

```rust
match module_def {
    ModuleDefinition::DefaultModule(m) => { let name = m.get_name(); /* … */ }
    ModuleDefinition::Dynamic(m)       => { let name = m.get_name(); /* … */ }
    //                                      identical body — DynamicModule implements the same trait
}
```

Both arms are the same code. The enum encodes a difference that does not exist at the level the scanner
operates.

## Member 2 — `ForwardReference` cures a load-order problem Rust does not have

`forwardRef(() => SomeModule)` exists for circular imports. ES modules evaluate top to bottom on
import:

```ts
// a.module.ts                                // b.module.ts
import { BModule } from './b.module';         import { AModule } from './a.module';

@Module({ imports: [BModule] })               @Module({ imports: [AModule] })
export class AModule {}                       export class BModule {}
```

Evaluating `a.module.ts` requires `b.module.ts`, which requires `a.module.ts` mid-evaluation.
JavaScript resolves the deadlock by giving `b.module.ts` a temporarily-`undefined` binding for
`AModule`. The `@Module` decorator runs at that moment and captures `imports: [undefined]`. The graph
is corrupt before Nest starts. `forwardRef(() => AModule)` wraps the reference in a closure — a value
that exists now and defers reading `AModule` until evaluation finishes.

Two properties of toni make that failure unreachable. First, the macro-generated `imports()`
constructs each imported module inside a method body:

```rust
fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
    Some(vec![Box::new(BModule)])   // BModule constructed when imports() is CALLED, not before
}
```

`BModule` is a type; it exists at compile time. There is no evaluation order in which it is
`undefined`. The construction runs only when the scanner calls `imports()`, so the method body is a
`forwardRef` closure by nature — laziness is the default, not an opt-in.

Second, the remaining hazard — infinite traversal of a cyclic module graph — is broken by the
scanner's visited-name set. In `scan_for_modules_with_imports`
([toni/src/scanner.rs](../../toni/src/scanner.rs)), `ctx_registry` records every module already seen and
skips re-pushing it:

```text
stack: [A]            registry: []
pop A               → registry: [A],   A.imports() = [B], B unseen → push B
pop B               → registry: [A, B], B.imports() = [A], A seen  → skip
stack empty         → done. Both modules registered, cycle broken.
```

A `ForwardReference` variant would guard against a corruption that cannot occur and a loop the scanner
already terminates.

This covers circular *module imports*. Circular *provider injection* (service A injects B, B injects
A) is a separate layer that the `ModuleDefinition` union never addressed in NestJS either.

## Member 3 — `Promise<DynamicModule>` targets the wrong layer

`Promise<DynamicModule>` lets an `imports` entry be awaited while Nest builds the module graph
(`ConfigModule.forRootAsync(...)`). toni's graph scan is synchronous, but the capability it enables —
running IO before the application serves traffic — already exists one layer down. A `for_root`
function only stores configuration:

```rust
pub fn for_root(database_url: impl Into<String>) -> DynamicModule {
    DynamicModule::builder("SeaOrmModule")
        .provider(SeaOrmConnectionFactory { database_url })  // just data
        .export::<DatabaseConnection>()
        .global().build()
}
```

The IO happens in provider construction, which is already `async`:

```rust
#[async_trait]
impl ProviderFactory for SeaOrmConnectionFactory {
    async fn build(/* … */) -> /* … */ {
        let db = Database::connect(&self.database_url).await;  // the await lives here
        // …
    }
}
```

So the two systems place the await at different stages:

```text
NestJS:  [await IO] → build module → build graph → instantiate providers
toni:    build module (sync data) → build graph (sync) → [await IO in provider build]
```

Awaiting *during graph construction* would require making `ModuleMetadata::imports()` async, which
turns the whole scan path async. That is a trait change on the import path — a path that never passed
through the enum. At the root, a caller who needs async setup writes `create_with(build().await)`
directly. No async requirement argues for the enum.

## The rule this leaves behind

A new kind of module belongs behind an `impl ModuleMetadata`, not behind a new entry-point wrapper
type. If a capability cannot be expressed as a `ModuleMetadata` implementation, the trait is the thing
to change — the entry-point signature is already as open as the trait itself.
