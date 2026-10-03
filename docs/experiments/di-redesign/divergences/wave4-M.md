# Divergences: wave 4, agent M (`value = expr?` in an `into` list)

Every place M's code departs from `DESIGN.md`, or fills a gap the design leaves that a user would
see. Each entry gives what the design says, what M wrote, and why. What the expansion names and
requests for core files follow the entries.

Files: `crates/ulo-macros/src/module_attr/providers.rs`, `crates/ulo-macros/src/lib.rs` (the
`#[module]` doc). `crates/ulo/src/__private.rs` is unchanged.

Superseded by this wave: wave 3's M 3 (`value = expr?` a compile error), its limit (a configured
module's fallible value built from its own fields has no `#[module]` spelling) and its request R2.

## Entries

### 1. `value = expr?` lowers to `Contribute::try_value`

- **Design:** §4 lowers a providers list's `expr?` to `m.try_value(expr)` and is silent on
  `value = expr?` in an `into` list.
- **M:**

  ```rust
  m.contribute::<K>().try_value(::core::result::Result::map(
      expr,
      |v| -> ::ulo::__private::Arc<K> { ::ulo::__private::Arc::new(v) },
  ));
  ```

  spanned at `expr`, as the `value = expr` arm is, so `#[track_caller]` on `try_value` records the
  entry's location. An `Err` reports at `wire()` beside a providers list's `expr?`, and
  `value = Plugin::new(&self.cfg)?` in a configured module now has a spelling: `register` takes
  `&self`.
- **Coercion:** `Result<Arc<T>, E>` does not coerce to `Result<Arc<dyn Trait>, E>`, so
  `Result::map(expr, Arc::new)` fails for a `dyn` key. The closure's return type is annotated
  `Arc<K>`, which makes the tail `Arc::new(v)` a coercion site where `Arc<T>` unsizes to `Arc<K>`.
  `Arc::new(v) as Arc<K>` was rejected: a cast a coercion could replace is what the
  allow-by-default `trivial_casts` lint reports, and a crate denying it would fail at the user's
  span. The annotation is not a cast.
- **UFCS:** `Result::map(expr, ..)` rather than `expr.map(..)`. An `expr` that is not a `Result`,
  an `Option` with `?`, then fails as a type mismatch on `expr` itself, as a providers list's
  `expr?` does at `m.try_value`; the method form would accept `Option::map` and fail one call
  later at `try_value`.
- **Limit:** the macro writes the `Arc::new`, as for `value = expr`, so an `expr?` already yielding
  an `Arc<T>` becomes `Arc<Arc<T>>`, which coerces to `Arc<K>` only when `Arc<T>` implements the
  key's trait.

## What the expansion names

- `Contribute::try_value(self, Result<Arc<U>, E>)` with `E: Into<BoxError>`, on the shared
  `impl<'m, U, Q, M>`, reached from `m.contribute::<K>()` (`Plain`).
- `::core::result::Result::map`, `::ulo::__private::Arc` (present since wave 3).

## Requests for core files

None. W's `Contribute::try_value` in `crates/ulo/src/binding/contribute.rs` matches the signature
the expansion calls, on the shared `impl<'m, U, Q, M>`, with `#[track_caller]`.
