# 0058 — A provider declaration is a value, written through one macro

Status: proposed

A provider is declared by `provide!(key => source)`, one macro whose every form expands to a public
constructor of the value API, so the builder and a function building declarations take the same
values. A token in a declaration is a value, never a spelling a macro reads. A key is single-bound
unless a binding under it is declared with `into`; a collection is keyed by its element type. An
enhancer role comes from the enhancer's own type or from the key's. A provider factory is always
async. `#[new]` is the one constructor. What `build` returns is a `Registration`.

## Context

**Two vocabularies over ten code paths.** A provider is declared by `provide!` with five markers, or
by one of `provider_value!`, `provider_factory!`, `provider_alias!` and `provider_token!`. They are
not alternatives: `provide!` classifies its input, renders it back to tokens and hands it to the
other four. Under both, `multi` is a second copy of every declaration kind rather than a property of
a binding, so five acts by two channels are ten code paths.

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
`provider_*!` macros as the declaration surface. Each of its forms expands to a public constructor
(`Provide::value`, `Provide::factory`, `Provide::alias`, `Provide::into`, a type's own declaration),
so `builder.provider(provide!(..))`, `#[module(providers: [..])]` and an integration crate's
`for_root` function build the same values. The macro is sugar over the value API, never beside it.
It is a proc macro: it reads the source's syntax to choose a form, and it points each refusal at the
user's value or closure. Its grammar is led by keywords and position, never by how a name is cased.

| Written | Means |
| --- | --- |
| key: a string, a `Token<T>` or `Many<T>` const, `dyn Trait`, or none | where the binding goes; none keys it by what the source builds |
| `into` before the key | a contribution to the collection under that key |
| a bare name as the source, whatever its case | that type's own declaration, the rule `providers: [Db]` uses |
| an inline closure / any other inline expression | a factory / a value |
| `factory f`, `value C` | a factory or value held in a variable or const |
| `alias K` | a second name for an existing binding |

```rust
providers: [
    Db,                                                           // the type declares itself
    provide!("app.port" => 3000u16),
    provide!(tokens::API_KEY => value key),                       // a value held in a variable
    provide!(async |cfg: Config| Db::connect(&cfg.url).await),    // keyed by Db
    provide!(dyn Logger => ConsoleLogger),                        // a trait bound to its implementation
    provide!("app.log" => alias tokens::LOGGER),
    provide!(tokens::AUTH => HeaderGuard("x-auth")),             // a guard, from the key's type
    provide!(into APP_GUARD => Auth),                             // a global guard, every transport
    provide!(into dyn Guard<HttpContext> => RateLimit(100)),      // a global guard, HTTP
    provide!(into dyn Plugin => A {}),
    provide!(into dyn Plugin => async |d: Dep| B(d)),
    provide!(into dyn Plugin => C),
    provide!(into dyn Plugin => factory make),                    // a factory held in a variable
    provide!(into tokens::LEGACY => A {}),                        // a named collection
]
```

The macro keeps what the macros it replaces lacked: a token stays a value, one vocabulary serves
every place a declaration is written, and no role is detected by probing a value's type. A factory
closure is emitted as written, its dependencies read from its parameter types through a trait
implemented per arity, and the macro writes only the conversion of its output.

**Single or many is a property of the key.** A key is single-bound unless a binding under it is
declared with `into`, and a single binding and an `into` on one key fail `create` naming both
(ADR-0057). A collection is keyed by its element type: `#[inject] plugins: Vec<Arc<dyn Plugin>>`
asks for the collection `dyn Plugin` the way `#[inject] db: Arc<Db>` asks for `Db`. A second
collection over one trait is a named `Many<dyn Plugin>` const. The macro writes the `Arc<V>` to
`Arc<dyn Trait>` cast at the call site and hands `Provide::into` the converted item, so no trait
needs a declaration to be collected. Contributions reach other modules through exports, as single
bindings do, in declaration order.

**A token is an `impl IntoToken`.** In value position every spelling works: a qualified const, a
bare const, a runtime `String`. Nothing classifies a path; the compiler rejects a key that
implements no `IntoToken`, and `#[diagnostic::on_unimplemented]` carries the message. A typed token
is load-bearing: `provide!(tokens::API_KEY => 42u32)` where `API_KEY: Token<String>` fails to
compile, and `provide!("app.key" => 42u32)` compiles. Token names follow a prefix convention,
`"billing.plugins"`, and two bindings that collide on one name fail `create` (ADR-0057).

**A role comes from a type: the enhancer's own, or the key's.** An `#[injectable]` enhancer
registers its roles, and rebinding it under another key forwards them. A key typed with a role
trait makes whatever is provided under it take that role, and a value that does not implement it
fails to compile:

- a `Token<dyn Guard<HttpContext>>` is a single guard, applied where a route names it;
- the unnamed collection `dyn Guard<HttpContext>` is HTTP's global guards;
- `APP_GUARD`, `APP_INTERCEPTOR` and `APP_ERROR_HANDLER` are the every-transport global sets, and
  a contribution under one has to implement the role for all four transports.

A string token, or a token typed with a data type, carries data and registers no role.

**`#[module]` and the builder take the same expressions.** All four keys parse expressions, and
`exports:` takes both forms the builder's export methods have. The builder has one
`.provider(value)` and one `.controller(value)`. The attribute and the builder are two syntaxes for
one list. An `Extension<T>` needs no declaration, as `Extensions` needs none: the container answers
any `Extension<T>` token from the execution's bag.

**A provider factory is always async.** The bound is `F: Fn(A..) -> Fut, Fut: Future<Output = R> +
Send`, which accepts every async spelling and refuses a sync closure. `AsyncFn` accepts the same
set, but its future type is unstable, so `Send` cannot be named on it. `Provider::resolve` and
`ProviderFactory::build` are already async; the surface becomes consistent with the SPI.

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
| Recipe: token, dependencies, build | `ProviderFactory` |
| What answers a resolution | `Provider` |
| What `build` returns: an instance and its roles | `Registration` |
| Marking a type a provider | `#[injectable]` |
| Marking a dependency | `#[inject]` |
| A type's own declaration | `T::provide()`, or a bare name in `provide!` |
| Binding under another key | `provide!(key => T)`, or `T::provide().under(key)` |
| Contributing to a collection | `provide!(into key => source)`, or `Provide::into(key, item)` |
| A named collection | `Many<T>` |

`provide()` is a trait method, so `DeclaresProvider` goes in the prelude with `provide!`.

**Nothing is kept for compatibility.** The crate is unpublished and its first release will be a
beta. The four `provider_*!` macros and the old `provide!` grammar are deleted rather than
deprecated, `init = "…"` is deleted without a migration error, `Injectable` is renamed without an
alias, and `__ulo_provider_factory` leaves the public surface.

## Consequences

- Every provider declaration in the tree, and every page showing the old `provide!` or one of the
  four macros, is rewritten.
- A trait is collected with no declaration of its own; a contribution that does not implement it
  fails to compile.
- `APP_GUARD` and `APP_INTERCEPTOR` stop meaning HTTP alone, and `APP_ERROR_HANDLER` joins them. An
  enhancer implementing its role for one transport contributes to that transport's collection and
  fails to compile under a constant.
- A token spelled as a qualified const, a bare const or a runtime string works in every position.
- A module can export a string-token provider, a generic provider and a provider from a submodule,
  and declare a path-qualified controller.
- A sync factory gains one word, `async`.
- A struct's dependencies have one source, its `#[new]` signature.
- A value becomes an enhancer only where a type says so. A plain guard value provided under a
  string token registers no role, and a `#[use_guards]` naming that token fails startup with the
  registry's not-found diagnostic. A value that does not implement `Guard` fails to compile under a
  guard token.

## Roads not taken

**One grammar for the five markers over the four macros.** It leaves the ten code paths and the
token as a token tree.

**Two parallel vocabularies with one documented as preferred.** Each keeps its own code path, and
the call sites split across both.

**Fixing each token surface as it is reported.** It leaves five surfaces with five subsets, and the
defect has been rediscovered from two directions already.

**Documenting the module grammar's limits.** It sends an application needing a string-token export
to `DynamicModule`.

**A marker type parameter over sync and async factories**, ambiguous at the call site for an async
closure, and **two entry points**, which compiled a provider of futures until an `Output: Clone`
bound was added and still leaves two doors for one act.

**`init = "…"` kept beside `#[new]`**, which keeps two sources of dependencies for one struct.

**Detecting a value's role inside the constructor.** Stable Rust cannot ask a generic value
whether it implements a trait. **Naming the role at each declaration** (`.guard::<Http>()`) adds a
modifier per role where the token already says it. **A `#[module]` syntax of its own**, emitting the
same probes, would work in one of the two places a declaration is written and leave the builder
without roles.

**Values alone.** A contribution to a collection would need `Arc::new` around every value and an
`as Arc<dyn Trait>` in every factory body, and a type's own declaration could not contribute at all.
**A `.multi::<dyn Trait>(name)` modifier** needs `Unsize` and has no body that compiles on
stable. **A declaration once per collected trait** (`collects!(dyn Plugin)`) and **a block declaring
every token** both work, and both keep multi as a separate step a user takes before a collection
exists.

**A coercion closure at each contribution** (`|p| p`) repeats at every call the cast the macro
writes.

**`provide!` probing a value's type for a role.** The macro would register a role the constructor it
expands to cannot, and `provide!(k => v)` would build a different declaration from
`Provide::value(k, v)`.
