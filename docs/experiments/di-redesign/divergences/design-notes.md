# Design notes raised while folding `ConstructError`

Points in DESIGN.md that the fold of the twelfth response surfaced, outside that response's scope.
Each goes to the user's sign-off with the build's divergences.

1. **`Sites::site` is a chosen name.** No document spells a method on `Sites`. §13's hand-written
   `Construct` example calls `s.site::<Dep<PgPool>>()`, and the Sites row of §13's table now names it.
2. **`Resolver::dep` and the qualifier.** §13 lists `Resolver::dep` but nothing calls it; the new
   example writes `r.dep::<PgPool>().await?`. If `dep` takes a `Q` parameter, that spelling needs the
   qualifier written or a second method, since a function's generic parameters take no defaults.
   `Dep::<PgPool>::read(r)` is the spelling the `Site` trait already supports.
3. **§13's pool binding binds a `Result`.** `m.singleton(..)` around sqlx's `connect_lazy(..)`, which
   returns `Result<PgPool, Error>`, binds the `Result` under the key, so the `.ready(|pool:
   Dep<PgPool>| ..)` beside it reads a key nothing binds. `try_singleton`, or sqlx's
   `connect_lazy_with`, which returns the pool, fixes it.
