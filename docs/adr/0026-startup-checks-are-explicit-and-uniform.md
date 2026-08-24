# 0026 — Connectivity is verified by an explicit check, not by the driver's own dial

Status: accepted

## Context

The database integrations disagreed about when an unreachable server was discovered, because each
inherited whatever its driver does while constructing a pool:

| | construction did | an unreachable server surfaced |
| --- | --- | --- |
| seaorm | `Database::connect` | at startup, after sqlx's 30-second acquire timeout |
| redis | `ConnectionManager::new` | at startup, after six retries with the driver's own backoff |
| sqlx | `Pool::connect` | at startup, after a 30-second acquire timeout |
| mongodb | `Client::with_options` | on the first query |
| diesel | deadpool `build()` | on the first query |

The same wrong connection string therefore failed a deployment for three integrations and returned
500s to users for the other two. Where it did fail, it took half a minute to say so, which is longer
than many readiness deadlines — the container is killed before the diagnosis arrives.

### Both directions were available

Making them uniformly eager or uniformly lazy were both implementable: `ConnectOptions::connect_lazy`,
`ConnectionManager::new_lazy_with_config` and `PoolOptions::connect_lazy` exist, and mongodb and
diesel already construct without dialing. The choice was not constrained by the drivers.

### Uniform eagerness would have inherited three different behaviours

Adopting each driver's dial as the check keeps three timeout policies under one name, none of them
configurable through this framework, one of them absent. "All integrations verify at startup" would
have described three schedules.

## Decision

### Construction is lazy everywhere; a separate check contacts the server

Every integration configures its pool or client without touching the network, and the connection
provider's `on_module_init` runs the verification. That hook already returns `InitResult`, which
ADR-0025 made surface as `StartupError::HookFailed` naming the module, so the reporting path needed
nothing new.

### The policy is one type, in core

`StartupCheck` carries attempts, delay between them, and a per-attempt timeout, and runs a probe
against them. An application configures one type regardless of which database it uses, and every
integration fails on the same schedule.

Drivers that retry internally are told not to: redis's connection manager would otherwise run its
own six attempts inside one of ours.

### It is on by default

Default: three attempts, two seconds apart, five seconds each — about nineteen seconds at worst,
which covers a database container that starts a few seconds after the application and stays inside a
typical readiness deadline.

On by default because the careless path should be the safe one. An application whose readiness probe
does not report its database gets a startup failure naming the module rather than errors on the
first request that needs it. Turning it off is a decision made in the code that turns it off.

### Constructors return `CheckedModule`

```rust
SeaOrmModule::for_root(url)                                       // checked, with the defaults
RedisModule::for_root(url).without_startup_check()
SqlxModule::postgres(url).with_startup_check(StartupCheck::default().attempts(5))
```

A wrapper around `DynamicModule` rather than a second constructor per combination: there are already
two constructors per connection kind, and a `_with` variant of each doubles that for one knob. It
holds the module built with the current check and rebuilds when the check changes, because the check
is folded into the provider factories rather than read later.

### `StartupCheck::run` sits behind a feature

Core owns no runtime, and the retry timer needs one. The `startup-check` feature pulls `tokio/time`;
the integrations enable it, and nobody else acquires a runtime dependency by depending on toni.

### Prisma does not participate

`PrismaModule::for_root<C>(connect)` takes a closure returning the generated client by value. There
is no operation to call on an arbitrary `C` to see whether it works, so this one integration still
discovers an unreachable database on first use, and says so in its documentation.

## Consequences

- The same wrong connection string now fails startup for every integration but Prisma, with the
  module named and the credentials redacted.
- mongodb and diesel applications that started against an absent database no longer start. That is
  the change, and `without_startup_check()` restores the old behaviour where it was wanted.
- An unreachable server is reported in seconds rather than after a driver's 30-second timeout.
- Startup gains a bounded delay when a database is slow to accept: at most `StartupCheck::worst_case`.
- Each integration keeps one place that opens a connection, so the policy is applied once per
  integration rather than once per provider.

## Roads not taken

**Uniform eager construction, using each driver's own dial as the check.** Smaller, and it inherits
three incompatible timeout policies plus one driver that does not dial at all. The knob users would
then want — how long to wait — is not reachable through a shared API.

**Uniform lazy construction with no check, leaving connectivity to a readiness probe.** The cleanest
orchestration story: no crash loop, and the application recovers by itself when the database returns.
It fails the application that has not wired readiness, which is the one most likely to need help, and
it makes the safe outcome depend on configuration outside the code.

**A `_with` constructor per existing constructor.** Two per connection kind become four, across six
crates, for one knob — and a second knob would double it again.
