use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::panic::Location;

use crate::key::{BindingKind, Key, KeyName, role_spelling, short_type_name};
use crate::module::{ModuleName, colliding_names};
use crate::redact::Redacted;
use crate::scope::ScopeKind;

/// Every failure `wire()` found, one entry each, in the order the six steps of §10.1 found
/// them. `Display` writes the full report:
///
/// ```text
/// error: wiring failed with 2 errors
///
///   × missing dependency `dyn Mailer`
///     ├─ needed by UserService (param `mailer`) in UsersModule
///     └─ help: import a module that exports `dyn Mailer`, or provide it in UsersModule
///
///   × scope violation: singleton `ReportService` depends on per-execution data
///     └─ ReportService → AuditContext (execution) → Ext<CurrentUser>
///        help: declare ReportService #[injectable(execution)], or inject a factory
/// ```
///
/// Type names are cut to their last path segment. Where the report would print two different
/// keys alike, `a::Config` and `b::Config`, or two different modules alike, `billing::Module` and
/// `users::Module`, it prints those keys or modules with their full paths. Text an entry carries
/// already rendered, such as what reads a missing key or the steps of a path, stays short. A
/// [`WiringError`] displayed on its own applies the same rule to the keys and modules it names.
///
/// `Debug` writes the same report, so `main` returning `Box<dyn Error>` prints it.
pub struct WiringErrors {
    errors: Vec<WiringError>,
}

impl WiringErrors {
    pub(crate) fn new(errors: Vec<WiringError>) -> Self {
        WiringErrors { errors }
    }

    pub fn iter(&self) -> std::slice::Iter<'_, WiringError> {
        self.errors.iter()
    }

    pub fn len(&self) -> usize {
        self.errors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}

impl<'a> IntoIterator for &'a WiringErrors {
    type Item = &'a WiringError;
    type IntoIter = std::slice::Iter<'a, WiringError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter()
    }
}

impl fmt::Display for WiringErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.errors.len();
        let noun = if count == 1 { "error" } else { "errors" };
        write!(f, "error: wiring failed with {count} {noun}")?;
        let full = Collisions {
            keys: colliding(self.errors.iter().flat_map(|error| error.key_names())),
            modules: colliding_names(self.errors.iter().flat_map(|error| error.module_names())),
        };
        for error in &self.errors {
            let text = Rendered { error, full: &full }.to_string();
            let mut lines = text.lines();
            if let Some(first) = lines.next() {
                write!(f, "\n\n  × {first}")?;
            }
            for line in lines {
                write!(f, "\n    {line}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for WiringErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for WiringErrors {}

/// One wiring failure. `at` is the source location of the registration call that declared the
/// offending item, captured with `#[track_caller]`.
#[non_exhaustive]
#[derive(Debug)]
pub enum WiringError {
    /// Step 1: the path of module names, from a module back to itself.
    ImportCycle { path: Vec<ModuleName> },
    /// Step 1: a re-export of a key the module cannot see.
    ReexportNotVisible { module: ModuleName, key: KeyName, at: &'static Location<'static> },
    /// Step 1: a re-export of a key the module sees from several sources.
    ReexportAmbiguous { module: ModuleName, key: KeyName, sources: Vec<ModuleName>, at: &'static Location<'static> },
    /// Step 1: an execution input declared inside a keyed module.
    KeyedInput { module: ModuleName, key: KeyName, at: &'static Location<'static> },
    /// Step 1: `export::<T>()` of a key the module does not bind. `imported` when the module sees
    /// the key through an import, which leaves through `reexport`; `near` is a key the module
    /// binds that is spelled the same up to a trailing `+ Send + Sync`, or by its last path
    /// segment.
    ExportNotBound { module: ModuleName, key: KeyName, imported: bool, near: Option<KeyName>, at: &'static Location<'static> },

    /// Step 2: a second single binding for a key in one module, naming both source locations.
    DuplicateBinding { key: KeyName, module: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },
    /// Step 2: `provide` and `contribute` under one key.
    KindMix { key: KeyName, module: ModuleName, single: &'static Location<'static>, collection: &'static Location<'static> },
    /// Step 2: an alias whose target nothing visible binds.
    DanglingAlias { alias: KeyName, target: KeyName, module: ModuleName, at: &'static Location<'static> },
    /// Step 2: the `Err` a `try_value` recorded, redacted: `ModuleDef::try_value`, or
    /// `Contribute::try_value`, whose `key` is the collection's.
    ValueFailed { module: ModuleName, key: KeyName, error: Redacted, at: &'static Location<'static> },
    /// Step 2: a qualified contribution under a role key, `key` the qualified collection. A
    /// transport reads only the role key's unqualified collection.
    QualifiedRoleContribution { key: KeyName, module: ModuleName, at: &'static Location<'static> },
    /// Step 2: a single binding under a role key, through its primary key, an `also_as` key or an
    /// alias. A transport reads enhancers only from the role key's collection. `key` is the
    /// binding's key that names the role key, qualified as the binding is, and `at` the call that
    /// registered the binding.
    SingleRoleBinding { key: KeyName, module: ModuleName, at: &'static Location<'static> },
    /// Step 2: two `.ready(..)` checks on one binding.
    DuplicateReadiness { key: KeyName, module: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },
    /// Step 2: an execution input a transport declares through `Transport::inputs` that has a
    /// second source. `first` is a transport's declaration; `second` is a module's
    /// `input::<T>().seeded_by::<Tr>()` naming another seeder, a later transport's declaration,
    /// or a single binding.
    InputConflict { key: KeyName, first: InputOrigin, second: InputOrigin },
    /// Step 2, tests: an override that matches no binding. `keyed` names a keyed module whose
    /// qualifier the override's key carries and which binds the key unqualified, as a keyed
    /// module's bindings are: `.in_module_keyed::<M, Q>()` reaches that binding as written.
    OverrideUnmatched { key: KeyName, keyed: Option<ModuleName>, at: &'static Location<'static> },
    /// Step 2, tests: an override that matches several bindings, listing every match; scope it
    /// with `.in_module::<M>()` or replace all with `.everywhere()`.
    OverrideAmbiguous { key: KeyName, matches: Vec<ModuleName>, at: &'static Location<'static> },
    /// Step 2, tests: `.in_module::<M>()` over several instances of `M`; name one with
    /// `.in_module_keyed::<M, Q>()` or `.in_module_of(&config)`.
    OverrideModuleAmbiguous { module: &'static str, candidates: Vec<ModuleName>, at: &'static Location<'static> },
    /// Step 2, tests: an override that would change a key's kind; `override_many` replaces a
    /// whole collection.
    OverrideKind { key: KeyName, expected: BindingKind, found: BindingKind, at: &'static Location<'static> },
    /// Step 2, tests: `override_value::<dyn Timer>`. Hint: "set it with `TestApp::timer(..)`".
    TimerOverride { at: &'static Location<'static> },
    /// Step 2, tests: a `replace_module` replacement that does not export a superset of the
    /// original's keys.
    ReplacementMissingExports { original: ModuleName, replacement: ModuleName, missing: Vec<KeyName> },
    /// Step 2, tests: a `replace_module` whose original no module imports, the same stale mock an
    /// override that matches nothing is.
    ReplacementUnmatched { original: ModuleName, at: &'static Location<'static> },
    /// Step 2, tests: a second `replace_module` of an original already replaced, naming both
    /// calls. Wiring applies the first.
    DuplicateReplacement { original: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },

    /// Step 3: an injection point whose key the module cannot see. `consumer` names what reads
    /// it, as in ``UserService (param `mailer`)``. `near` is a visible key spelled the same up to
    /// a trailing `+ Send + Sync`, or by its last path segment, with the module exporting it.
    Missing { key: KeyName, consumer: String, module: ModuleName, near: Option<(KeyName, ModuleName)> },
    /// Step 3: an injection point whose key the module sees from several sources, naming each.
    Ambiguous { key: KeyName, consumer: String, module: ModuleName, sources: Vec<(ModuleName, &'static Location<'static>)> },

    /// Step 4: the full path, as in `A → B → C → A`, each step with its module.
    Cycle { path: Vec<(KeyName, ModuleName)> },

    /// Step 5: a binding that cannot run in an execution depends on per-execution data. `path`
    /// is the dependency path that introduces it, each step as printed. `by_closure` when a
    /// factory closure builds the binding rather than a `Construct` type, so its scope is
    /// written where the closure is registered, not on an `#[injectable]`.
    ScopeViolation { binding: KeyName, declared: ScopeKind, by_closure: bool, module: ModuleName, path: Vec<String> },
    /// Step 5: hooks on an `Auto` binding the scope pass inferred per-execution, a contribution
    /// declared by closure included.
    HooksOnPerExecution { binding: KeyName, module: ModuleName },
    /// Step 5: an enhancer a handler declares by closure in an explicit singleton scope, which
    /// reads per-execution data. `closure` names it, as in ``method-level guard #2 of
    /// UsersController::get (Http)``, `role` as `guard`, `interceptor` or `error handler`;
    /// `path` runs from the injection point to the read of execution data.
    ClosureScopeViolation { closure: String, role: &'static str, path: Vec<String>, at: &'static Location<'static> },
    /// Step 5: a hook, readiness, module-hook or metadata closure that reads `Ext`,
    /// `ExecutionRef`, an execution input or a per-execution key, where no execution exists.
    /// `closure` names it, as in ``readiness check of `PgPool` in DbModule``; `path` runs from the
    /// injection point to the read of execution data. A metadata value has no `at`.
    ClosureNeedsExecution { closure: String, path: Vec<String>, at: Option<&'static Location<'static>> },
    /// Step 5: a non-optional input read on a path from a handler whose transport does not seed
    /// it, with the path from the handler to the service that reads it.
    InputNotSeeded { handler: String, transport: &'static str, input: KeyName, seeder: &'static str, path: Vec<String> },

    /// Step 6: an explicit bound with no `Timer`. `item` names the bounded item, as in
    /// ``readiness `.attempt_timeout` of `PgPool` ``.
    BoundWithoutTimer { item: String, at: &'static Location<'static> },
    /// Step 6: a readiness `.backoff(..)` on an app with no `Timer`, which cannot wait between
    /// attempts. `binding` is the checked binding as printed, and `at` the last `.backoff(..)`
    /// call.
    BackoffWithoutTimer { binding: String, at: &'static Location<'static> },
    /// Step 6: a builder knob set on an app with no `Timer`.
    KnobWithoutTimer { knob: &'static str },
}

impl WiringError {
    /// Every key this entry prints, for the check that decides which keys print with their full
    /// paths.
    fn key_names(&self) -> Vec<&KeyName> {
        match self {
            WiringError::ReexportNotVisible { key, .. }
            | WiringError::ReexportAmbiguous { key, .. }
            | WiringError::KeyedInput { key, .. }
            | WiringError::DuplicateBinding { key, .. }
            | WiringError::KindMix { key, .. }
            | WiringError::ValueFailed { key, .. }
            | WiringError::QualifiedRoleContribution { key, .. }
            | WiringError::SingleRoleBinding { key, .. }
            | WiringError::DuplicateReadiness { key, .. }
            | WiringError::InputConflict { key, .. }
            | WiringError::OverrideUnmatched { key, .. }
            | WiringError::OverrideAmbiguous { key, .. }
            | WiringError::OverrideKind { key, .. }
            | WiringError::Ambiguous { key, .. } => vec![key],
            WiringError::ExportNotBound { key, near, .. } => std::iter::once(key).chain(near).collect(),
            WiringError::DanglingAlias { alias, target, .. } => vec![alias, target],
            WiringError::ReplacementMissingExports { missing, .. } => missing.iter().collect(),
            WiringError::Missing { key, near, .. } => std::iter::once(key).chain(near.iter().map(|(bound, _)| bound)).collect(),
            WiringError::Cycle { path } => path.iter().map(|(key, _)| key).collect(),
            WiringError::ScopeViolation { binding, .. } | WiringError::HooksOnPerExecution { binding, .. } => vec![binding],
            WiringError::InputNotSeeded { input, .. } => vec![input],
            WiringError::ImportCycle { .. }
            | WiringError::OverrideModuleAmbiguous { .. }
            | WiringError::TimerOverride { .. }
            | WiringError::ReplacementUnmatched { .. }
            | WiringError::DuplicateReplacement { .. }
            | WiringError::ClosureNeedsExecution { .. }
            | WiringError::ClosureScopeViolation { .. }
            | WiringError::BoundWithoutTimer { .. }
            | WiringError::BackoffWithoutTimer { .. }
            | WiringError::KnobWithoutTimer { .. } => Vec::new(),
        }
    }

    /// Every module name this entry holds as a `ModuleName`, for the same check over modules.
    fn module_names(&self) -> Vec<&ModuleName> {
        match self {
            WiringError::ImportCycle { path } => path.iter().collect(),
            WiringError::ReexportNotVisible { module, .. }
            | WiringError::KeyedInput { module, .. }
            | WiringError::ExportNotBound { module, .. }
            | WiringError::DuplicateBinding { module, .. }
            | WiringError::KindMix { module, .. }
            | WiringError::DanglingAlias { module, .. }
            | WiringError::ValueFailed { module, .. }
            | WiringError::QualifiedRoleContribution { module, .. }
            | WiringError::SingleRoleBinding { module, .. }
            | WiringError::DuplicateReadiness { module, .. }
            | WiringError::ScopeViolation { module, .. }
            | WiringError::HooksOnPerExecution { module, .. } => vec![module],
            WiringError::ReexportAmbiguous { module, sources, .. } => std::iter::once(module).chain(sources).collect(),
            WiringError::OverrideUnmatched { keyed, .. } => keyed.iter().collect(),
            WiringError::OverrideAmbiguous { matches, .. } => matches.iter().collect(),
            WiringError::OverrideModuleAmbiguous { candidates, .. } => candidates.iter().collect(),
            WiringError::ReplacementMissingExports { original, replacement, .. } => vec![original, replacement],
            WiringError::ReplacementUnmatched { original, .. } | WiringError::DuplicateReplacement { original, .. } => {
                vec![original]
            }
            WiringError::Missing { module, near, .. } => {
                std::iter::once(module).chain(near.iter().map(|(_, exporter)| exporter)).collect()
            }
            WiringError::Ambiguous { module, sources, .. } => {
                std::iter::once(module).chain(sources.iter().map(|(source, _)| source)).collect()
            }
            WiringError::Cycle { path } => path.iter().map(|(_, module)| module).collect(),
            WiringError::InputConflict { first, second, .. } => first.module().into_iter().chain(second.module()).collect(),
            WiringError::OverrideKind { .. }
            | WiringError::TimerOverride { .. }
            | WiringError::ClosureNeedsExecution { .. }
            | WiringError::ClosureScopeViolation { .. }
            | WiringError::InputNotSeeded { .. }
            | WiringError::BoundWithoutTimer { .. }
            | WiringError::BackoffWithoutTimer { .. }
            | WiringError::KnobWithoutTimer { .. } => Vec::new(),
        }
    }

    /// The entry, each key and module in `full` printed with its full paths.
    fn render(&self, f: &mut fmt::Formatter<'_>, full: &Collisions) -> fmt::Result {
        let show = |name: &KeyName| if full.keys.contains(&name.key()) { format!("{name:#}") } else { name.to_string() };
        let show_module =
            |name: &ModuleName| if full.modules.contains(name) { format!("{name:#}") } else { name.to_string() };
        let type_text = |name: &KeyName| {
            let ty = name.key().type_name();
            if full.keys.contains(&name.key()) { ty.to_owned() } else { short_type_name(ty) }
        };
        match self {
            WiringError::ImportCycle { path } => tree(
                f,
                format!("import cycle: {}", closed_loop(path.iter().map(show_module).collect())),
                vec!["help: move what the modules in the cycle share into a module each of them imports".to_owned()],
            ),
            WiringError::ReexportNotVisible { module, key, at } => {
                let key = show(key);
                let module = show_module(module);
                tree(
                    f,
                    format!("re-export of `{key}` from {module}, which {module} cannot see"),
                    vec![
                        format!("declared at {}", place(at)),
                        format!("help: import a module that exports `{key}`, or remove the re-export"),
                    ],
                )
            }
            WiringError::ReexportAmbiguous { module, key, sources, at } => {
                let key = show(key);
                let module = show_module(module);
                let mut items: Vec<String> = sources.iter().map(|s| format!("exported by {}", show_module(s))).collect();
                items.push(format!("declared at {}", place(at)));
                items.push(format!("help: import `{key}` into {module} from one module only"));
                tree(f, format!("ambiguous re-export of `{key}` in {module}"), items)
            }
            WiringError::KeyedInput { module, key, at } => tree(
                f,
                format!("execution input `{}` declared in the keyed module {}", show(key), show_module(module)),
                vec![
                    format!("declared at {}", place(at)),
                    "help: inputs are app-wide and belong to transports; declare it in a module that is not keyed"
                        .to_owned(),
                ],
            ),
            WiringError::ExportNotBound { module, key, imported, near, at } => {
                let key = show(key);
                let module = show_module(module);
                let help = match near {
                    Some(near) => format!("help: {module} binds `{}`; the export names `{key}`", show(near)),
                    None if *imported => format!(
                        "help: an import of {module} provides `{key}`; re-export it with `reexport`, or bind it in {module}"
                    ),
                    None => format!("help: bind `{key}` in {module}, or remove the export"),
                };
                tree(
                    f,
                    format!("export of `{key}` from {module}, which {module} does not bind"),
                    vec![format!("declared at {}", place(at)), help],
                )
            }
            WiringError::DuplicateBinding { key, module, first, second } => tree(
                f,
                format!("duplicate binding for `{}` in {}", show(key), show_module(module)),
                vec![
                    format!("first bound at {}", place(first)),
                    format!("bound again at {}", place(second)),
                    "help: remove one, or give one a qualifier with `.qualified::<Q>()`".to_owned(),
                ],
            ),
            WiringError::KindMix { key, module, single, collection } => tree(
                f,
                format!("`{}` is bound both as a single binding and as a collection in {}", show(key), show_module(module)),
                vec![
                    format!("single binding at {}", place(single)),
                    format!("contribution at {}", place(collection)),
                    "help: contribute every entry with `contribute::<T>()` and read them as `Many<T>`, or bind it once"
                        .to_owned(),
                ],
            ),
            WiringError::DanglingAlias { alias, target, module, at } => {
                let target = show(target);
                let module = show_module(module);
                tree(
                    f,
                    format!("alias `{}` in {module} points at `{target}`, which {module} cannot see", show(alias)),
                    vec![
                        format!("declared at {}", place(at)),
                        format!("help: bind `{target}` in {module}, or import a module that exports it"),
                    ],
                )
            }
            WiringError::ValueFailed { module, key, error, at } => tree(
                f,
                format!("the value for `{}` in {} failed to build: {error}", show(key), show_module(module)),
                vec![format!("recorded by `try_value` at {}", place(at))],
            ),
            WiringError::QualifiedRoleContribution { key, module, at } => {
                let role = role_spelling(&type_text(key));
                tree(
                    f,
                    format!("a qualified contribution to the role key `{role}` in {} is read by no transport", show_module(module)),
                    vec![
                        format!("contributed as `{}` at {}", role_spelling(&show(key)), place(at)),
                        format!("help: contribute it unqualified; a transport reads only `{role}`"),
                    ],
                )
            }
            WiringError::SingleRoleBinding { key, module, at } => {
                let role = role_spelling(&type_text(key));
                tree(
                    f,
                    format!("a single binding under the role key `{role}` in {} is read by no transport", show_module(module)),
                    vec![
                        format!("bound as `{}` at {}", role_spelling(&show(key)), place(at)),
                        format!("help: contribute it: `into {role}: [..]`, or `m.contribute::<{role}>()`"),
                    ],
                )
            }
            WiringError::DuplicateReadiness { key, module, first, second } => tree(
                f,
                format!("two readiness checks on `{}` in {}", show(key), show_module(module)),
                vec![
                    format!("first `.ready(..)` at {}", place(first)),
                    format!("second `.ready(..)` at {}", place(second)),
                    "help: fold both into one check".to_owned(),
                ],
            ),
            WiringError::InputConflict { key, first, second } => tree(
                f,
                format!("execution input `{}` has two sources", show(key)),
                vec![
                    first.line(&show_module),
                    second.line(&show_module),
                    "help: an input is seeded by the one transport that declares it, and no module binds it; remove the other source"
                        .to_owned(),
                ],
            ),
            WiringError::OverrideUnmatched { key, keyed, at } => {
                let key = show(key);
                let help = match keyed {
                    Some(module) => format!(
                        "help: {} binds it unqualified, as a keyed module's bindings are; reach it with `.in_module_keyed::<M, Q>()` and no `.qualified`",
                        show_module(module)
                    ),
                    None => format!("help: remove the override, or bind `{key}` in the module the test expects"),
                };
                tree(f, format!("override of `{key}` matches no binding"), vec![format!("declared at {}", place(at)), help])
            }
            WiringError::OverrideAmbiguous { key, matches, at } => {
                let mut items: Vec<String> = matches.iter().map(|m| format!("bound in {}", show_module(m))).collect();
                items.push(format!("declared at {}", place(at)));
                items.push("help: scope it with `.in_module::<M>()`, or replace every match with `.everywhere()`".to_owned());
                tree(f, format!("override of `{}` matches several bindings", show(key)), items)
            }
            WiringError::OverrideModuleAmbiguous { module, candidates, at } => {
                let module = short_type_name(*module);
                let mut items: Vec<String> = candidates.iter().map(|c| format!("instance {}", show_module(c))).collect();
                items.push(format!("declared at {}", place(at)));
                items.push(format!("help: name one with `.in_module_keyed::<{module}, Q>()` or `.in_module_of(&config)`"));
                tree(f, format!("`.in_module::<{module}>()` matches several instances of {module}"), items)
            }
            WiringError::OverrideKind { key, expected, found, at } => tree(
                f,
                format!(
                    "override of `{}` would change its kind: the binding is {}, the override {}",
                    show(key),
                    kind_text(*expected),
                    kind_text(*found)
                ),
                vec![format!("declared at {}", place(at)), "help: `override_many` replaces a whole collection".to_owned()],
            ),
            WiringError::TimerOverride { at } => tree(
                f,
                "`override_value::<dyn Timer>` overrides app configuration, not a binding".to_owned(),
                vec![format!("declared at {}", place(at)), "help: set it with `TestApp::timer(..)`".to_owned()],
            ),
            WiringError::ReplacementMissingExports { original, replacement, missing } => {
                let original = show_module(original);
                let replacement = show_module(replacement);
                let mut items: Vec<String> = missing.iter().map(|k| format!("missing `{}`", show(k))).collect();
                items.push(format!("help: export the missing keys from {replacement}"));
                tree(f, format!("{replacement} replaces {original} without exporting every key {original} exports"), items)
            }
            WiringError::ReplacementUnmatched { original, at } => {
                let original = show_module(original);
                tree(
                    f,
                    format!("`replace_module` replaces {original}, which no module imports"),
                    vec![
                        format!("declared at {}", place(at)),
                        format!("help: remove the replacement, or import {original} where the test expects it"),
                    ],
                )
            }
            WiringError::DuplicateReplacement { original, first, second } => tree(
                f,
                format!("two `replace_module` calls replace {}", show_module(original)),
                vec![
                    format!("first `replace_module` at {}", place(first)),
                    format!("second `replace_module` at {}", place(second)),
                    "help: keep one; wiring applied only the first".to_owned(),
                ],
            ),
            WiringError::Missing { key, consumer, module, near } => {
                let key = show(key);
                let module = show_module(module);
                let help = match near {
                    Some((bound, exporter)) => {
                        format!("help: {} exports `{}`; the injection point reads `{key}`", show_module(exporter), show(bound))
                    }
                    None => format!("help: import a module that exports `{key}`, or provide it in {module}"),
                };
                tree(f, format!("missing dependency `{key}`"), vec![format!("needed by {consumer} in {module}"), help])
            }
            WiringError::Ambiguous { key, consumer, module, sources } => {
                let shown: Vec<(String, &Location<'static>)> = sources.iter().map(|(m, at)| (show_module(m), *at)).collect();
                let width = shown.iter().map(|(m, _)| m.chars().count()).max().unwrap_or(0);
                let mut items = vec![format!("needed by {consumer}")];
                items.extend(shown.iter().map(|(m, at)| format!("exported by {m:<width$}   ({})", place(at))));
                tree(f, format!("ambiguous dependency `{}` in {}", show(key), show_module(module)), items)
            }
            WiringError::Cycle { path } => {
                let mut steps: &[(KeyName, ModuleName)] = path;
                if let [first, .., last] = steps {
                    if first.0 == last.0 {
                        steps = &steps[..steps.len() - 1];
                    }
                }
                let head = format!("dependency cycle: {}", closed_loop(steps.iter().map(|(k, _)| show(k)).collect()));
                tree(f, head, steps.iter().map(|(k, m)| format!("`{}` in {}", show(k), show_module(m))).collect())
            }
            WiringError::ScopeViolation { binding, declared, by_closure, module, path } => {
                // The path's steps are printed short by the graph, so its first step is matched
                // against the short name whichever form the binding prints in.
                let short = binding.to_string();
                let name = show(binding);
                let module = show_module(module);
                let head = match (declared, by_closure) {
                    (ScopeKind::Singleton, _) => {
                        format!("scope violation: singleton `{name}` in {module} depends on per-execution data")
                    }
                    (ScopeKind::Auto, false) => format!(
                        "scope violation: `{name}` in {module} is a provider, so a singleton, and depends on per-execution data"
                    ),
                    (ScopeKind::Auto, true) => format!(
                        "scope violation: `{name}` in {module} is a provider declared by closure with no scope, so a singleton, and depends on per-execution data"
                    ),
                    _ => format!("scope violation: `{name}` in {module} depends on per-execution data"),
                };
                let help = match (declared, by_closure) {
                    (_, true) => "help: declare the closure `with(execution) = ..`, or register it with `.execution(..)`".to_owned(),
                    (ScopeKind::Singleton, false) => format!("help: declare {name} #[injectable(execution)], or inject a factory"),
                    _ => "help: declare it #[injectable(execution)]".to_owned(),
                };
                let item = match path.first() {
                    None => help,
                    Some(first) if first.starts_with(&short) => format!("{}\n{help}", path.join(" → ")),
                    Some(_) => format!("{name} → {}\n{help}", path.join(" → ")),
                };
                tree(f, head, vec![item])
            }
            WiringError::HooksOnPerExecution { binding, module } => {
                let binding = show(binding);
                tree(
                    f,
                    format!("lifecycle hooks on `{binding}` in {}, which the scope pass inferred per-execution", show_module(module)),
                    vec![format!(
                        "help: hooks run on singletons only; move them to a singleton, or remove what makes `{binding}` need an execution"
                    )],
                )
            }
            WiringError::ClosureNeedsExecution { closure, path, at } => {
                let mut items = Vec::new();
                if !path.is_empty() {
                    items.push(path.join(" → "));
                }
                if let Some(at) = at {
                    items.push(format!("declared at {}", place(at)));
                }
                items.push(
                    "help: hooks, readiness checks and metadata run where no execution exists; read singletons there, or take `ModuleRef` and open an execution with `execute`"
                        .to_owned(),
                );
                tree(f, format!("{closure} reads per-execution data"), items)
            }
            WiringError::ClosureScopeViolation { closure, role, path, at } => {
                let method = role.replace(' ', "_");
                let mut items = Vec::new();
                if !path.is_empty() {
                    items.push(path.join(" → "));
                }
                items.push(format!("declared at {}", place(at)));
                items.push(format!(
                    "help: build it per call: drop the scope (`with = ..`, `{method}_with`), or declare it `with(execution) = ..` (`{method}_with_in::<PerExecution>`)"
                ));
                tree(f, format!("scope violation: {closure}, declared by closure as a singleton, depends on per-execution data"), items)
            }
            WiringError::InputNotSeeded { handler, transport, input, seeder, path } => {
                let input = show(input);
                let transport = short_type_name(*transport);
                let seeder = short_type_name(*seeder);
                let start = format!("{handler} ({transport})");
                let route = match path.first() {
                    None => start,
                    Some(first) if first.starts_with(handler.as_str()) => path.join(" → "),
                    Some(_) => format!("{start} → {}", path.join(" → ")),
                };
                tree(
                    f,
                    format!("input `{input}` is seeded by {seeder}, read on a path from the {transport} handler {handler}"),
                    vec![route, format!("help: read it as `Option<Dep<{input}>>` where the path is shared across transports")],
                )
            }
            WiringError::BoundWithoutTimer { item, at } => tree(
                f,
                format!("{item} needs a `Timer`, and the app has none"),
                vec![
                    format!("declared at {}", place(at)),
                    "help: set one with `.timer(..)`, or leave the bound at its default or write `.unbounded()`".to_owned(),
                ],
            ),
            WiringError::BackoffWithoutTimer { binding, at } => tree(
                f,
                format!("readiness `.backoff` of `{binding}` waits between attempts on a `Timer`, and the app has none"),
                vec![format!("declared at {}", place(at)), "help: set one with `.timer(..)`, or remove the `.backoff(..)`".to_owned()],
            ),
            WiringError::KnobWithoutTimer { knob } => tree(
                f,
                format!("`{knob}` is set on an app with no `Timer`"),
                vec!["help: set one with `.timer(..)`; every timing knob is timed by it".to_owned()],
            ),
        }
    }
}

/// One entry of the report: a headline, then a tree of details whose last item is usually the
/// help. `WiringErrors` indents it under its `×`.
impl fmt::Display for WiringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = Collisions { keys: colliding(self.key_names()), modules: colliding_names(self.module_names()) };
        self.render(f, &full)
    }
}

/// One source of an execution input's key, as [`WiringError::InputConflict`] names it. `at` is the
/// call that declared or bound it.
///
/// `Display` writes the line the report prints, as in ``declared by transport `Http` at
/// src/transport.rs:40``; `{:#}` writes a module name with its full path.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum InputOrigin {
    /// The transport's `Transport::inputs`. `name` is the transport as reports name it, `Http`
    /// for `ulo_http::Http`.
    Transport { name: &'static str, at: &'static Location<'static> },
    /// A module's `input::<T>().seeded_by::<Tr>()`. `seeders` holds each `Tr`'s full type name,
    /// which the line prints by its last path segment.
    Module { module: ModuleName, seeders: Vec<&'static str>, at: &'static Location<'static> },
    /// A single binding under the input's key.
    Binding { module: ModuleName, at: &'static Location<'static> },
}

impl InputOrigin {
    fn module(&self) -> Option<&ModuleName> {
        match self {
            InputOrigin::Transport { .. } => None,
            InputOrigin::Module { module, .. } | InputOrigin::Binding { module, .. } => Some(module),
        }
    }

    /// The report's line for this source, `module` writing each module name.
    fn line(&self, module: impl Fn(&ModuleName) -> String) -> String {
        match self {
            InputOrigin::Transport { name, at } => format!("declared by transport `{name}` at {}", place(at)),
            InputOrigin::Module { module: declared_in, seeders, at } => {
                let noun = if seeders.len() == 1 { "seeder" } else { "seeders" };
                let names: Vec<String> = seeders.iter().map(|&seeder| format!("`{}`", short_type_name(seeder))).collect();
                format!("declared in {} with {noun} {} at {}", module(declared_in), names.join(", "), place(at))
            }
            InputOrigin::Binding { module: bound_in, at } => format!("bound in {} at {}", module(bound_in), place(at)),
        }
    }
}

impl fmt::Display for InputOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let full = f.alternate();
        f.write_str(&self.line(|name| if full { format!("{name:#}") } else { name.to_string() }))
    }
}

/// What one report prints with full paths: the keys and the modules whose short form a different
/// key, or a different module, in the same report shares.
struct Collisions {
    keys: HashSet<Key>,
    modules: HashSet<ModuleName>,
}

/// An entry rendered against the whole report's collisions.
struct Rendered<'a> {
    error: &'a WiringError,
    full: &'a Collisions,
}

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.render(f, self.full)
    }
}

/// The keys among `names` whose short form another, different key shares, `a::Config` beside
/// `b::Config`. A key's kind is left out of the comparison: `Config` and `Config (collection)`
/// read alike too.
fn colliding<'a>(names: impl IntoIterator<Item = &'a KeyName>) -> HashSet<Key> {
    Key::colliding(names.into_iter().map(KeyName::key))
}

/// Writes `head`, then each item on its own line under `├─`, the last under `└─`. A line break
/// inside an item continues it under the same branch.
fn tree(f: &mut fmt::Formatter<'_>, head: String, items: Vec<String>) -> fmt::Result {
    f.write_str(&head)?;
    let last = items.len().saturating_sub(1);
    for (i, item) in items.iter().enumerate() {
        let (branch, continued) = if i == last { ("└─", "   ") } else { ("├─", "│  ") };
        let mut lines = item.lines();
        if let Some(first) = lines.next() {
            write!(f, "\n{branch} {first}")?;
        }
        for line in lines {
            write!(f, "\n{continued}{line}")?;
        }
    }
    Ok(())
}

/// `A → B → C → A`: the steps with the first repeated at the end unless it already is.
fn closed_loop(mut steps: Vec<String>) -> String {
    let closed = steps.len() > 1 && steps.first() == steps.last();
    if !closed {
        if let Some(first) = steps.first().cloned() {
            steps.push(first);
        }
    }
    steps.join(" → ")
}

fn place(at: &Location<'_>) -> String {
    format!("{}:{}", at.file(), at.line())
}

fn kind_text(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Single => "a single binding",
        BindingKind::Collection => "a collection",
    }
}
