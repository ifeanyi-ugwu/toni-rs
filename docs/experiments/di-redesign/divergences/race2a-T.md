# Divergences: race 2a, area T (`ulo-transport`, `ulo-transport-macros`)

Every place area T's bodies depart from `transports/DESIGN.md` or from the spine, fill a gap the
designs leave open, or add a public item. Each entry gives what the design or spine says, what was
written, and why. All await the user's sign-off. Entries 1–5 change behaviour a user can observe;
6–16 settle what the designs leave open, the four points the build plan hands T included.

## Behaviour

### 1. An extraction or reply-conversion failure reaches the error handlers as a `CallError`

- **Design:** §2.2: `ExtractError` "reaches the error handlers as a `CallError` of that kind
  through the blanket `From`, holding the `ExtractError` as its source"; an error handler reshapes
  it with `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()`.
- **Spine:** `__private::Param::extract` documented "the failure boxed as an `ExtractError`" and
  boxed it bare.
- **Written:** `Param::extract` boxes `CallError::from(extract_error)`. The reply probe does the same
  for an `IntoReplyError` from the value side: a `CallError` of kind `Internal` holding it.
- **Why:** the design's stated shape. H's `render_error` finds the variant for 413 and 415 through
  `source_as::<ExtractError>()` on what `from_boxed` returns, which works for either boxing.

### 2. `from_boxed` maps a constructor's error by the whole table

- **Design:** `LookupError::Construct { reason: Errored(r) }` "recurses into
  `r.downcast_ref::<CallError>()`".
- **Written:** `r` is mapped by every row of the table: a `CallError`, an `ExtractError`,
  `GuardRejected`, `PanicRecovered`, a nested `LookupError`, `Closed`, another `Redacted`.
- **Why:** a constructor that returns a dependency's `LookupError` as its own `Err` would otherwise
  turn that dependency's 404 into a 500. The design's case is one row of this.

### 3. The messages `from_boxed` writes in place of a withheld one

- **Design:** "`Internal`, with a generic message" and "with the message withheld"; silent on the
  text, and on the message for `Forbidden` and `Unavailable`.
- **Written:** `"internal error"` for `Internal` (the text `IntoReplyError` already uses);
  `"forbidden"` for `GuardRejected`, whose `Display` names the guard's type; `"the application is
  shutting down"` for `Closed` and `LookupError::Closed`, whose `Display` names a key.

### 4. `ExtractError::Dependency` withholds the lookup error's text

- **Design:** `ExtractError` classifies `BadRequest` or `Unprocessable`; the spine's
  `Dependency` "classifies as `CallError::from_boxed` maps its lookup error". `Classify`'s default
  `public_message` is the error's `Display`.
- **Written:** `ExtractError`'s `Classify` impl also overrides `public_message` and `details`: a
  `Dependency` takes the message and details its lookup error maps to, so a lookup failure's text,
  which names keys, never reaches the caller. Every other variant keeps its `Display` as the
  message, which names the parameter and, for `Malformed`, carries the redacted decoder text.
- **Gap:** an execution-scoped constructor answering `CallError::unauthorized(challenge)` keeps the
  challenge through `from_boxed` and `Param::extract` (a crate-private `CallError::from_extract`),
  but loses it through a user's own `?` on the blanket `From`, since `Classify` has no method to
  carry one.

### 5. `length` counts characters for text

- **Spine:** `Rule::Length` "checked against `.len()`"; the derive's docs say "on anything with a
  `len()`".
- **Written:** text, anything `AsRef<str>`, is counted in characters; a collection whose borrowing
  iterator is an `ExactSizeIterator` is counted in items. The derive picks by autoref through
  `__private::validate::LengthProbe` (entry 13).
- **Why:** a byte count makes `length(max = 64)` refuse a 40-character name in a non-Latin script.
  The `validator` crate counts characters too.
- **Consequence:** a user type with an inherent `len()` and no borrowing `ExactSizeIterator` does
  not compile under `length`.

## Points the build plan leaves to T

### 6. `Valid<P>`'s `param` text

- **Written:** `P`'s outermost type, its last path segment without generic arguments, sliced from
  `std::any::type_name::<P>()` so it stays `&'static str` without allocating: `"Json"` for
  `Valid<Json<NewUser>>`. A tuple, a reference or any other name that is not a path is kept whole.
- **Why:** the full type name would write crate paths into a client-visible `detail`
  (`"`ulo_http::extract::Json<app::NewUser>` failed 2 validation rules"`). The violations name the
  fields that failed.

### 7. The `validator` bridge offers `violations` alone

- **Written:** `validator_bridge::violations(&ValidationErrors) -> Vec<FieldViolation>`, with the
  three-line `Validate` impl a user writes shown in its docs. Nested paths are joined with `.`,
  list items written `items[0]`, the result sorted by field. A description is the rule's message
  when set, otherwise ``failed the `<code>` rule (k = v, ..)``; the `value` parameter, which
  `validator` fills with the rejected input, is left out, since it may be a secret.
- **Not written:** `impl<T: validator::Validate> Validate for T`. A blanket impl behind a feature
  would change coherence for every downstream crate when the feature turns on, and features must be
  additive.

### 8. The `email` rule's grammar

- **Written:** exactly one `@`; a local part of 1 to 64 bytes with no whitespace or control
  character; a domain of two or more `.`-separated labels, each non-empty, of letters, digits and
  `-`, neither starting nor ending with `-`; 254 bytes in all (RFC 5321's path limit). Letters
  include non-ASCII ones, so an internationalized domain passes.

### 9. Where `from_boxed` looks inside a `Redacted`

- **Written:** a `Redacted` that is the error itself, or the `Errored` reason inside
  `LookupError::Construct`, is looked into by type for every row of the table. A `Redacted` in
  another error's field (`Malformed`'s source, `PanicRecovered`'s message, `FailureReason::Panicked`)
  is not consulted. No `source()` chain is walked, so an outside error wrapping a `CallError` is
  `Internal`. A bare `CallError` wins over a panic found deeper; after it and `ExtractError`,
  `ulo::is_panic` is tested before any other row.

## Shapes the designs leave open

### 10. `#[derive(Validate)]` reports the field as the request names it

- **Design:** silent; the spine's `FieldViolation::field` is "the field's path, as the request
  names it".
- **Written:** the derive reads serde's `rename` (or `rename(deserialize = ..)`) on the field and
  `rename_all` (or `rename_all(deserialize = ..)`) on the struct, applying serde's case rules, and
  strips `r#` from a raw identifier. Serde attributes are read leniently: a form the derive does
  not follow counts as absent, and serde reports it. `flatten` and nested structs are not followed.

### 11. `#[derive(Validate)]`'s grammar beyond the three rules

- `length` and `range` with neither bound, a bound written twice, `email` with arguments, and an
  unknown rule are span errors. Several rules may share one attribute.
- A bound is evaluated once and written into the description through `Display`, so a `range` bound
  on a type without `Display` does not compile: ``length must be between 1 and 64``, ``must be at
  least 13``, ``must be an email address``.
- `range` is written as `!(value >= min)`, so a float's NaN fails both bounds.
- An enum, a union, or a tuple or unit struct is refused, spanned on the type's name.

### 12. `#[derive(Classify)]`'s errors

- A second `#[classify(..)]` on one item, an unknown kind word (the message lists the eleven), a
  union, and a struct with no kind are span errors. Every variant left with no kind is reported in
  one compile, each spanned on its name.

### 13. `__private::validate` is new

- **Spine:** no runtime support for the derive.
- **Written:** doc-hidden `LengthProbe`, `ViaChars`, `ViaItems` (entry 5) and `is_email` (entry 8)
  in `ulo_transport::__private::validate`, which `#[derive(Validate)]` names.
- **Why:** the alternative, emitting the probe and the email check inside every derived impl,
  repeats them per type. Logged because rule 5 counts any added public item.

### 14. `Details`' JSON members

- **Spine:** an array of objects tagged by `"type"`, `retry_after` in whole seconds.
- **Written:** `{"type": "field_violations", "violations": [{"field", "description"}]}`,
  `{"type": "error_info", "reason", "domain", "metadata"}`, `{"type": "retry_after", "seconds"}`,
  `{"type": "help", "links": [{"description", "url"}]}`, `{"type": "json", "value"}`. A fractional
  second rounds up, so a client honouring it never retries early.

### 15. `span::call`

- **Written:** an `INFO` span with the static name `"call"`; `otel.name`, `ulo.transport` recorded;
  `ulo.handler` recorded only when `Some`, otherwise declared for the transport to record; every
  other field declared `Empty`.

### 16. A reply probe converted twice

- **Written:** the probe's value sits in a `Cell<Option<_>>`; a second `into_reply` on one probe
  answers a `CallError` of kind `Internal` rather than panicking. Generated code converts each
  probe once, so this path is unreachable from it.

## Requests to other areas

- **H (`render.rs`):** `Retry-After` from a `RetryAfter` detail or `shed_retry_after` should round a
  fractional second up, as entry 14 does, so the header and the `details` member agree. Advisory;
  the header is H's.
