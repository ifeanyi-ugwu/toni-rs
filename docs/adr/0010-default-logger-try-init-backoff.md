# 0010 — Default logging: installed on create with `try_init` back-off, no runtime knob

Status: accepted

## Context

Every diagnostic the framework emits goes through the `tracing` facade: bootstrap progress, guard
rejections, panic recoveries, adapter lifecycle events — and the error logged just before `create`
aborts the process on a failed initialization. `tracing` discards all of it when no global
subscriber is installed. An application that never sets one up runs silent, and a broken
application exits with code 1 and no output at all. Every example carried the same subscriber
boilerplate to compensate.

The ecosystem splits on whose job that setup is. Minimal frameworks (axum, actix-web) leave it to
the application. Batteries-included frameworks install a default: NestJS — the framework's
architectural model — logs out of the box and takes `logger: false` to opt out. Rocket also
installs its own logging and is the cautionary case: a default that is hard to replace is a
long-standing source of friction there.

Any default has to leave three off-ramps open: replacing it with a custom backend (JSON output,
OpenTelemetry), silencing it at runtime, and removing the dependency entirely.

## Decision

`ToniFactory` installs a default subscriber at the top of `initialize`, the funnel shared by
`create_with` and `create_application_context_with`: a `tracing-subscriber` fmt writer to stderr,
filtered by `RUST_LOG` with an `info` fallback, installed via `try_init()`.

`try_init` is the load-bearing choice: it fails when a global subscriber is already set, and the
failure is ignored. A subscriber the application installs before `create` always wins, with no
configuration and no API to discover. The remaining off-ramps are `RUST_LOG=off` for runtime
silence and the default-on `logger` cargo feature — gating the `tracing-subscriber` dependency —
for compiling the default out.

Output goes to stderr, not stdout: `create_application_context` serves CLI tools and workers whose
stdout is program output.

There is no runtime knob. A `disable_default_logger()` builder method was considered for Nest
parity and rejected: every identified case is covered by one of the three off-ramps, and Nest's
`logger: false` guards a logger abstraction (`LoggerService`) that toni does not own — in the
tracing ecosystem, install-your-own-subscriber is the idiom, and the back-off honors it. Adding a
builder method later is non-breaking; shipping one now would be surface without a case.

## Consequences

- Every library crate depending on `toni` must declare `default-features = false`. Cargo features
  are additive across the dependency graph: one adapter pulling `toni` with default features
  re-enables `logger` for the application and breaks its opt-out. All 25 dependent crates in the
  workspace declare it; new crates must follow. Binary and test packages (`examples`,
  `integration-tests`) keep defaults on purpose, exercising the default path.
- The feature flag is dependency trimming, not the primary switch — flag-based opt-out is
  unreliable exactly because of that unification. The `try_init` back-off is the mechanism; the
  flag removes the compiled weight.
- Examples and quick starts need no logging setup; the subscriber boilerplate the examples carried
  is deleted.
- Test harnesses that install their own subscriber (the integration suite's `init_tracing`,
  filtered to `toni=error`) compose with the back-off in either install order — whichever
  `try_init` runs first wins, and both ignore losing.
