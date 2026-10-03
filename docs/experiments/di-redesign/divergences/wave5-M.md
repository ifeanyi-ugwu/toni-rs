# Divergences: wave 5, agent M (`with = closure` in an `into` list)

Every place M's code departs from `DESIGN.md` or `RESPONSE.md`'s fifteenth response, or fills a
gap they leave. Each entry gives what the design says, what M wrote, and why. What the expansion
names and requests for core files follow the entries.

Files: `crates/ulo/src/__private.rs` (the `into` arm of `factory`),
`crates/ulo-macros/src/module_attr/providers.rs` (the lowering and its module doc),
`crates/ulo-macros/src/lib.rs` (the `#[module]` doc).

Superseded by this wave: wave 3's lowering of `with = closure` to `singleton`/`try_singleton`.

## Entries

### 1. `with = closure` lowers to `Contribute::with` / `Contribute::try_with`

- **Design:** the fifteenth response lowers `with = closure` to one value-API method whose record
  says "declared by closure", the core resolving the scope at freeze from the role.
- **M:** the expansion is unchanged in shape:

  ```rust
  {
      #[allow(unused_imports)]
      use ::ulo::__private::factory::{FallibleContribution as _, PlainContribution as _};
      (&::ulo::__private::factory::Probe::new(<closure, wrapped in `async move`>))
          .contribute_with::<K, _>(&mut *m, |a| a);
  }
  ```

  and the two probe impls call `m.contribute::<U>().try_with::<Args, F, T, E>(factory, coerce)`
  (`Probe<F, Args>`, output a `Result`) and `m.contribute::<U>().with::<Args, F>(factory, coerce)`
  (`&Probe<F, Args>`, any other output). The autoref ranking, the `Cell` holding the factory, the
  `Built` associated type and `wrap_async` are as wave 3 wrote them.
- **Rename:** the probe method `contribute_singleton` is now `contribute_with`, since it no longer
  builds a singleton. The traits keep their names, `FallibleContribution` and `PlainContribution`.

### 2. A providers-list closure still lowers to `singleton` / `try_singleton`

- **Design:** the fifteenth response covers `with` in an `into` list and is silent on a bare
  closure in a providers list.
- **M:** unchanged, through `Fallible`/`Plain` and `register_singleton`. A providers-list closure
  binds the single key its output names, never a role key, so it is always a provider, and the
  role rule gives a provider a singleton. Lowering it to a role-resolved method would record
  "declared by closure" for an entry whose role is known at expansion.

### 3. `#[guards(with = ..)]` is unchanged

- **Design:** a method- or controller-tier closure is built per execution through the enhancer
  spec (item [26]).
- **M:** no edit to `crates/ulo-macros/src/enhancers/`. Its module doc, "by closure, built per
  execution", already states the role rule for an enhancer.

### 4. Docs

- `providers.rs` module doc: the `into` table row names `.with(closure, |a| a)` or `.try_with(..)`,
  and the paragraph states that the expansion declares no scope, the record says "declared by
  closure", an enhancer is built per execution and a provider contribution is a singleton that
  `wire()` refuses when it reads execution data. The `value` sentences that shared that paragraph
  are now their own paragraph.
- `lib.rs` `#[module]` doc: `with = |..| ..` is "a factory written as in `#[guards]`, `try_with`
  when its output is a `Result`", followed by a paragraph stating the role rule.
- `__private.rs`: the `into` arm's doc says the contribution is declared by closure and carries
  no scope.

## What the expansion names

- `Contribute::with::<Args, F>(self, factory: F, coerce)` with `F: Factory<Args>`,
  `F::Output: Send + Sync + 'static`, and
  `Contribute::try_with::<Args, F, T, E>(self, factory: F, coerce)` with
  `F: Factory<Args, Output = Result<T, E>>`, `T: Send + Sync + 'static`,
  `E: Into<BoxError> + Send + 'static`; `coerce: impl Fn(Arc<_>) -> Arc<U> + Send + Sync + 'static`.
  Reached from `m.contribute::<K>()` (`Plain`). The returned handle is discarded, so its type does
  not constrain the expansion.
- `::ulo::__private::factory::{Probe, FallibleContribution, PlainContribution}` (present since
  wave 3).

## Requests for core files

- **R1 (met):** `with` and `try_with` carry `#[track_caller]`, as `singleton` does. The probe's
  `contribute_with` is `#[track_caller]` and the expansion is spanned at the closure, so the
  entry's location reaches the record only if the core method reads `Location::caller()` through
  the same chain. The core's methods in `crates/ulo/src/binding/contribute.rs` carry it and pass
  `Location::caller()` to `push_by_role`; their generic order and bounds match the probe's calls.
  They return `Handle<'_, _, Contribution<Auto, Open>>`, which the probe discards.

## Check

`cargo check -p ulo -p ulo-macros`: 0 errors, 19 warnings, all in `ulo` files this wave did not
touch. It compiles the probe impls against the core's `with`/`try_with`. It expands no `into`-list
`with` entry: no file in the workspace writes one, and no test was run.
