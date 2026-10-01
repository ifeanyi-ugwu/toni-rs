use std::error::Error;
use std::fmt;
use std::panic::Location;

use crate::key::{BindingKind, KeyName};
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
#[derive(Debug)]
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
        todo!()
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

    /// Step 3: a site whose key the module cannot see. `consumer` names what reads it, as in
    /// ``UserService (param `mailer`)``. `near` is a visible key spelled the same up to a trailing
    /// `+ Send + Sync`, with the module exporting it.
    Missing { key: KeyName, consumer: String, module: ModuleName, near: Option<(KeyName, ModuleName)> },
    /// Step 3: a site whose key the module sees from several sources, naming each.
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

impl fmt::Display for WiringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}
