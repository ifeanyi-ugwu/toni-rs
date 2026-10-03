# Divergences: wave 6, agent M (scope as its own axis)

Every place M's code departs from `RESPONSE.md`'s sixteenth response or the wave-6 brief, or
fills a gap they leave. Each entry gives what the design says, what M wrote, and why. What the
expansion names and requests for core files follow the entries.

Files: `crates/ulo-macros/src/shared/scope.rs` (new), `crates/ulo-macros/src/shared/mod.rs`,
`crates/ulo-macros/src/injectable/args.rs`, `crates/ulo-macros/src/enhancers/mod.rs`,
`crates/ulo-macros/src/module_attr/providers.rs`, `crates/ulo-macros/src/lib.rs`,
`crates/ulo/src/__private.rs` (the `factory` module).

Superseded by this wave: wave 5's statement that a `with` entry's scope follows its role, in
`lib.rs`, `providers.rs` and `__private.rs`, and the enhancer module doc's "by closure, built per
execution" (item [26] as first written).

## Entries

### 1. Enhancer attributes: `with(<scope>) = closure`

- **Design:** `with = f` lowers to `spec.guard_with(f)`, now `Auto` in the core; `with(singleton |
  execution | transient) = f` lowers to `spec.guard_with_in::<Singleton | PerExecution |
  Transient>(f)`; the transport-scoped form takes the same grammar.
- **M:** `Form::With` carries `scope: Option<ScopeArg>`. The generated local fn keeps its
  signature and bounds, and its body is the one line that differs:

  ```rust
  fn get_users<__UloA, __UloF>(spec: &mut ::ulo::EnhancerSpec<Http>, build: __UloF)
  where
      __UloF: ::ulo::Factory<__UloA>,
      <__UloF as ::ulo::Factory<__UloA>>::Output: ::ulo::Guard<Http>,
  {
      spec.guard_with::<__UloA, __UloF>(build);                                // with = ..
      spec.guard_with_in::<::ulo::scope::PerExecution, __UloA, __UloF>(build); // with(execution) = ..
  }
  ```

  `interceptor_with_in` and `error_handler_with_in` likewise. The scope path is spanned at the
  scope word. The brief writes `guard_with_in::<S>(f)` with one type argument; the expansion
  names all three, `<S, Args, F>`, in the core's generic order, since it calls from inside a fn
  generic over `Args` and `F`, and Rust has no partial turbofish.
- **Location:** the core's six closure methods are `#[track_caller]` and record
  `Location::caller()`. The local fn is now `#[track_caller]` too, for both `with` forms, and its
  call is spanned at the entry with the callee ident re-spanned there (`set_span`, which keeps a
  raw ident raw). The recorded location is the closure entry; without this it was the handler's
  name, where the call inside the local fn is spanned. The by-type and by-value forms are
  unchanged: their core methods record no location.
- **Transport-scoped:** `http(with(execution) = ..)` parses and lowers the same way. `__handler`
  re-emits the entry as `with(execution) = ..`, the word keeping its span, and
  `__enhancer_specs!` re-parses it.
- **Unchanged:** `wrap_async`, the controller-tier `value` refusal, the method-tier refusal of an
  entry for another transport.

### 2. `into` lists: one probe method with a scope type argument

- **Design:** `with = f` keeps lowering to `with`/`try_with`; `with(singleton | execution |
  transient) = f` to `singleton`/`try_singleton`, `execution`/`try_execution`,
  `transient`/`try_transient`, by the existing autoref ranking. The brief leaves the probe's
  shape to M.
- **M:** the probe method `contribute_with` takes the scope marker as a type argument, and a new
  trait `__private::factory::ContributionScope`, implemented for `Auto`, `Singleton`,
  `PerExecution` and `Transient`, maps the marker to the `Contribute` method. The expansion:

  ```rust
  {
      #[allow(unused_imports)]
      use ::ulo::__private::factory::{FallibleContribution as _, PlainContribution as _};
      (&::ulo::__private::factory::Probe::new(<closure, wrapped in `async move`>))
          .contribute_with::<K, S, _>(&mut *m, |a| a);
  }
  ```

  with `S` = `::ulo::scope::Auto` for `with = ..` and the written marker for `with(<scope>) = ..`.
  The ranking picks `FallibleContribution` (output a `Result`) or `PlainContribution`, which call
  `S::try_contribute::<U, Args, F, T, E, C>(m.contribute::<U>(), factory, coerce)` and
  `S::contribute::<U, Args, F, C>(..)`. Each `ContributionScope` impl calls one pair, with
  explicit type arguments in `contribute.rs`'s order: `into.with::<Args, F>` /
  `into.try_with::<Args, F, T, E>` for `Auto`, `singleton`/`try_singleton` for `Singleton`,
  `execution`/`try_execution` for `PerExecution`, `transient`/`try_transient` for `Transient`.
  A `macro_rules!` writes the four impls from that table.
- **Why one method rather than four:** the scope stays one axis in the expansion too: the macro
  writes one shape for both `into` forms and maps the word to a marker, as it does for an
  enhancer and for `#[injectable(..)]`. The alternative, `contribute_singleton`,
  `contribute_execution` and `contribute_transient` beside `contribute_with` on both probe
  traits, is eight trait methods and eight bodies for the same four calls.
- **`#[track_caller]`:** on every probe and `ContributionScope` method, declaration and impl, so
  the entry's location reaches the record through the extra hop.

### 3. One scope-word parser

- **M:** `ScopeArg` moved from `injectable/args.rs` to `shared/scope.rs`, with `from_ident`,
  `parse_parenthesized`, `path` and `word`. `#[injectable(..)]` and both `with(..)` parsers read
  the word through it, so `#[injectable(execution)]` and `with(execution)` accept the same three
  words and lower to the same markers. `#[injectable]`'s diagnostics are unchanged.

### 4. An unknown scope word is a span error naming the three

- `with(request) = ..`: "expected a scope: `singleton`, `execution` or `transient`", on `request`.
- `with() = ..`: "`with(..)` takes a scope: `singleton`, `execution` or `transient`", on the
  parentheses.
- `with(execution, singleton) = ..`: "`with(..)` takes one scope", on the token after the first
  word.
- A non-identifier, `with("execution")`, gets the first message, spanned where the identifier was
  expected.

The same parser serves the enhancer attributes and the `into` lists. These messages and spans are
read from the code; no expansion exercised them, since this wave ran no tests.

### 5. `value(..)` is refused as taking no scope

- **Gap:** the design gives `value` no scope and says nothing about `value(execution) = ..`.
- **M:** refused in both grammars: "a `value` is built once and shared, so it takes no scope".
  Before this wave, the enhancer attributes answered `value(..)` with "write `value = ..`, or
  `<transport>(value = ..)` to scope it", and an `into` list with its generic "an expression is
  contributed with `value = ..`".

### 6. A scope word in a transport key's place is refused

- **Gap:** `#[guards(execution = AuthGuard)]` or `execution(with = ..)` parses as an entry for a
  transport keyed `execution`. On a method that was already an error. On the impl, an entry for a
  transport no handler has is skipped, so the guard would apply to nothing, unreported.
- **M:** a transport key that is one of the three scope words is a span error: "`execution` is a
  scope, not a transport key; a type declares its scope on the type, `#[injectable(execution)]`,
  and a closure is written `with(execution) = ..`". No transport crate in the workspace declares
  such a key. Drop it if a transport may.

### 7. Vocabulary: "scope" now means lifetime

- `Entry.scope` (the transport key) is now `Entry.transport`.
- "scope key" is now "transport key" in the `ulo-macros` docs (`enhancers/mod.rs`, `lib.rs`).
  No other crate uses the phrase.
- The method-tier diagnostic "an entry scoped to `rpc` applies to nothing; write it unscoped" now
  reads "an entry for `rpc` applies to nothing; write it for every transport". No UI test pins
  either wording.
- "transport-scoped" stays, as the brief uses it.

### 8. A providers-list closure has no scope syntax

- **Gap:** the response puts closures under one scope axis "on types, on closures, in method
  attributes, in into lists, in the value API", and does not mention a bare closure in a
  `providers` list.
- **M:** unchanged: `singleton`/`try_singleton` through `register_singleton`. Under `Auto` a
  provider is a singleton anyway, so the lowering matches the rule. What is missing is a way to
  write a providers-list closure in another scope; `with(execution) = ..` is not accepted there,
  and such a binding needs the value API or a `#[injectable(execution)]` type. Not added: no item
  asks for it, and it would change the providers grammar.

### 9. Docs

- `lib.rs`, `#[module]`: the `into` forms list `with(<scope>) = |..| ..`. The role-rule paragraph
  is replaced by the axis: `with` says how an item is built; `with = ..` declares `Auto` (built
  once unless what the closure reads needs an execution; a provider contribution is a singleton,
  refused if its closure reads execution data); `with(<scope>)` writes the scope instead. A
  separate paragraph states that inference reads what a closure takes, not what it does, and that
  `|| RequestTimer::start()` must be written `with(execution) = ..`.
- `lib.rs`, `#[guards]` (which `#[interceptors]` and `#[error_handlers]` point to): "built per
  execution" is replaced by the same axis, `http(with(execution) = ..)` included, and the same
  `RequestTimer` paragraph.
- `enhancers/mod.rs` module doc: a grammar line for `with(<scope>) = ..` and its lowering, the
  transport-scoped line extended, and a paragraph on the axis and the `RequestTimer` case.
- `providers.rs` module doc: three table rows for the explicit scopes; the paragraph saying the
  expansion declares no scope and the core resolves it from the role is replaced by the probe's
  scope argument and the `Auto` rule.
- `__private.rs`: the `into` arm's doc names `S`; `ContributionScope` has its own.

## What the expansion names

- `EnhancerSpec::{guard_with, interceptor_with, error_handler_with}::<Args, F>(build)`
  (unchanged).
- `EnhancerSpec::{guard_with_in, interceptor_with_in, error_handler_with_in}::<S, Args, F>(build)`,
  `S` one of `::ulo::scope::{Singleton, PerExecution, Transient}`.
- `::ulo::scope::{Auto, Singleton, PerExecution, Transient}`.
- `::ulo::__private::factory::{Probe, FallibleContribution, PlainContribution}`; the probe
  method is `contribute_with::<U, S, C>`.
- Through `ContributionScope`: `Contribute::{with, try_with, singleton, try_singleton, execution,
  try_execution, transient, try_transient}`.

## Requests for core files

- **R1 (met):** `guard_with_in`, `interceptor_with_in` and `error_handler_with_in` take the scope
  as the first type parameter, `<S, Args, F>`, and bound `F` by nothing beyond `F: Factory<Args>`
  and `F::Output: <Role><T>`, plus bounds on `S` alone. The core declares exactly that, with
  `S: ExplicitScope` (implemented for `Singleton`, `PerExecution` and `Transient`, not `Auto`).
  The generated local fn is generic over `Args` and `F` and carries only those two bounds; a
  further bound on `Args` or `F` would have to be mirrored in `entry_statement`.
- **Met by the core without a request:** `Contribute::with`/`try_with` push `Auto` through
  `push_factory`, so the `Auto` impl of `ContributionScope` reaches the `Auto` registration.

## Check

`cargo check -p ulo -p ulo-macros`, run after the core's methods appeared and again after touching
`enhancers/mod.rs` and `__private.rs` to force both crates to rebuild: 0 errors, 19 warnings, all
in `ulo` files this wave did not touch (`app/shared.rs`, `binding/handle.rs`, `binding/mod.rs`,
`dependency/mod.rs`, `graph/mod.rs`, `lifecycle/run.rs`, `module/def.rs`, `module/mod.rs`,
`redact.rs`, `timer.rs`).

It compiles the `ContributionScope` impls and the probes against `contribute.rs`, so the eight
`Contribute` calls and their type-argument order are checked. It expands no `with` entry of
either kind: nothing in `ulo` writes one, and no test was run. The enhancer expansion's call
`spec.guard_with_in::<S, __UloA, __UloF>(build)` and the parser's diagnostics are checked against
the core by reading only.
