# 0039 — A gRPC status carries the error it came from

Status: proposed

Supersedes the mechanism of [ADR-0037](0037-a-grpc-handler-hands-the-chain-its-error.md).

## Context

A gRPC handler answers with a domain error and the generated method renders it. Two things are
lost on the way out.

The **type** is lost because `tonic::Status` is what the trait returns, and a status is a code and
a string. ADR-0037 kept it by parking the error on the execution: the generated method wrote it
into the context's extension bag, and the wrapper took it out before running the error chain. That
works while a context exists. It also puts a value in a bag that outlives the call it belongs to,
and reads it back by a rule — the next failure in this execution is mine — rather than by holding
it.

The **code** is lost when a handler wants one no `ErrorKind` reaches. `grpc_code` maps eleven kinds
onto the canonical codes; `FailedPrecondition`, `OutOfRange`, `AlreadyExists`, `DataLoss` and
`Cancelled` are not among them. Answering with one meant registering a chain handler to claim the
error and return the status — a seam away from the handler, for a fact the handler knows.

## Decision

**A `GrpcStatus` carries the error it was built from.** `GrpcStatus::of(error)` maps the kind to a
code as before and keeps the value:

```rust
pub struct GrpcStatus {
    pub code: GrpcCode,
    pub message: String,
    source: Option<Arc<dyn toni::Error>>,
}
```

**The status travels whole, through `tonic::Status`'s own source slot.** `Status::set_source` takes
an `Arc<dyn std::error::Error + Send + Sync>`, so the domain error rides out of the generated
method attached to the answer and the wrapper reads it back before running the chain. The slot is
typed `dyn std::error::Error`, which drops the `Send + Sync` the chain needs, so the error is
wrapped in a concrete `GrpcFailure` that a downcast recovers the bound from.

The execution's extension bag is no longer involved: `stash_failure` and `take_failure` are gone.
A call dispatched without a toni context now keeps its error type too, which the parked form could
not do.

**`GrpcStatus` implements `toni::Error`,** so a handler can return one and name any code:

```rust
#[grpc_method]
async fn reserve(&self, Payload(req): Payload<ReserveRequest>)
    -> Result<ReserveReply, GrpcStatus>
{
    if !self.window_open() {
        return Err(GrpcStatus::new(GrpcCode::FailedPrecondition, "the booking window is closed"));
    }
    ...
}
```

`GrpcStatus::of` answers a status as itself rather than re-deriving one, so the code a handler
names is the code on the wire. Its `kind()` is the inverse of `grpc_code` where the table has an
answer and `Internal` where it does not — that path is reached only when a `GrpcStatus` is rendered
somewhere other than gRPC.

**The blanket `impl<E: toni::Error> From<E> for GrpcStatus` is removed.** It cannot coexist with
`impl toni::Error for GrpcStatus`: the two together would give `From<GrpcStatus> for GrpcStatus`,
which collides with the reflexive impl in the standard library. `GrpcStatus::of` is the owned
conversion and `GrpcStatus::from_error` the borrowed one.

## Consequences

- A handler that wants a code outside the kind table names it inline. A chain handler is still the
  place to reshape an error raised somewhere else — a guard rejection, a panic.
- A handler answering `Result<_, GrpcStatus>` gives the chain a `GrpcStatus`, so a
  `#[catch(MyError)]` handler does not match it. Carrying the domain error through
  `GrpcStatus::of(my_error)` keeps both: the code the kind maps to, and the type for the chain.
- The error reaches the chain whether or not the call came through toni's dispatch, because it
  travels on the answer rather than beside it.
- `to_status` keeps its meaning for a service registered through `GrpcAdapter::add_service`: it
  renders the error and attaches it, so a status leaving such a service carries its cause even
  though nothing downstream reads one.
- One value describes a gRPC failure end to end. The wrapper builds a `GrpcStatus` from the
  `tonic::Status` it caught either way, and the difference between "the handler named this code"
  and "a kind mapped to it" stops being visible to anything downstream.

## Roads not taken

**Keeping the parked form as a fallback.** Two mechanisms for one fact, and the fallback is reached
only where the primary already works.

**Putting the domain error in `tonic::Status`'s details or metadata.** Both are wire fields —
serialized, sent, and read by a caller that has never heard of the type. The source slot is
in-process, which is the whole lifetime the chain needs.

**Making `GrpcStatus::kind()` fail for a code no kind reaches.** `kind` is not fallible on any other
implementation of `toni::Error`, and the alternative to `Internal` is a panic in a rendering path.
