# 0058 — A provider declaration is a value, written through one macro

Status: accepted

A provider is declared by `provide!(key => source)`, one macro whose every form builds a value a
public constructor of the value API also builds, so the builder and a function building declarations
take the same values. A key is a type (ADR-0059), and no macro tells a type from a const by how it
is written. A key is single-bound unless a binding under it is declared with `into`; the unnamed
collection is keyed by its element type. An enhancer role comes from the enhancer's own type or from
the key's. A provider factory is always async. `#[new]` is the one constructor. What `build` builds
is a `Registration`.

## Context

**Two vocabularies over ten code paths.** A provider is declared by `provide!` with five marker
keywords, or by one of `provider_value!`, `provider_factory!`, `provider_alias!` and
`provider_token!`. They are not alternatives: `provide!` classifies its input, renders it back to
tokens and hands it to the other four. Under both, `multi` is a second copy of every declaration
kind rather than a property of a binding, so five acts by two channels are ten code paths.

**A token is read by its spelling.** One function decides whether a token path names a type or a
const by asking whether it has one segment and is written in capitals. A `Token<T>` const in a
`tokens` module, where a project puts them, is refused in every macro position, and a type named
`HTTP` is read as a const. Each surface refuses a different subset of spellings. The refusals are
`E0573`, `E0423` and `E0747`, and none names the constraint. `provide!` carries a second, looser
classifier whose answer the handler it forwards to discards.

**The module grammar has four dialects.** `#[module]`'s `imports:` and `providers:` take an
expression; `controllers:` and `exports:` take a bare identifier. A module cannot export a
string-token provider, a generic provider or a provider from a submodule, and cannot declare a
path-qualified controller. `DynamicModule`'s builder accepts what the attribute refuses, and splits
each list into a by-type method and a by-factory method. `Extension<T>` is declared in `providers:`
through a factory registered per payload type, and `exports:` cannot name it without a type alias.

**Construction has two mechanisms that read different places.** `#[new]` reads a method's
parameters. `init = "…"` reads the struct's `#[inject]` fields and passes them positionally, so the
same struct moved between the two forms resolves different dependencies. `#[default(expr)]` is inert
whenever `#[new]` is present, with no warning. `#[controller]` refuses `init = "…"` already, in a
message saying "as with `#[injectable]`", which is false while `#[injectable]` accepts it. An init
method named `from_request` changes what the macro emits.

**A factory is sync or async, and the duality is copied.** Nest's `useFactory` accepts a value or a
promise because JavaScript's `await` on a non-promise is a no-op. Rust has no such affordance.
Building it by hand has two shapes and both fail: a marker type parameter is ambiguous at the call
site for an async closure (`E0282`), and two entry points accept the async form through the sync
door as a provider of futures.

**A collection needs a conversion that only concrete code can write.** A multi-provider collection
hands out `Vec<Arc<dyn Plugin>>`, so each contribution's `Arc<V>` becomes an `Arc<dyn Plugin>`. For
a trait the caller chooses, that conversion needs `Unsize`, which is unstable: a generic function
cannot write it, and a modifier such as `.multi::<dyn Plugin>(..)` compiles as a signature and has
no body that type-checks. Where both types are concrete, at the call site, the conversion is a plain
`as` cast.

**A role is detected where the value's type is concrete.** `provider_value!` and
`provider_factory!` emit autoref probes that register a middleware, guard, interceptor or
error-handler role when the produced value's type implements one. A value surface is a generic
function, which sees its value only through its bounds, so the probe has nothing to resolve against
there.

## Decision

**One macro over the value API.** `provide!(key => source)` replaces the old `provide!` and the four
`provider_*!` macros as the declaration surface. Each of its forms builds a value a public
constructor also builds (a type's own declaration, and `Provide::value`, `Provide::factory`,
`Provide::alias` and `Provide::into`, each addressing a type's own slot by a type parameter and a
marker's slot through its `_key` form, ADR-0059), so `builder.provider(provide!(..))`,
`#[module(providers: [..])]` and an integration crate's `for_root` function build the same values.
The macro is sugar over the value API, never beside it. It is a proc macro: it reads the source's
syntax to choose a form, and it points each refusal at the user's value or closure. Its grammar is
led by keywords and position, never by how a name is cased.

| Written | Means |
| --- | --- |
| key: a type implementing `Key` (ADR-0059), `dyn Trait`, or none | where the binding goes; none keys it by what the source builds |
| `into` before the key | a contribution to the collection under that key |
| a bare path as the source, whatever its case | that type's own declaration, the rule `providers: [Db]` uses |
| an inline closure / any other inline expression | a factory / a value |
| `factory f`, `value C` | a factory or value held in a variable or const |
| `alias K` | a second name for an existing binding |

```rust
key!(pub Port: u16);                        // a named slot
key!(pub ApiKey: String);
key!(pub AuditLog: dyn Logger);
key!(pub Auth: dyn Guard<HttpContext>);     // a role slot
key!(pub LegacyPlugins: dyn Plugin);        // a second collection over `dyn Plugin`

providers: [
    Db,                                                           // the type declares itself
    provide!(Port => 3000u16),
    provide!(ApiKey => value key),                                // a value held in a variable
    provide!(async |cfg: Arc<Config>| Cache::connect(&cfg.url).await), // keyed by Cache
    provide!(dyn Logger => ConsoleLogger),                        // a trait bound to its implementation
    provide!(AuditLog => alias dyn Logger),                       // a second name for the dyn Logger binding
    provide!(Auth => HeaderGuard("x-auth")),                      // a guard, from the key's `Value`
    provide!(into AppGuards => AuditGuard),                       // every transport: a guard implementing all four
    provide!(into dyn Guard<HttpContext> => RateLimit(100)),      // a global guard, HTTP
    provide!(into dyn Plugin => A {}),
    provide!(into dyn Plugin => async |d: Arc<Dep>| B(d)),
    provide!(into dyn Plugin => C),
    provide!(into dyn Plugin => factory make),                    // a factory held in a variable
    provide!(into LegacyPlugins => A {}),                         // the second collection
]
```

The macro has what the macros it replaces lacked: one vocabulary serves every place a declaration is
written, and no role is detected by probing a value's type. A factory closure is emitted as written,
its dependencies read from its parameter types through a trait implemented per arity, and the macro
writes only the conversion of its output.

**Every key position reads a type (ADR-0059).** `provide!`'s key side, `#[inject(..)]`, `into`,
`alias`, `exports:` and the `#[use_*]` attributes of ADR-0055 read a bare path as a type. On
`provide!`'s source side a bare path is that type's own declaration, an inline closure a factory
and any other inline expression a value; `value` and `factory` name a value or factory held in a
variable or const, and `alias K` a second name for the binding under `K`. The `#[use_*]` attributes
read a bare path as the binding under that type's key, an inline expression as a value built once
and shared, `value X` as a value held in a const or, by reference, a `static`, and a closure as a
constructor run per execution. A keyword counts only where the rest does not read as one
expression; `value &X` and `factory |..|` parse as one `&` or `|` expression and are keywords all
the same.

`#[inject(K)]` fills a field holding `K::Value` in a shape its binding gives an injection point:
`Arc<K::Value>` for any binding, `K::Value` for one handing out a value (a transient, or a provider
written by hand to hand out a handle), `Vec<Arc<K::Value>>` for a collection. `#[inject]` keys by
the field's type with that shape removed.

```rust
#[use_guards(AuthGuard, Auth, value &SHARED_LIMITER)]
#[inject(ApiKey)] key: Arc<String>,
#[inject] db: Arc<Db>,
```

**Single or many is a property of the key.** A key is single-bound unless a binding under it is
declared with `into`, and a single binding and an `into` on one key fail `create` naming both
(ADR-0057). The unnamed collection is keyed by its element type:
`#[inject] plugins: Vec<Arc<dyn Plugin>>` asks for the collection `dyn Plugin` the way
`#[inject] db: Arc<Db>` asks for `Db`. A second collection over one trait is a marker whose `Value`
is `dyn Plugin`. The macro writes the `Arc<V>` to `Arc<dyn Trait>` cast at the call site and passes
it to the value API beside the declaration, so no trait needs a declaration to be collected.
A collection gathers every module's contributions under its key, exported or not, in the order the
scan reaches them.

**A key names what it holds.** A marker's `Value` fixes what may be provided under it and what a
field reads from it: `provide!(ApiKey => 42u32)`, where `ApiKey` holds a `String`, fails to compile;
an `alias` under it is checked when it resolves. Two bindings under one key fail `create`
(ADR-0057).

**A role comes from a type: the enhancer's own, or the key's.** An `#[injectable]` enhancer
registers its roles, and rebinding it under another key forwards them. A key typed with a role
trait makes whatever is provided under it take that role, and a value that does not implement it
fails to compile:

- a marker holding `dyn Guard<HttpContext>` is a single guard, applied where a route names it;
- the unnamed collection `dyn Guard<HttpContext>` is HTTP's global guards;
- the markers naming the every-transport global sets (ADR-0057) take a contribution only if it
  implements the role for all four transports.

A value or factory under a marker holding a data type registers no role; a type's own declaration
rebound under one keeps its type's roles.

**`#[module]` and the builder take the same expressions.** `imports:`, `providers:` and
`controllers:` parse expressions, a bare path in the last two being its type's own declaration.
`exports:` lists key types, as `.export::<K>()` does. The builder has one `.provider(value)` and one
`.controller(value)`. The attribute and the builder are two syntaxes for one list. An `Extension<T>`
needs no declaration, as `Extensions` needs none: the container answers any `Extension<T>` key from
the execution's bag.

**A provider factory is always async.** The bound is
`F: Fn(A..) -> Fut, Fut: Future<Output = R> + Send`, which accepts every async spelling and refuses
a sync closure. `AsyncFn` accepts the same set, but its future type is unstable, so `Send` cannot be
named on it. `Provider::resolve` and `ProviderFactory::build` are already async; the surface becomes
consistent with the SPI.

The rule behind this one: *where a language affordance is being copied rather than a design, check
that the affordance exists.* Nest's duality rests on `await`, and copying it means building
`await`'s tolerance by hand.

**`#[new]` is the only constructor.** It reads dependencies from the signature, where a Rust reader
looks, and needs no attribute on the fields. `init = "…"` is removed from `#[injectable]`, and
`#[controller]`'s refusal message goes with the key. `#[default]` beside `#[new]` is refused naming
both, since the constructor decides every field.

**One word means one thing, and a type is named for what its consumer does with it.**

| Concept | Name |
| --- | --- |
| Recipe: key, dependencies, build | `ProviderFactory` |
| What answers a resolution | `Provider` |
| What `build` builds: an instance and its roles | `Registration` |
| Marking a type a provider | `#[injectable]` |
| Marking a dependency | `#[inject]` |
| A type's own declaration | `T::provide()`, or a bare name in `provide!` |
| Binding under another key | `provide!(K => T)`, or `T::provide().under_key::<K>()`; under a trait-object `Value`, `under_key_with::<K>(cast)`, where `provide!` writes the cast |
| Contributing to a collection | `provide!(into K => source)`, or `Provide::into::<T>(item)` / `Provide::into_key::<K>(item)` |
| A named slot or collection | a marker type implementing `Key` |

`provide()` is a trait method, so `DeclaresProvider` goes in the prelude with `provide!`.

**Nothing is kept for compatibility.** The crate is unpublished and its first release will be a
beta. The four `provider_*!` macros and the old `provide!` grammar are deleted rather than
deprecated, `init = "…"` is deleted without a migration error, `Injectable` is renamed without an
alias, and `__ulo_provider_factory` leaves the public surface.

## Consequences

- Every provider declaration in the tree is rewritten.
- A trait is collected with no declaration of its own; a contribution that does not implement it
  fails to compile.
- The every-transport sets (`AppGuards`, `AppInterceptors`, `AppErrorHandlers`, ADR-0059) run on
  all four transports. An enhancer implementing its role for one transport contributes to that
  transport's collection and fails to compile under one of those markers.
- A module can export a marker's slot, a generic provider and a provider from a submodule, and
  declare a path-qualified controller.
- A sync factory gains one word, `async`.
- A struct built by `#[new]` takes its dependencies from the signature alone.
- A value becomes an enhancer only where a type says so. A plain guard value provided under a
  marker holding a data type registers no role, and a `#[use_guards]` naming that marker fails
  startup with the registry's not-found diagnostic. A value that does not implement `Guard` fails
  to compile under a guard marker.

## Roads not taken

**One grammar for the five marker keywords over the four macros.** It leaves the ten code paths and
the token as a token tree.

**Two parallel vocabularies with one documented as preferred.** Each keeps its own code path, and
the call sites split across both.

**Fixing each token surface as it is reported.** It leaves five surfaces with five subsets, and the
defect has been rediscovered from two directions already.

**Documenting the module grammar's limits.** It sends an application needing to export a named slot
to `DynamicModule`.

**A marker type parameter over sync and async factories**, ambiguous at the call site for an async
closure, and **two entry points**, which compiled a provider of futures until an `Output: Clone`
bound was added and still leaves two doors for one act.

**`init = "…"` kept beside `#[new]`**, which keeps two sources of dependencies for one struct.

**Detecting a value's role inside the constructor.** Stable Rust cannot ask a generic value
whether it implements a trait. **Naming the role at each declaration** (`.guard::<Http>()`) adds a
modifier per role where the key already says it. **A `#[module]` syntax of its own**, emitting the
same probes, would work in one of the two places a declaration is written and leave the builder
without roles.

**Values alone.** A contribution to a collection would need `Arc::new` around every value and an
`as Arc<dyn Trait>` in every factory body, and a type's own declaration could not contribute at all.
**A `.multi::<dyn Trait>(name)` modifier** needs `Unsize` and has no body that compiles on
stable. **A declaration once per collected trait** (`collects!(dyn Plugin)`) and **a block declaring
every key** both work, and both keep multi as a separate step a user takes before a collection
exists.

**A coercion closure at each contribution** (`|p| p`) repeats at every call the cast the macro
writes.

**`provide!` probing a value's type for a role.** The macro would register a role the constructor it
expands to cannot, and `provide!(K => v)` would build a different declaration from the value API's
constructor for the same form.

**String and `Token<T>` keys behind a position rule.** A key that is always a type needs no rule to
tell it from a const (ADR-0059).
