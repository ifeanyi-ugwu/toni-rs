# Divergences: wave 3, agent W (the contribution marker, roles by `TypeId`, replacements, module names)

Every place W's code departs from `DESIGN.md` or the fourteenth response, or fills a gap a user
would see. Each entry gives what the spec says, what W wrote, and why. Requests for other agents'
files follow the entries.

Files: `binding/contribute.rs`, `module/{mod,def}.rs`, `graph/{mod,scopes,wire}.rs`,
`error/wiring.rs`, `testing.rs` (a doc comment), `transport/mod.rs` (doc comments).

Superseded by this wave: wave 2's W 3 (a qualified enhancer contribution accepted and unreported),
W 2's "a contribution through `contribute` under a role key stays a provider", W 5's unreported
second replacement, W 7's `at` on the `.ready(..)` call, and W 9's limit that `ModuleName`s print
short.

## Entries

### 1. The marks on `Contribute` (D22)

- **Spec:** `Contribute<'m, U, Q, Mark = Plain>`, public zero-sized markers `Plain` and
  `Enhancer`, sealed or with private fields; `qualified` only for `Plain`; every other method
  shared through `impl<.., M> Contribute<.., M>`.
- **W:**
  - `pub enum Plain {}` and `pub enum Enhancer {}`, uninhabited, the device `Open`, `Set`,
    `Pending` and `Wired` already use. Neither sealing nor a private field is needed: no value of
    either type can be built at all.
  - `Contribute` keeps a private `enhancer: bool`, written by the constructor of each mark, so
    the shared `impl<'m, U, Q, M>` carries no bound on `M`. Code generic over the mark compiles
    without naming a sealed trait it cannot see.
  - `qualified` sits in `impl<'m, U, Q> Contribute<'m, U, Q, Plain>` and returns a `Plain`
    builder. `ModuleDef::enhancer` returns `Contribute<'_, R, (), Enhancer>`, so
    `m.enhancer::<AnyGuard<Http>>().qualified::<Q>()` is E0599 at `.qualified`.
  - No wiring-time handling of a qualified enhancer contribution existed to remove: wave 2's
    entry 3 recorded it as accepted and unreported.
- **Why:** the marker is the only thing that differs between the two builders, and an empty enum
  states that no value is meant to exist.

### 2. Roles by `TypeId` at freeze (D23)

- **Spec:** at freeze, collect the role keys' `TypeId`s from every mounted handler plus the keys
  of every `m.enhancer::<R>()` registration; any contribution whose key's `TypeId` is in the set
  gets the enhancer role, however it was registered.
- **W:** `scopes::mark_role_contributions`, called at the end of `freeze` once every controller
  has mounted its handlers.
  - The set is the `role_keys` of every handler in the graph, a lazy load's base included, plus
    the primary key of every collection binding freezing already marked `Enhancer` from
    `ModuleNode::enhancers`.
  - Every collection binding still a provider whose primary key's `TypeId` is in the set becomes
    an enhancer. Single bindings are not touched: a single binding under a role key is a
    `KindMix`.
  - The qualifier is not compared, following "the key's `TypeId`" literally. Entry 3 has the
    consequence.
  - `ModuleNode::enhancers`, and `remove_contributions` keeping it aligned, stay: they carry the
    explicit mark.
- **Why:** the handlers are mounted inside `freeze`, so the set is complete only after the last
  module is appended; `assign_roles`, which needs the visibility tables, still runs in `check`.

### 3. A qualified contribution under a role key, through `contribute` (open)

- **Spec:** D22 refused a qualified enhancer contribution at compile time on the `Enhancer`
  builder. With D23, `contribute::<AnyGuard<Http>>()` registers what `enhancer` does.
- **W:** `m.contribute::<AnyGuard<Http>>().qualified::<Q>()` compiles, registers under
  `AnyGuard<Http> @ Q`, takes the enhancer role by entry 2's rule, and is read by no transport
  and reported by nothing: D22's fault through the other builder. A negative bound cannot refuse
  it on `Plain`.
- **Why:** open. The brief removed wiring-time handling of qualified enhancers, and refusing this
  case needs one. Options: (a) refuse at `wire()` a qualified contribution whose key's `TypeId` is
  in entry 2's set; (b) accept and document. W built (b) without the documentation and leaves the
  choice to the user.

### 4. `DuplicateReplacement` (D25)

- **Spec:** a second `replace_module` of an original already replaced is reported, naming both
  calls, as `DuplicateReadiness` does.
- **W:** a new variant, `WiringError::DuplicateReplacement { original: ModuleName, first, second }`,
  step 2, `first` and `second` the two `replace_module` locations. Text: "two `replace_module`
  calls replace {original}", items "first `replace_module` at …", "second `replace_module` at …",
  "help: keep one; wiring applied only the first".
  - Detected from the test plan's replacements in order, independent of the registration walk.
  - Over an original nothing imports, the first call is `ReplacementUnmatched` and each later one
    `DuplicateReplacement`, never both on one call.
- **Why:** no existing variant fits. `DuplicateReadiness` and `DuplicateBinding` name a key and the
  module holding it; a replacement names a module identity and has neither.

### 5. `BackoffWithoutTimer::at` (D26)

- **W:** `at` is `ReadyRecord::backoff_location`, falling back to the `.ready(..)` location when it
  is `None`. The fallback is unreachable while the check fires only on a non-zero backoff, which
  only `.backoff(..)` writes.

### 6. Full paths on collision for module names

- **Spec:** `{:#}` on `ModuleName` prints the full type path, `{}` stays short; the report's
  grouping pass extends to `ModuleName` fields; rendered strings stay short.
- **W:**
  - `ModuleName` holds both texts. `{:#}` writes the module type's and the qualifier's full
    paths, with the `#n` suffix: `my_app::db::DbModule @ my_app::Replica #2`.
  - A labelled module keeps its label in both forms, and `{:#}` follows it with the type's full
    path: `redis (my_app::Redis) @ my_app::Primary`. A label is not a path; printing the path
    beside it is what separates two module types sharing a label.
  - `Display` now writes through `Formatter::pad`, as `Key`'s does, so width and alignment flags
    apply. `Debug` passes the flag along, so `{:#?}` writes full paths. Equality and hashing cover
    both texts, which differ only between different modules.
  - `module::colliding_names` (crate-visible) groups names by short text and returns those in a
    group of two or more distinct names. `WiringErrors`' `Display` runs it over every `ModuleName`
    field of every entry, separately from keys, and a `WiringError` displayed alone over its own.
  - The `Ambiguous` entry's column of exporters is padded to the widest name as printed, full or
    short.
  - **Limits:** `OverrideModuleAmbiguous::module` is a type name held as `&'static str`, not a
    `ModuleName`, and stays short; its candidates share that type and so never collide by type.
    `consumer`, path steps, `item`, `handler` and `closure` stay short. Errors outside the wiring
    report, `LookupError::Ambiguous` among them, are R's (request R1).
- **Why:** the fourteenth response's "Module names still collide".

### 7. Doc comments

- `Contribute`, `ModuleDef::contribute`, `ModuleDef::enhancer`, `Role`, the `transport` module,
  `graph::Role`, `assign_roles` and `freeze` state the `TypeId` rule. `enhancer`'s doc names its
  remaining job: marking a role key no mounted handler reads, where `contribute` would leave an
  `Auto` contribution needing an execution refused as a singleton.
- `TestApp::replace_module` names `DuplicateReplacement`, and `BackoffWithoutTimer` names the
  `.backoff(..)` call as `at`.

## Requests for other agents

### R1 (R). Full paths in `LookupError::Ambiguous`

- **File, item:** `error/mod.rs`, `write_list`.
- **Request:** print each source with `{:#}` when `crate::module::colliding_names(sources)` holds
  it, so `billing::Module` and `users::Module` no longer read "Module, Module". `AmbiguousModule`'s
  candidates share one type and need nothing.

### L1 (whoever owns `lib.rs`). Export the marks

- **File, item:** `lib.rs`.
- **Request:** re-export `binding::contribute::{Plain, Enhancer}` so a user can name
  `Contribute<'_, AnyGuard<Http>, (), Enhancer>`. W suggests `ulo::handle`, beside the other
  typestate markers, with that module's doc widened to cover `Contribute`'s mark; a bare
  `ulo::Enhancer` at the root reads like a trait. Without the export the code compiles, and the
  type is unnameable outside the crate.

### Notes for M and the design fold

- **M:** with entry 2, `into K: [..]` may lower to `contribute` for every `K`, as the fourteenth
  response says. A role key no mounted handler reads then stays a provider, which only
  `enhancer` avoids.
- **Design:** §3's two limits of token-level recognition and the note that `contribute` under a
  role key is a provider are obsolete; the second-section entry reads "the same builder, marked".
  Entry 3 is a decision for the user.
