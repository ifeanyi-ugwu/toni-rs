# Divergences: area C (modules, the graph and the wiring pass)

Every place area C's code departs from `DESIGN.md`, or fills a gap the design leaves that a user
would see. Each entry gives what the design says, what C wrote, and why. All await the user's
sign-off.

Files: `module/{mod,def,dynamic,keyed,meta}.rs`, `graph/{mod,register,wire,visibility,cycles,scopes,order}.rs`.

## Visibility

### 1. A module's own binding and an imported export of one key are two sources

- **Design:** §8.2 lists what a module sees; §12 refuses "two sources for one key" and, for a
  module binding `dyn Timer`, treats its own binding beside the core's global export as two
  sources. It does not say whether an own binding shadows an import's export.
- **C:** no shadowing. A key a module binds and also sees through an import or a global is
  `Visible::Ambiguous`. One binding reached through two routes, an import's export and another
  import's re-export of it, is one source.
- **Why:** the `dyn Timer` row requires it, and a shadowing rule would make the row unreachable.

### 2. The root's table is checked whole

- **Design:** §8.2 states the root's table is free of ambiguity once wiring passes; §10.1 step 3
  reports ambiguity only for a site that reads the key.
- **C:** after resolving sites, every `Ambiguous` entry of the root's table not already reported
  is reported, read or not, with the consumer "a lookup that names no module". Other modules'
  tables report only what a site reads.
- **Why:** `app.get` and `exec.get` resolve with the root's visibility and have no variant for an
  ambiguity; without the sweep the design's claim is false for a key no site reads.

### 3. `export::<T>()` of a key the module does not bind

- **Design:** silent; §8.1 distinguishes `exports = [T, reexport U]`.
- **C:** step 1 reports `WiringError::Missing` with the consumer "its export list", or "its export
  list (a key an import provides leaves through `reexport`)" when the module sees the key through
  an import. A plain export never re-exports.
- **Why:** no variant names an unbound export; see request E1.

### 4. Execution inputs are in every module's table

- **Design:** "Inputs are app-wide and belong to transports" (§6.4, §8.3).
- **C:** every declared input is `Visible::Input` in every module's table, below a binding under
  the same key. An input declared twice, or an input key that any module also binds as a single,
  is `DuplicateBinding` naming both locations.
- **Why:** app-wide visibility follows from the design; the conflict needs a report, and
  `DuplicateBinding` is the variant for two sources declared at registration.

### 5. A single/collection mix is app-wide

- **Design:** §4 "[10]": "a mix of `provide` and `contribute` under one key" is a wiring error; it
  does not say whether both must be in one module.
- **C:** a single binding in any module under a key that has contributions in any module is
  `KindMix`, reported once per key, naming the single binding's module.
- **Why:** collections are app-wide (§8.2), so the mix exists wherever the two halves are.

## Roles and scopes

### 6. Role keys for a transport with no handlers (BUILD_PLAN point C)

- **Design:** silent; contributions under a role key are enhancers for the `Auto` rule (§3.3, §7).
- **C:** a contribution is an enhancer when its key is a role key a mounted handler names, or any
  key of the three families `AnyGuard<T>`, `AnyInterceptor<T>`, `AnyErrorHandler<T>`, whether or
  not `T` has a handler. The families are recognised by the `type_name` prefix of each role trait
  object, read at run time from a private probe transport in the same build.
- **Why:** treated as a provider, a global enhancer reading execution data on a transport the app
  mounts nothing on would be refused as an `Auto` provider. The prefix is the one place the core
  matches a type name; `type_name`'s format is not guaranteed across compilers, and a format
  change would demote such contributions to providers rather than break anything else.

### 7. Closure sites outside a binding's own are checked for keys alone

- **Design:** §10.1 step 3 resolves "every site"; §6.2 runs the scope rules over bindings.
- **C:** hook closures, readiness checks, module hooks, enhancer closures and metadata sites are
  checked for missing and ambiguous keys. They are not scope-checked: a hook reading `Ext<T>`
  fails at `connect` with `ExecutionRequired`. What a readiness check reads, other than its own
  binding, orders the connect walk before the binding and counts as an edge in the cycle check.
- **Why:** §6.2's table names bindings, and the runtime error already names the key a hook could
  not read. A check runs right after its binding is built and before its readers (§9.3): what it
  reads has to exist first, and a cycle through a check cannot be satisfied.

### 8. A readiness check counts as a hook on an `Auto` binding inferred per-execution

- **Design:** §6.2 refuses "hooks on types that became per-execution".
- **C:** `HooksOnPerExecution` also fires for a `.ready(..)` on such a binding.
- **Why:** `connect` runs readiness checks, and a binding built per call never reaches it (spine
  entry 3 already lists `.ready` with the closure hooks).

### 9. A refused binding does not cascade

- **Design:** §10.1 step 5 reports a violation "with the path that introduces the execution
  dependency".
- **C:** an explicit singleton or an `Auto` provider that needs an execution is refused and stays
  a singleton for its readers, which do not inherit the need. One violation yields one error.
- **Why:** reporting every binding above the one that introduces the dependency would repeat one
  fault up the graph.

### 10. What a handler reaches for the input check

- **Design:** §6.4: "walks each handler's reachable execution-scoped bindings".
- **C:** the walk starts at the controller, the global contributions under the handler's own
  role keys, the bindings its by-type enhancers name, and what its enhancer closures read; it
  enters execution-scoped and transient bindings only. An input an enhancer closure reads
  directly is reported with the path `handler (Transport) → enhancer closure → <site>`. Each input
  is reported once per handler and reading binding.
- **Why:** a singleton is built at `connect` with no execution; one reading an input has already
  failed the scope check.

### 11. One cycle per strongly connected component

- **Design:** §10.1 step 4: "run a DFS over the resolved edges and print the full path".
- **C:** Tarjan's algorithm; for each component with a cycle, the shortest cycle through its
  smallest binding is printed.
- **Why:** listing every elementary cycle is exponential in the worst case; one per tangle names
  where to cut it.

## Tests

### 12. What an override replaces

- **Design:** §11: "An override replaces the recipe of an existing key and keeps its origin
  module, its visibility and its exports."
- **C:** an override replaces the recipe and the sites (D's `Override::sites`), and keeps the
  scope, the `also_as` keys, the closure and trait hooks and the readiness check. An override of
  an `also_as` key moves that key off the binding into a binding of its own in the same module,
  with the replaced binding's scope. Overrides match keys as written inside a module, so
  `override_value::<PgPool>(..).in_module_keyed::<DbModule, Replica>()` matches the unqualified
  binding inside the keyed module. A `try_value` failure whose binding an override replaces is not
  reported.
- **Why:** the override's value is not the type the original builds, so an `also_as` key cannot
  keep the original's recipe; the hooks are the binding's, not the recipe's.

### 13. `override_many`

- **Design:** §11: "`override_many` replaces an entire collection".
- **C:** every contribution to the key, from every module, is removed, and the items are
  contributed as values from the root module, in the order given. With no contribution to the key
  it is `OverrideUnmatched`, or `OverrideKind` when the key is bound as a single.
- **Why:** the design states the replacement and not where the new entries live; the root keeps
  them last in collection order and visible to no lookup but `Many`.

### 14. `replace_module`

- **Design:** §11: "swaps by identity. The replacement must export a superset of the original's
  keys, or wiring reports what's missing."
- **C:** where the walk reaches the original's identity it registers the replacement. The
  original's `register` runs once, into a node that is then dropped, to learn its exports (keyed
  requalification applied). A replacement whose original is never imported is not reported.
- **Why:** no variant carries an unmatched replacement; see request E2.

## Modules

### 15. `module::<M>()` with no qualifier matches every instance

- **Design:** §8.5: `app.module::<UsersModule>()` answers `AmbiguousModule` "if several configs".
- **C:** `Graph::find_module(ty, name, None)` matches every module whose identity names `ty`,
  keyed or not; `Some(Q)` matches the instances keyed by `Q`. A type imported bare and keyed is
  ambiguous without a qualifier. None found is `NotFound { kind: Module }` naming the type, and the
  qualifier when one was given.
- **Why:** D's R6 left it to C; it matches `in_module::<M>()`, which §11 makes ambiguous over
  configured or keyed instances.

### 16. Module names

- **Design:** §3.6: "`DbModule @ Replica`, or `DbModule #2` when unlabeled".
- **C:** `#n` is written for the second and later module of one type and qualifier in collection
  order, labelled or not. The core's `Timer` module is named `AppBuilder::timer`; its binding's
  source location is inside the core.
- **Why:** §13's integration labels every configuration of `DbModule` with the same label, and two
  modules printed alike cannot be told apart.

### 17. The core's `Timer` module comes first in collection order

- **Design:** §3.9: the `Timer` is "a value in its own global module"; its position is not given.
- **C:** it takes `ModuleId(0)`, before the root's subtree.
- **Why:** it imports nothing and every module sees it; first keeps it out of the tie-break among
  user modules.

## Lazy loading

### 18. Which contributions a lazy module may make

- **Design:** §8.6: refused for "a collection the module does not introduce itself", and "any key
  that a pre-existing binding reads as `Many<T>`".
- **C:** refused when the key already has contributions, or anything in the graph reads it as a
  collection: a binding's site, a readiness or hook closure, a module hook, an enhancer closure, or
  a mounted handler's role key.
- **Why:** the pipeline reads a role collection through `entries` without being a binding, and
  closures read collections the same way bindings do.

### 19. One refusal, in a fixed order

- **Design:** `LoadError::Refused(LoadRefusal)` holds one refusal; the order is not given.
- **C:** the modules the load brings are checked in collection order, each for controllers,
  metadata, inputs, a global export, then contributions; the first found is returned. A global
  module with no exports is accepted.
- **Why:** the type holds one; a global module exporting nothing exports nothing globally.

## Requests for other areas

### E1. Optional: a variant for an unbound export

- **File, item:** `error/wiring.rs`, `WiringError`.
- **Request:** `ExportNotBound { module: ModuleName, key: KeyName, imported: bool, at: &'static
  Location<'static> }`, so entry 3 stops passing a hint through `Missing::consumer`; its help would
  read "bind `{key}` in {module}" or, when `imported`, "re-export it with `reexport`". C switches to
  it in `check_modules` once it exists.

### E2. Optional: a variant for an unmatched replacement

- **File, item:** `error/wiring.rs`, `WiringError`.
- **Request:** `ReplacementUnmatched { original: ModuleName, at: &'static Location<'static> }`,
  step 2, for a `replace_module` whose original no module imports; C would report it from
  `Registry::replaced` against the test plan. The design reports an unmatched override as a stale
  mock (§11), and the same reasoning covers a replacement.

## Requests made of C

- **A, R5:** honoured. `also_as` keys and requalified exports map to the original binding's
  `Visible::Binding`; `Graph::collection` answers an empty slice; an alias keeps
  `Recipe::Alias { target }`, looked up in its origin module's table.
- **B, R1:** honoured. Step 2 reports `DuplicateReadiness` from `replaced_ready`, `second` being
  its second entry or `ready`'s location.
- **B, R2:** honoured. `provide`, `provide_with`, `try_provide_with` and `controller` set
  `hooks = erase_trait_hooks::<T>(location)` and `constructs = true`; `provide` and `controller`
  also set `construct_bound = T::CONSTRUCT_TIMEOUT`. An alias is transient, takes its edge from
  its target in steps 3 to 5, and is not in `connect_order`.
- **D, R1:** honoured. An override's `sites` replace the replaced binding's.
- **D, R5:** honoured. Each metadata value is frozen with `Arc::from(box)`.
- **D, R6:** honoured. Base ids are kept; the new modules are `base.modules.len()..`, in
  collection order; `singletons` lists the new singletons in connect order. `find_module`'s `None`
  is entry 15.
- **E, R2:** honoured. `connect_order` of the extended graph is recomputed whole; the base order
  is its prefix and the lazily loaded singletons follow. Each load's modules share one `loaded`
  number, one more than the previous load's.
- **E, R3:** honoured. `ScopeViolation::path` starts at the binding; `InputNotSeeded::path`
  starts with `<handler> (<transport>)`; cycle paths end on their first step; `item` is a phrase
  such as ``readiness `.attempt_timeout` of `PgPool` ``; `knob` is passed as given.
- **F, R2:** honoured. By-type enhancer keys resolve against the controller module's table, a
  miss reported as `Missing` naming the handler, the binding marked `Role::Enhancer`; closure
  sites are checked for keys and take part in the input check. `HandlerRecord::module` is the
  controller's origin, and an ambiguous controller key in its own module is reported.

## Strings C writes into report fields (BUILD_PLAN point C)

E's `Display` owns the hint text. These are the formats C passes it:

- `Missing::consumer` and `Ambiguous::consumer`: ``UserService (param `mailer`)``, ``UserService
  (field `repo`)``, `PgPool factory (param #1)` for a factory binding, `readiness check of PgPool
  (param #1)`, `OnModuleDestroy hook of PgPool (param #1)`, `OnModuleInit hook of UsersModule
  (param #1)` for a module hook, ``metadata `Middleware` of UsersModule (param #1)``,
  ``UsersController::get (enhancer `AuthGuard`)``, `UsersController::get (enhancer closure)
  (param #1)`, `UsersController::get (its controller)`, ``the alias `PgPool @ ReadOnly` ``, and
  `a lookup that names no module`.
- Path steps: a binding as its built type's short name with its qualifier, followed by
  `(execution)` or `(transient)`, space-separated, when not a singleton; the last step is the site,
  ``Dep<RequestHead> (field `head`)``.
- `BoundWithoutTimer::item`: ``construction of `PgPool` ``, ``readiness `.timeout` of `PgPool` ``,
  ``readiness `.attempt_timeout` of `PgPool` ``, ``` `OnModuleDestroy` hook of `PgPool` ```, and
  ``` `OnModuleInit` hook of module UsersModule ```.
