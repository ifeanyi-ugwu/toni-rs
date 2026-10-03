# Divergences: wave 4, agent W (role-key refusals, `Contribute::try_value`)

Every place W's code departs from the brief, or fills a gap a user would see. Each entry gives
what the brief says, what W wrote, and why. Requests for other agents' files follow the entries.

Files: `binding/contribute.rs`, `graph/{scopes,wire}.rs`, `error/wiring.rs`.

Superseded by this wave: wave 3's W 3 (a qualified contribution under a role key, through
`contribute`, accepted and unreported) and the clause of W 2 that leaves the qualifier uncompared
without consequence. Marking still ignores the qualifier; the refusal is entry 1.

## Entries

### 1. `QualifiedRoleContribution` (item 1)

- **Brief:** at freeze, a contribution whose key's type is in the role-key set and whose qualifier
  is not `()` is a wiring error naming the key, the module and the call location, with the fix
  "contribute it unqualified".
- **W:** a new variant, `WiringError::QualifiedRoleContribution { key, module, at }`, step 2.
  `key` is the qualified collection, `at` the contribution's registration call.
  - Text: "a qualified contribution to the role key `AnyGuard<Http>` in UsersModule is read by no
    transport", items "contributed as `AnyGuard<Http> @ Replica (collection)` at …",
    "help: contribute it unqualified; a transport reads only `AnyGuard<Http>`".
  - Found in `scopes::mark_role_contributions`, after the marking loop.
  - The contribution still takes the enhancer role. As a provider, an `Auto` one needing an
    execution would draw a second entry, a `ScopeViolation`, for the same line.
- **Why a new variant:** no existing variant names a contribution that is well-formed but unread.

### 2. `SingleRoleBinding` (item 2)

- **Brief:** a single binding whose key's type is in the role-key set is a wiring error, covering
  the second key `also_as` writes, an alias and the value API, with the fix "contribute it:
  `into AnyGuard<Http>: [..]`, or `m.contribute::<..>()`".
- **W:** a new variant, `WiringError::SingleRoleBinding { key, module, at }`, step 2.
  - Every key of the record is tested: the primary key (an alias, or an override's split record),
    each `also_as` key, each with the binding's qualifier. One entry per key under a role key.
  - Text: "a single binding under the role key `AnyGuard<Http>` in UsersModule is read by no
    transport", items "bound as `AnyGuard<Http> @ Q` at …", "help: contribute it:
    `into AnyGuard<Http>: [..]`, or `m.contribute::<AnyGuard<Http>>()`". The help names the
    unqualified key, since a qualified contribution is refused by entry 1.
  - `at` is the call that registered the binding, `provide` or `singleton`, not the `.also_as(..)`
    call: `AlsoAs` records no location, and `binding/handle.rs` is not W's (request H1).
  - `KindMix` is no longer reported for a key whose type is a role key. A single binding beside
    contributions under the same role key draws `SingleRoleBinding` alone: `KindMix`'s help, "or
    bind it once", is wrong advice there, and two entries would name one cause.
- **Limit:** the binding keeps the provider role. An `Auto` one reading execution data also draws
  a `ScopeViolation`, which the fix removes with it.
- **Why a new variant:** `KindMix` needs a contribution's location, and a single binding under a
  role key is refused with no contribution present.

### 3. The role key's spelling in both entries

- **Gap:** a role key's type name is `dyn ErasedGuard<Http>`, which every report prints for
  `AnyGuard<Http>`. A help telling the user to write `into dyn ErasedGuard<Http>: [..]` names a
  spelling the user never wrote.
- **W:** `error::wiring::role_spelling` rewrites a text opening with `dyn` and a path whose last
  segment is `ErasedGuard`, `ErasedInterceptor` or `ErasedErrorHandler` into the alias:
  `AnyGuard<Http>`, or `ulo::transport::AnyGuard<ulo_http::Http>` when the report prints full
  paths. The qualifier and the `(collection)` suffix are kept. Only entries 1 and 2 apply it.
- **Limit:** every other entry naming a role key, `Missing` and `Ambiguous` among them, still
  prints `dyn ErasedGuard<Http>`. Applying it there is a change to `Key`'s `Display`, outside W's
  files.

### 4. The limit of both checks, and lazy loads

- Both refusals run in the pass that marks roles, over the set `scopes::role_types` returns: the
  role keys of every mounted handler and the key of every contribution with the enhancer role.
  A role key no mounted handler reads and no `m.enhancer` marks is not in the set, so neither a
  qualified contribution nor a single binding under it is checked. This follows from the rule.
- A lazy load reports a binding it brought, and a base binding only under a role key the load
  itself marked through `m.enhancer`: the base wiring checked the base bindings against every
  other key. Without the second case, a base single binding under a newly marked key would go
  unreported, since `KindMix` is suppressed for it (entry 2).
- `role_types` is shared by the marking pass and `check_bindings`. The set is the same before and
  after `mark_role_contributions` and `assign_roles`: the first marks only contributions already
  under a key in the set, the second only single bindings.

### 5. `Contribute::try_value` (item 3)

- **Brief:** `try_value<E: Into<BoxError>>(self, value: Result<Arc<U>, E>) -> Handle<'m, U,
  Contribution<Singleton, Set>>`, `#[track_caller]`, on the impl shared by both marks; an `Err`
  recorded as `ModuleDef::try_value` records it, the recipe `Recipe::Failed`; an `Ok` as `value`.
- **W:** as specified, on `impl<'m, U, Q, M> Contribute<'m, U, Q, M>`, so both `Plain` and
  `Enhancer` builders have it.
  - The `ValueFailure`'s `key` is `U @ Q`. Nothing reads that field; the report takes the key from
    the record.
  - **Secrets:** `Contribute::value` registers none, and so `try_value` registers none on `Ok`.
    Neither can: `ModuleDef::register_secret` downcasts through `&dyn Any`, which an unsized `U`
    does not coerce to.
- **Wiring report:** `ValueFailed` now names the record's kind, so a failed contribution reads
  "the value for `dyn Plugin (collection)` in PluginsModule failed to build: …". Pairing by order
  holds unchanged: both `try_value`s push the failure and the `Failed` record in one call.
- **`override_many`:** a failed contribution under a key a test's `override_many` replaces is not
  reported, as a failed single binding an override replaces is not. The failures are reported
  before `apply_collections` removes the records, so the skip is by the overridden key.

## Requests for other agents

### M. The macro side

- Wave 3 M 3, the span error on `value = expr?` in an `into` list, can become the lowering to
  `m.contribute::<K>().try_value(..)`. That closes M's R2.
- Wave 3 M 5's limit, `X as HttpGuards` passing unreported, is now `SingleRoleBinding` at
  `wire()`, as is the value API's `also_as::<AnyGuard<Http>>`. That closes M's R1. The `as <role
  key>` compile error still reports earlier where it fires.

### H1 (owner of `binding/handle.rs` and `binding/mod.rs`, optional). The `also_as` location

- **Request:** make `Handle::also_as` `#[track_caller]` and record `Location::caller()` on
  `AlsoAs`, so `SingleRoleBinding::at` can name the `.also_as(..)` call rather than the
  registration call.

### Design fold

- State the two refusals beside the `TypeId` rule (§3, §7): a qualified contribution and a single
  binding under a role key are wiring errors, found where roles are marked, with entry 4's limit.
- `Contribute::try_value` joins the value API, and wave 3's W 3 is closed.
