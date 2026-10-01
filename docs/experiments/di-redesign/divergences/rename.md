# Divergences: the `Site` rename

The choices made beyond the signed-off table of the thirteenth response (`trait Site` →
`FromContainer`, `Construct::sites(s: &mut Sites)` → `Construct::dependencies(d: &mut
Dependencies)`, `Sites::{field, param, site}` → `Dependencies::{field, param, add}`, `SiteDesc` →
`Requirement`). Each entry gives what was renamed, to what, and why.

Files: every file under `crates/ulo/src` and `crates/ulo-macros/src` that named the family, and
`BUILD_PLAN.md`.

## Public items

### 1. The other declaration methods follow `Construct::dependencies`

- **Renamed:** `Factory::sites(s: &mut Sites)` and `ShutdownFactory::sites(s: &mut Sites)` →
  `dependencies(d: &mut Dependencies)`; `Meta::sites(&self, _s: &mut Sites)` →
  `Meta::dependencies(&self, _d: &mut Dependencies)`.
- **Why:** each declares the same thing `Construct::dependencies` declares, for a closure or a
  metadata value. Two names for one act would leave `sites` behind on public traits.

### 2. `ConstructError::Site` → `ConstructError::Dependency`

- **Renamed:** the variant carrying a failed dependency read, its `Debug` name, and every match on
  it (`app/shared.rs`, `lifecycle/connect.rs`, `binding/factory.rs`).
- **Why:** a public variant named `Site` is a public item keeping the word. Its doc already
  described it as "a dependency read failed".
- **Open:** DESIGN.md still spells the variant `Site` in §3.4, §10.2 and §13 (the
  `ConstructError` paragraph, the type listing, the `connect` paragraph and the hand-written
  `Construct` example's comment) as of this rename.

### 3. `FromContainer::describe` takes `req`

- **Renamed:** the parameter `d: &mut SiteDesc` → `req: &mut Requirement`.
- **Why:** `d` is now the `Dependencies` parameter of `Construct::dependencies`. DESIGN §3.2
  writes `req`.

### 4. Diagnostic text

| Where | Old | New |
|---|---|---|
| `FromContainer` `on_unimplemented` message | `` `{Self}` is not an injection site `` | `` `{Self}` cannot be obtained from the container `` |
| `FromContainer` second note | kept verbatim: `or set it inside the #[construct] fn / mark the field #[injectable(default)]` | |
| `AllowedIn` label | `this site needs an execution` | `this injection point needs an execution` (DESIGN §3.3's text) |
| `Factory` message | `` `{Self}` is not a factory over sites `` | `` `{Self}` is not a factory the container can call `` |
| `Factory` label, `ShutdownFactory` label | `injection site(s)` | `injection point(s)` |
| `Factory` note | `annotate each parameter with its site type` | `annotate each parameter with its type` |
| `WiringError::Missing` help | `the site reads` | `the injection point reads` (DESIGN §10.1's sample) |
| macro: factory parameter without a type | `needs its site type written` | `needs its type written` |
| macro: constructor with `self` | `each an injection site` | `each an injection point` |
| macro: constructor parameter pattern | `print as the site's name` | `print as the parameter's name` |
| macro: `#[injectable(default)]` marker | `every other field is a site` | `every other field is injected` |
| macro: `#[injectable]` on another item | `whose fields are sites` | `whose fields are injected` |
| macro: `with = ..` not a closure | `whose parameters are sites` | `whose parameters are injection points` |

The `FromContainer` text is the table's; none of it suggests implementing the trait.

## Internal names

### 5. Module and file renames

- `crates/ulo/src/site/` → `crates/ulo/src/dependency/` (the private `site` module → `dependency`).
- `crates/ulo-macros/src/shared/sites.rs` → `shared/dependencies.rs` (`shared::sites` →
  `shared::dependencies`).

### 6. Core identifiers

| Old | New | Why |
|---|---|---|
| `SiteRead` | `Read` | one read a `Requirement` records |
| `SiteRecord` | `DependencyRecord` | one entry of `Dependencies.list`, beside `BindingRecord` and `HookRecord` |
| `SiteRecord.desc` | `DependencyRecord.requirement` | its type is `Requirement` |
| `SiteLabel` | `DependencyLabel` | |
| `sites` fields of `BindingRecord`, `ReadyRecord`, `HookRecord`, `Override`, `ClosureDecl`, `MetaEntry` | `dependencies` | |
| `Edge.site` | `Edge.dependency` | index into `record.dependencies.list` |
| `Steps.sites` (wiring step 3's errors) | `Steps.dependencies` | |
| `site_label`, `site_text`, `site_step` | `dependency_label`, `dependency_text`, `dependency_step` | |
| `sites_deps` | `closure_deps` | beside `construction_deps` and `readiness_deps`; it serves closures |
| `resolve_sites`, `check_sites`, `clone_sites` | `resolve_dependencies`, `check_dependencies`, `clone_dependencies` | |
| `factory_sites`, `sites_of`, `shutdown_sites`, `meta_sites` | `factory_dependencies`, `dependencies_of`, `shutdown_dependencies`, `meta_dependencies` | |
| `site_failure` | `dependency_failure` | |
| `HookSite`, `site_hooks`, `site_key` | `HookOwner`, `hooks_of`, `owner_key` | not part of the family: the type's doc already read "what owns a hook", and with the family renamed "site" means nothing else in the core |
| `Graph::consumer(.., site: usize)`, `Graph::dependency_step(.., site: usize)` | `index: usize` | the old parameter was shadowed by the record it indexed |

### 7. Macro identifiers

| Old | New |
|---|---|
| `SiteSpec`, `SiteLabel` | `DependencySpec`, `DependencyLabel` |
| `sites_param()`, generated ident `s` | `dependencies_param()`, generated ident `d` |
| `ConstructImpl.sites` | `ConstructImpl.dependencies` |
| `FieldRole::Site` | `FieldRole::Dependency` |
| generated bindings `__ulo_site_{i}` | `__ulo_dep_{i}` |

## `BUILD_PLAN.md`

### 8. `assert_site` replaced by what the spine built

- **Changed:** area G's `__private.rs` row named `assert_site`, which `__private.rs` never
  defined. It now names `field` and `param`, the two functions generated code calls to declare a
  dependency and assert `FromContainer + AllowedIn`.
- **Why:** the row carried the old word, and its name did not exist to rename.

## Kept

- `Span::call_site`, `Span::mixed_site` and "mixed-site" in macro comments: proc-macro hygiene.
- "a factory's call site" in `__private.rs` and "call-site span" in `module_attr`: where a call is
  written.
- `visited` in `graph/order.rs`.
