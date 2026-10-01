use std::error::Error;
use std::fmt;
use std::panic::Location;

use crate::key::{BindingKind, KeyName, short_type_name};
use crate::module::ModuleName;
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
        for error in &self.errors {
            let text = error.to_string();
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

    /// Step 2: a second single binding for a key in one module, naming both source locations.
    DuplicateBinding { key: KeyName, module: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },
    /// Step 2: `provide` and `contribute` under one key.
    KindMix { key: KeyName, module: ModuleName, single: &'static Location<'static>, collection: &'static Location<'static> },
    /// Step 2: an alias whose target nothing visible binds.
    DanglingAlias { alias: KeyName, target: KeyName, module: ModuleName, at: &'static Location<'static> },
    /// Step 2: the `Err` a `try_value` recorded, redacted.
    ValueFailed { module: ModuleName, key: KeyName, error: Redacted, at: &'static Location<'static> },
    /// Step 2: two `.ready(..)` checks on one binding.
    DuplicateReadiness { key: KeyName, module: ModuleName, first: &'static Location<'static>, second: &'static Location<'static> },
    /// Step 2, tests: an override that matches no binding.
    OverrideUnmatched { key: KeyName, at: &'static Location<'static> },
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

    /// Step 3: an injection point whose key the module cannot see. `consumer` names what reads
    /// it, as in ``UserService (param `mailer`)``. `near` is a visible key spelled the same up to
    /// a trailing `+ Send + Sync`, with the module exporting it.
    Missing { key: KeyName, consumer: String, module: ModuleName, near: Option<(KeyName, ModuleName)> },
    /// Step 3: an injection point whose key the module sees from several sources, naming each.
    Ambiguous { key: KeyName, consumer: String, module: ModuleName, sources: Vec<(ModuleName, &'static Location<'static>)> },

    /// Step 4: the full path, as in `A → B → C → A`, each step with its module.
    Cycle { path: Vec<(KeyName, ModuleName)> },

    /// Step 5: a binding that cannot run in an execution depends on per-execution data. `path`
    /// is the dependency path that introduces it, each step as printed.
    ScopeViolation { binding: KeyName, declared: ScopeKind, module: ModuleName, path: Vec<String> },
    /// Step 5: hooks on an `Auto` binding the scope pass inferred per-execution.
    HooksOnPerExecution { binding: KeyName, module: ModuleName },
    /// Step 5: a non-optional input read on a path from a handler whose transport does not seed
    /// it, with the path from the handler to the service that reads it.
    InputNotSeeded { handler: String, transport: &'static str, input: KeyName, seeder: &'static str, path: Vec<String> },

    /// Step 6: an explicit bound with no `Timer`. `item` names the bounded item, as in
    /// ``readiness `.attempt_timeout` of `PgPool` ``.
    BoundWithoutTimer { item: String, at: &'static Location<'static> },
    /// Step 6: a builder knob set on an app with no `Timer`.
    KnobWithoutTimer { knob: &'static str },
}

/// One entry of the report: a headline, then a tree of details whose last item is usually the
/// help. `WiringErrors` indents it under its `×`.
impl fmt::Display for WiringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WiringError::ImportCycle { path } => tree(
                f,
                format!("import cycle: {}", closed_loop(path.iter().map(ToString::to_string).collect())),
                vec!["help: move what the modules in the cycle share into a module each of them imports".to_owned()],
            ),
            WiringError::ReexportNotVisible { module, key, at } => tree(
                f,
                format!("re-export of `{key}` from {module}, which {module} cannot see"),
                vec![
                    format!("declared at {}", place(at)),
                    format!("help: import a module that exports `{key}`, or remove the re-export"),
                ],
            ),
            WiringError::ReexportAmbiguous { module, key, sources, at } => {
                let mut items: Vec<String> = sources.iter().map(|s| format!("exported by {s}")).collect();
                items.push(format!("declared at {}", place(at)));
                items.push(format!("help: import `{key}` into {module} from one module only"));
                tree(f, format!("ambiguous re-export of `{key}` in {module}"), items)
            }
            WiringError::KeyedInput { module, key, at } => tree(
                f,
                format!("execution input `{key}` declared in the keyed module {module}"),
                vec![
                    format!("declared at {}", place(at)),
                    "help: inputs are app-wide and belong to transports; declare it in a module that is not keyed"
                        .to_owned(),
                ],
            ),
            WiringError::DuplicateBinding { key, module, first, second } => tree(
                f,
                format!("duplicate binding for `{key}` in {module}"),
                vec![
                    format!("first bound at {}", place(first)),
                    format!("bound again at {}", place(second)),
                    "help: remove one, or give one a qualifier with `.qualified::<Q>()`".to_owned(),
                ],
            ),
            WiringError::KindMix { key, module, single, collection } => tree(
                f,
                format!("`{key}` is bound both as a single binding and as a collection in {module}"),
                vec![
                    format!("single binding at {}", place(single)),
                    format!("contribution at {}", place(collection)),
                    "help: contribute every entry with `contribute::<T>()` and read them as `Many<T>`, or bind it once"
                        .to_owned(),
                ],
            ),
            WiringError::DanglingAlias { alias, target, module, at } => tree(
                f,
                format!("alias `{alias}` in {module} points at `{target}`, which {module} cannot see"),
                vec![
                    format!("declared at {}", place(at)),
                    format!("help: bind `{target}` in {module}, or import a module that exports it"),
                ],
            ),
            WiringError::ValueFailed { module, key, error, at } => tree(
                f,
                format!("the value for `{key}` in {module} failed to build: {error}"),
                vec![format!("recorded by `try_value` at {}", place(at))],
            ),
            WiringError::DuplicateReadiness { key, module, first, second } => tree(
                f,
                format!("two readiness checks on `{key}` in {module}"),
                vec![
                    format!("first `.ready(..)` at {}", place(first)),
                    format!("second `.ready(..)` at {}", place(second)),
                    "help: fold both into one check".to_owned(),
                ],
            ),
            WiringError::OverrideUnmatched { key, at } => tree(
                f,
                format!("override of `{key}` matches no binding"),
                vec![
                    format!("declared at {}", place(at)),
                    format!("help: remove the override, or bind `{key}` in the module the test expects"),
                ],
            ),
            WiringError::OverrideAmbiguous { key, matches, at } => {
                let mut items: Vec<String> = matches.iter().map(|m| format!("bound in {m}")).collect();
                items.push(format!("declared at {}", place(at)));
                items.push("help: scope it with `.in_module::<M>()`, or replace every match with `.everywhere()`".to_owned());
                tree(f, format!("override of `{key}` matches several bindings"), items)
            }
            WiringError::OverrideModuleAmbiguous { module, candidates, at } => {
                let module = short_type_name(*module);
                let mut items: Vec<String> = candidates.iter().map(|c| format!("instance {c}")).collect();
                items.push(format!("declared at {}", place(at)));
                items.push(format!("help: name one with `.in_module_keyed::<{module}, Q>()` or `.in_module_of(&config)`"));
                tree(f, format!("`.in_module::<{module}>()` matches several instances of {module}"), items)
            }
            WiringError::OverrideKind { key, expected, found, at } => tree(
                f,
                format!(
                    "override of `{key}` would change its kind: the binding is {}, the override {}",
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
                let mut items: Vec<String> = missing.iter().map(|k| format!("missing `{k}`")).collect();
                items.push(format!("help: export the missing keys from {replacement}"));
                tree(f, format!("{replacement} replaces {original} without exporting every key {original} exports"), items)
            }
            WiringError::Missing { key, consumer, module, near } => {
                let help = match near {
                    Some((bound, exporter)) => format!("help: {exporter} exports `{bound}`; the injection point reads `{key}`"),
                    None => format!("help: import a module that exports `{key}`, or provide it in {module}"),
                };
                tree(f, format!("missing dependency `{key}`"), vec![format!("needed by {consumer} in {module}"), help])
            }
            WiringError::Ambiguous { key, consumer, module, sources } => {
                let width = sources.iter().map(|(m, _)| m.as_str().chars().count()).max().unwrap_or(0);
                let mut items = vec![format!("needed by {consumer}")];
                items.extend(sources.iter().map(|(m, at)| format!("exported by {:<width$}   ({})", m.as_str(), place(at))));
                tree(f, format!("ambiguous dependency `{key}` in {module}"), items)
            }
            WiringError::Cycle { path } => {
                let mut steps: &[(KeyName, ModuleName)] = path;
                if let [first, .., last] = steps {
                    if first.0 == last.0 {
                        steps = &steps[..steps.len() - 1];
                    }
                }
                let head = format!("dependency cycle: {}", closed_loop(steps.iter().map(|(k, _)| k.to_string()).collect()));
                tree(f, head, steps.iter().map(|(k, m)| format!("`{k}` in {m}")).collect())
            }
            WiringError::ScopeViolation { binding, declared, module, path } => {
                let head = match declared {
                    ScopeKind::Singleton => {
                        format!("scope violation: singleton `{binding}` in {module} depends on per-execution data")
                    }
                    ScopeKind::Auto => format!(
                        "scope violation: `{binding}` in {module} is a provider, so a singleton, and depends on per-execution data"
                    ),
                    _ => format!("scope violation: `{binding}` in {module} depends on per-execution data"),
                };
                let help = match declared {
                    ScopeKind::Singleton => format!("help: declare {binding} #[injectable(execution)], or inject a factory"),
                    _ => "help: declare it #[injectable(execution)]".to_owned(),
                };
                let name = binding.to_string();
                let item = match path.first() {
                    None => help,
                    Some(first) if first.starts_with(&name) => format!("{}\n{help}", path.join(" → ")),
                    Some(_) => format!("{name} → {}\n{help}", path.join(" → ")),
                };
                tree(f, head, vec![item])
            }
            WiringError::HooksOnPerExecution { binding, module } => tree(
                f,
                format!("lifecycle hooks on `{binding}` in {module}, which the scope pass inferred per-execution"),
                vec![format!(
                    "help: hooks run on singletons only; move them to a singleton, or remove what makes `{binding}` need an execution"
                )],
            ),
            WiringError::InputNotSeeded { handler, transport, input, seeder, path } => {
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
            WiringError::KnobWithoutTimer { knob } => tree(
                f,
                format!("`{knob}` is set on an app with no `Timer`"),
                vec!["help: set one with `.timer(..)`; every timing knob is timed by it".to_owned()],
            ),
        }
    }
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
