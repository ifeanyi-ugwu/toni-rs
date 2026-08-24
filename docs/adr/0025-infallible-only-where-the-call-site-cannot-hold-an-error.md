# 0025 — An API is infallible only where its call site cannot hold an error

Status: accepted

## Context

One error class had three different fates, decided by nothing but which function it occurred in.
`on_module_init` and `on_application_bootstrap` share a signature (`InitResult`) and the scanner
wraps both failures into the same `HookFailed` variant, carrying the module and hook name:

| Hook | Where it runs | What happened |
| --- | --- | --- |
| `on_module_init` | `ToniFactory::create` | panic, discarding the variant |
| `on_application_bootstrap` | `ToniApplication::bind` | returned as `Err` |
| `on_application_bootstrap` | `create_application_context` | logged and swallowed |

The third is the failure ADR-0024 refused for adapters, in a different function: a CLI tool or
worker received a context that reported success after its bootstrap hooks failed.

### The panicking constructors are not the same case

`ConfigModule::new` panics, and so does every database module's `for_root` factory. That looks
like the same inconsistency and is not. Their call site is a module declaration:

```rust
#[module(imports: [ConfigModule::<AppConfig>::new(), SeaOrmModule::for_root(url)])]
```

That is an expression list inside a macro. There is no `?` available, and returning `Result` would
only move `.unwrap()` into the declaration — a panic with worse ergonomics and less information.
`ConfigModule` already offers `from_env` for callers who want the error, and it is a statement-position
API.

`create` has no such constraint. It is called from `main`, from a test, or from a spawned task, and
all three can hold a `Result`.

## Decision

### An API is infallible only where its idiomatic call site cannot hold an error

This is the rule that decides every future constructor. Absent that constraint, fallible is the
default, and no infallible twin is added for convenience.

### `create` and `create_application_context` return `Result`

All four entry points — `create`, `create_with`, `create_application_context`,
`create_application_context_with` — return `Result<_, StartupError>`. The standalone context runs
its bootstrap hooks on the way and propagates their failure rather than logging it.

### The error type is `StartupError`

`BindError` is renamed. It now spans both startup phases, and a `create` that binds nothing should
not hand back something called a bind error. `Adapter` remains reachable only from `bind`; the type
is wider than either function's range, which `#[non_exhaustive]` already absorbs.

### Module-declaration constructors stay panicking

`ConfigModule::new` and the `for_root` families are correct as they stand, by the rule above. They
are not a gap to close later.

### Provider construction still panics

`ProviderFactory::build` returns the instance directly:

```rust
async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable;
```

A factory that cannot build its instance has nowhere to put an error, so a database module that
cannot reach its server ends the process. Making that path fallible means changing the SPI every
integration and every macro-generated factory implements. Recorded here as the boundary of this
decision rather than left to be discovered: `create` returning `Result` does not mean every failure
inside it is returned.

## Consequences

- A failing `on_module_init` hook is matchable, and names its module and hook rather than arriving
  as panic text.
- A standalone context no longer reports success after its bootstrap hooks failed.
- Roughly 150 call sites gain `?` or `.unwrap()`. Examples with a `Result` main use `?`, which is
  what a reader should copy.
- The three refusal tests that asserted a panic assert an `Err` instead, and no longer need to catch
  unwinding.
- An unreachable database is still a panic from inside `create`, and which integrations dial at
  construction time still varies between them.

## Roads not taken

**A fallible/infallible pair across the API.** Every constructor gaining a `try_` twin doubles the
surface for a benefit that exists only where `?` is unavailable, and the pair pattern is a workaround
for that constraint rather than a virtue to spread.

**Splitting by failure class: `create` panics because it only reads your code, and `on_module_init`
moves to `bind` so everything environmental sits behind one `Result`.** Conceptually cleaner, and it
would have cost no call-site churn. It does not survive contact with what `create` does:
`create_instances_of_dependencies` runs every singleton's factory, and those open sockets. The split
would also require moving provider instantiation to `bind`, and eager construction during `create` is
what lets `app.get::<T>()` resolve before anything is bound.

**Keeping a panicking `create` beside the fallible one.** Exactly the pair the first decision exists
to prevent, at the one call site that has somewhere to put an error.
