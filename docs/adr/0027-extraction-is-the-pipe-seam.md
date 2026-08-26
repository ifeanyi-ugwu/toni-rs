# 0027 — Extraction is the seam a pipe was reaching for

Status: accepted

## Context

`Pipe<C, R>` was the fourth enhancer role, named after NestJS's `PipeTransform` and registered the
same way as the other three: `#[use_pipes]` on a handler or its impl, `use_global_{http,rpc,ws}_pipes`
on the factory, `APP_PIPE` for the token form. It ran on HTTP, RPC and WebSocket. gRPC never had one,
because a proto message is method-typed and cannot sit in a non-generic `GrpcContext`.

A Nest pipe receives the argument the handler is about to be given and returns the one the
handler gets. That is the whole point of the role: `ParseIntPipe` turns `"42"` into `42`,
`ValidationPipe` reads the DTO's decorators and rejects what fails them. It exists because
TypeScript's types are erased before the request arrives, so the framework has to rebuild them at
runtime from `design:paramtypes`.

toni's pipe could not do that, and the reason is structural rather than incidental.

### It ran on the wrong side of extraction

The pipe loop sat between the interceptor chain and `Route::execute`. Extraction happens inside
`execute`, in the code the handler macro generates. So a pipe had finished running before any handler
argument existed. There was nothing to transform yet.

### It could not reach the input either

Every context hands out shared references: `HttpContext::request` borrows the parts, `RpcContext::data`
borrows the call's payload, `WsContext::message` borrows the frame. None of them has a setter. A pipe
could read, and it could put a value in `ctx.extensions()`, but it could not change what the handler
would go on to read.

The one exception made this worse rather than better. `HttpContext::take_request` takes `&self`, so a
pipe that reached for the body took it — and every body extractor on that route then failed with
`BodyAlreadyRead`. The shape that looked most like a transform was the one that broke the handler.

### What was left duplicated two other roles

Subtract the transform and a pipe could do two things: attach to `ctx.extensions()` and continue, or
return `Some(R)` and answer. A `Guard<C>` does the first, and does it `async`. An `Interceptor<C, R>`
does the second by returning without calling `next`, and does that `async` too. Sync-ness was the only
property a pipe held alone, and it is the absence of a capability, not one.

The framework's own documentation had noticed. The pipes page taught validation through
`Validated<Json<T>>` and parsing through `Path<i32>`, then illustrated "a transforming pipe" with an
example that writes to the extension bag — a guard, spelled differently. No pipe in this repository
did a job outside a test fixture.

### The same shape, twice, in the body-DTO seam

`Route::get_body_dto` returned a `Box<dyn Validatable>` for the dispatcher to validate before the
handler ran, rendering a 400 when it failed. ADR-0016's shared-handle change removed the half that
stored the DTO on the context and kept this call, on the reading that its `Err` arm was live.

It was not. `Validatable` has no implementor in the workspace, the controller macro emits `None` for
every route, and the function that could have built one is uncalled and returns `None`. The branch has
never executed. It is the same idea as a pipe — validate the input somewhere other than where the
input is named — and it had already decayed further.

## Decision

### The `Pipe` role is removed

The trait, the per-transport entry and factory types, the `ProviderRole` variants, the registry slots,
the container globals, `use_global_{http,rpc,ws}_pipes`, the `APP_PIPE` token, `#[use_pipes]`, and the
three dispatcher loops all go. `PipelineSegment::Pipe` goes with them: a segment that cannot run
cannot panic.

### Extraction is where a value is parsed, validated, or refused

`FromContext<C>` is the seam a pipe was reaching for and could not reach. An extractor is handed the
raw input, produces the typed value the handler receives, and refuses by returning `Err`. It is
per-parameter rather than per-request, and the handler's signature says which rules applied.

The mapping is total:

| A pipe was used to | Now |
| --- | --- |
| Parse a param into a typed value | `Path<T>` / `Query<T>` — serde parses, a bad value is a 400 |
| Validate a body or payload | `Validated<Json<T>>`, `Validated<Payload<T>>` |
| Supply a default for a missing field | `#[serde(default)]`, or `Option<T>` |
| Normalise a field in place | `#[serde(deserialize_with = …)]`, or a newtype with `#[serde(try_from)]` |
| Refuse on policy | `Guard<C>` — rejection is a `GuardRejection`, reshaped by `#[catch(GuardRejection)]` |
| Answer without running the handler | `Interceptor<C, R>` — return without calling `next` |
| Attach a computed value for the handler | `Guard<C>` writing to `ctx.extensions()` |

### `Validated` is not HTTP-only

`Validated<E>` implements `FromContext<C>` for whatever context its inner extractor reads from, and
`Payload<T>` joins the extractors it can wrap. `Validated<Payload<T>>` is the WebSocket and RPC
spelling of `Validated<Json<T>>`, so removing the pipe does not leave those two transports without a
declarative way to validate. A wrong pairing — `Validated<Query<T>>` in a WebSocket handler — stays a
trait-bound error.

### The body-DTO seam goes in the same cut

`Validatable`, `Route::get_body_dto`, the unreachable 400 branch, and the macro plumbing that fed it a
permanent `None` are removed. What it modelled is `Validated<E>`.

## Consequences

- Breaking. `#[use_pipes]` no longer resolves, `Pipe` is not importable, and the three
  `use_global_*_pipes` methods are gone from `ToniFactory`. The framework is pre-release, so there is
  no upgrade path to keep and none is provided.
- `PipelineSegment` loses a variant. It is `#[non_exhaustive]`, so a downstream `match` already needed
  a wildcard arm.
- `toni::traits_helpers::validate::Validatable` is gone from the public API, and `Route` loses
  `get_body_dto` — a breaking change for a hand-written `Route`, of which the two GraphQL controllers
  were the only ones.
- Enhancer registration is now three roles per transport instead of four, everywhere: the detection
  probes, the emission table the macros read, the resolvers, and the wrappers.
- An extraction failure on RPC or WebSocket renders as `Internal`, where a pipe could answer with any
  error it liked. This is how every extraction failure on those transports already rendered — the
  removal inherits it rather than causing it, and correcting the kind is its own change.
- ADR-0016's treatment of `Pipe` is superseded: the sections deciding that a pipe answers by
  returning `Some`, and that a pipe's answer is the reply rather than an error to render, describe a
  role that no longer exists. The rest of 0016 stands.

## Roads not taken

**Keeping `#[use_pipes]` as a compile error that names the replacement.** Worth it when someone's build
would otherwise break with `cannot find attribute`. No released version has the attribute, so the stub
would have no reader.

**Giving `Pipe` the position it was named for — after extraction, over the extracted value.** That is a
different feature: it would have to run per-parameter, be generic over the parameter's type, and sit
inside generated code rather than in the dispatcher. At which point it is `FromContext` with a second
name.

**Keeping pipes for the sync fast path.** A pipe cost a direct call where an interceptor costs a boxed
future. The saving is one allocation per registered enhancer per request, against a role that no code
in the repository used.
