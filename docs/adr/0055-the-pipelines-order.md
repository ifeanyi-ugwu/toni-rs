# 0055 — The pipeline's order is the order that was written

Status: proposed

Refined by [ADR-0058](0058-a-provider-declaration-is-a-value.md) and
[ADR-0059](0059-every-key-is-a-type.md): in the `#[use_*]` attributes a bare path is a type, the
enhancer's own or a marker naming a slot, and a value held in a `const` is written `value X`, one
held in a `static` `value &X`.

An enhancer declaration runs in the order written, whatever spelling each entry uses; the error
chain sits above the interceptors on every transport; a guard answers `Result<(), E>`; and the
three enhancer roles take one transport parameter apiece.

## Context

The pipeline's order is decided by how each entry is spelled, and its error seam sits above the
interceptors. This record takes the order and the seam's position relative to the interceptors;
[ADR-0056](0056-the-pre-dispatch-role.md) takes the role that wraps dispatch.

**The order that runs is not the order that was written.** `#[use_guards(AuthGuard{}, RoleGuard)]`
runs `RoleGuard` first. The macro sorts one declaration into a token vector and an instance vector,
and the resolver pushes every token-resolved entry before every entry built at the declaration
site; the interleaving the author wrote is gone before any resolver sees it. The tiers go with it on
HTTP, where the controller-level and method-level attributes are merged into one map before the
split, so a controller-level inline guard runs after a method-level token guard. For error handlers
the declaration order stops mattering at all. `#[use_guards]`'s own documentation states both rules
this breaks — the tier order, and "within each level, guards execute in the order specified" — and
is the only place a caller learns either. The shape is security-sized: an authentication guard
written first and a role guard written second run in that order, or do not, depending on whether one
of them took a constructor argument.

**The inline form cannot reach the per-execution arm that exists for it.** A guard or interceptor
entry is `Ready` (one `Arc`, shared by every execution) or `Factory` (built per execution, through
DI); an error-handler entry has only `Ready`, so an execution-scoped `ErrorHandler` registers no
role at all. For a guard or interceptor a DI token reaches both arms; a value written at the declaration site — `RoleGuard::new("admin")` — reaches only
`Ready`, since the expression is evaluated once with no context to hand it. The form that has to be
abandoned to get per-execution construction is also the only one that takes arguments chosen at the
site.

**A guard's refusal has no reason.** `Guard::can_activate` answers `bool`, so `GuardRejection.reason`
is documented and never set, and a guard that knows why it refused has nowhere to put it.

**The chain answers outside the interceptors.** An interceptor that stamps a header on every response
it returns puts it on a `200` and not on the `418` an error handler recovered the call with. The
chain runs above the whole interceptor stack on all four transports, so an interceptor sees the
`Err` on the way out and never sees what answered it. Nest divides it the same way: its lifecycle
runs interceptors, then exception filters, then the response, and an uncaught exception skips
straight to the filter.

**A transport's error names a kind twice.** Each transport's error keeps named variants that map
onto an `ErrorKind` at render time, so picking a variant picks a kind: a guard refusing a WebSocket
connect raises `WsError::AuthFailed` and renders `Unauthorized`, and the same guard refusing a
message raises `GuardRejection` and renders `Forbidden`.

**Three roles, three spellings of the transport.** `Guard<C>`, `Interceptor<C, R>` and
`ErrorHandler<C, R>` each name the context, and two also name the answer. `InterceptorNext` is a
public trait where the middleware surface has a concrete `NextHandle` over a private trait. The
trait method is `handle_error` where the attribute is `#[catch]`, and `MiddlewareResult` sits beside
`HttpHandlerResult`.

## Decision

**A declaration's order is the contract.** The entries a declaration names run in the order written,
whatever spelling each uses. The macro carries one ordered vector whose element is a token or a
value, and the resolver preserves it; the token vector beside the instance vector goes.

**Three spellings, three lifecycles, told apart by grammar alone:**

| written | is | lifecycle | DI |
| --- | --- | --- | --- |
| a bare path — `AuthGuard` | a type name | whatever its scope says | yes |
| a value expression — `RateLimiter::new(100)`, `X{}` | a value | built once at startup, shared | no |
| a closure — `\|ctx\| …`, `\|_\| …` | a constructor | built per execution, at this site | through `ctx` |

The closure is the third spelling and resolves to the `Factory` arm, which is how a declaration site
reaches per-execution construction. Error handlers gain the `Factory` arm, so all three spellings
reach every role and an execution-scoped `ErrorHandler` is built per execution. The `{}` on a unit struct is not decoration: it is what tells a
value from a type name.

**Guards are evaluated before any interceptor is built, and built one at a time.** A refusal at index
0 constructs nothing at index 1. gRPC already evaluates guards before interceptors; that order
reaches the other three, and all four build each guard only when the one before it admits.

**The error chain stays outside the interceptors, on every transport.** An interceptor can fail, so
the chain must be above it or that failure has no claimant, and ADR-0035's one-seam rule forces the
nesting rather than permitting it. An inner layer never observes what its outer layer does; that is
what nesting means, and stamping every answer is by definition an outermost job — which the
pre-dispatch role of ADR-0056 holds.

**A guard answers `Result<(), E>`.** `Ok(())` admits; `Err(e)` refuses with a typed reason,
`E: Into<T::Error>`, so `Err(MyAuthError::TokenExpired)` reaches `#[catch(MyAuthError)]` exactly as a
handler's error does, and the framework still attaches the guard's index.

**One type parameter on all three roles.** `Guard<Http>`, `Interceptor<Http>`, `ErrorHandler<Http>`,
`Next<Http>`. `Transport` is public and sealed: public so it can be named, sealed because a user's
own marker would get no adapter and no dispatch. A cross-transport guard is
`impl<T: Transport> Guard<T>`.

**One shape for the rest of the surface.** A concrete `Next<T>` with `InterceptorNext` made private;
the context first everywhere, so `ErrorHandler::catch(&self, ctx, error)`; `handle_error` becomes
`catch`, aligning the trait with `#[catch]`; one result alias.

**A capability trait for the answer-touching case.** `HasHeaders`, implemented for `HttpResponse` and
`GrpcReply` and nothing else, so an interceptor written over every transport and bounded on it
covers the two where stamping means something and is refused at compile time on the two whose wire
has no header slot.

**One error vocabulary.** `ErrorKind` is it. Each transport's error becomes `Status { kind, message }`
plus `Custom { status, message, source }` plus `AppError`, with today's constructors kept as sugar.
The `source` slot is what lets a hand-named status keep the domain error a `#[catch]` handler
matches on.
A guard's refusal is one event on all four transports and both WebSocket phases.

**The rule under two of these.** *A trait carries what is true of everything that implements it;
anything narrower goes on a narrower trait that only the things which have it implement.* It is why
an `Answer` does not go on `ExecutionContext`, and why a header slot does not go on `Interceptor`.

## Consequences

- A declaration mixing spellings runs in the written order, and the same declaration with the
  spellings permuted runs in the permuted order, and `#[use_guards]`'s documented rule holds.
- A closure in a declaration builds per execution; a value does not.
- A request refused by a guard constructs no interceptor and no later guard.
- `Guard` breaks: every implementation moves from `bool` to `Result<(), E>`. `Interceptor` and
  `ErrorHandler` break: one type parameter, context first, `catch`. The breaks ride together so an
  application migrates once.
- An interceptor never sees the answer an error handler produced. A stamp that must reach every
  answer is written at the pre-dispatch role.
- `GuardRejection.reason` is set by the guard that refused.

## Roads not taken

**Documenting the split.** It leaves a caller needing to know which spelling they used in order to
know which guard runs first, and the ordering is security-shaped.

**Deleting the value form.** It would lose the shared-stateful case a rate limiter needs.

**Moving the chain inside the interceptor stack.** It trades the one-seam property for the stamping
case. **Running it at both ends with the inner one always resolving** would hand every interceptor an
answer and take away the typed error that logging wants. **Widening `Interceptor` so its after-phase
carries both** costs a signature change to buy what the pre-dispatch role already provides.

**`bool` as it stands** for a guard's answer, which is why `GuardRejection.reason` is documented and
never set; and **`Result<bool, E>`**, which keeps two refusal paths that render differently.

**An `Answer` associated type on `ExecutionContext`**, the route that avoided the break. An answer is
a property of a transport, not of an execution, and `StandaloneContext` implements
`ExecutionContext` while having no answer at all.

**Nest's runtime narrowing** for the header-stamping case — `getType()` / `switchToHttp()`, the only
option TypeScript leaves, and one whose `else` branch returns a wrong answer with nothing to report
it, where a bound refuses to compile.
