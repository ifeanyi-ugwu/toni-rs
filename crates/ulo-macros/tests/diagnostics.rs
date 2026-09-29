//! Every diagnostic the macros and the declaration API document, executed.
//!
//! These macros' contract with a user is substantially a set of error
//! messages: what to write instead of `:param`, which two parameters broke the
//! one-body rule, that a handler impl may name one transport. A regression
//! there does not break a build — it replaces a message that says what to do
//! with one that does not, which is the entire value of having written a
//! custom diagnostic.
//!
//! Each case is a file that must fail to compile, beside the `.stderr` it must
//! produce. Regenerate after an intended change:
//!
//! ```text
//! rustup run 1.98.1 -- env TRYBUILD=overwrite \
//!     cargo test -p ulo-macros --test diagnostics
//! ```
//!
//! The version matters. Many of these snapshots carry rustc's own rendering:
//! below the macro's text, around a `diagnostic::on_unimplemented` message, or
//! as the whole diagnostic where a case pins that rustc's error lands on the
//! user's code. rustc rewords it every few releases, and where the phrase a
//! case pins is rustc's own, a rewording fails `every_case_reaches_its_diagnostic`
//! as well as the snapshot. CI runs this target on a pinned compiler rather
//! than on `stable` — see the `diagnostics` job in `.github/workflows/ci.yml`,
//! which holds the version and the reason. Regenerating on a different
//! compiler produces a snapshot that only fails in CI.
//!
//! Writing a case is not enough on its own. trybuild accepts any stable
//! output, so a case that stops reaching its diagnostic and starts failing
//! earlier — a parse error, a missing import — still passes once its snapshot
//! is regenerated. `every_case_reaches_its_diagnostic` pins the words instead,
//! and it is the check that reports a case of that kind. `#[websocket_gateway]`
//! had one: its arg parser left the inline struct unconsumed, so syn reported
//! "unexpected token" and the message naming the new form never ran.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/diagnostics")
}

/// The phrase each case exists to pin. Matched against the recorded `.stderr`.
const DOCUMENTED: &[(&str, &str)] = &[
    (
        "param_syntax",
        "uses `:param` syntax; ulo's parameter syntax is `{id}`",
    ),
    (
        "catch_takes_shared_refs",
        "#[catch(T)] arguments must be shared references, not &mut",
    ),
    (
        "controller_inline_struct_form",
        "the inline-struct form `#[controller(\"/p\", pub struct …)]` has been removed",
    ),
    (
        "gateway_inline_struct_form",
        "the inline-struct form `#[websocket_gateway(\"/p\", pub struct …)]` has been removed",
    ),
    (
        "one_transport_per_struct",
        "duplicate definitions with name `__ulo_dispatch`",
    ),
    (
        "one_body_per_handler",
        "both read the request body, and it can only be read once",
    ),
    (
        "option_forwards_consumes",
        "both read the request body, and it can only be read once",
    ),
    (
        "rpc_params_are_extractors",
        "is not an extractor for `RpcContext`",
    ),
    (
        "sse_answers_a_stream",
        "an `#[sse]` handler must answer with a stream of events",
    ),
    (
        "scope_request_renamed",
        "scope = \"request\" is now scope = \"execution\"",
    ),
    ("provide_factory_is_async", "is not an async factory"),
    (
        "role_slot_holds_its_role",
        "the trait bound `u8: ulo::enhancer::Guard<HttpContext>` is not satisfied",
    ),
    (
        "provide_key_holds_the_declared_type",
        "expected `Arc<u16>`, found `Arc<u32>`",
    ),
    (
        "provide_into_holds_its_trait",
        "the trait bound `NotAPlugin: Plugin` is not satisfied",
    ),
    (
        "provide_bare_name_is_a_type",
        "`Unit` has no provider declaration of its own",
    ),
    (
        "provide_key_is_a_type_implementing_key",
        "`Plain` is not a key",
    ),
    (
        "inject_key_holds_the_field",
        "type mismatch resolving `<Port as Key>::Value == String`",
    ),
    ("inject_key_is_a_key", "`Plain` is not a key"),
    ("inject_takes_no_string", "a key is a type, not a string"),
    (
        "provide_key_takes_no_string",
        "a key is a type, not a string",
    ),
    (
        "module_export_takes_no_string",
        "a key is a type, not a string",
    ),
    (
        "module_controller_is_a_dispatch_target",
        "`Service` is not a dispatch target",
    ),
    (
        "construction_takes_no_init",
        "Unknown #[injectable] key: 'init'. Expected 'scope'",
    ),
    ("default_beside_new", "is built by its `#[new]` constructor"),
    (
        "new_twice",
        "duplicate definitions with name `__ULO_ONE_NEW_PER_TYPE`",
    ),
    (
        "hook_on_a_scope_that_never_fires",
        "never fires: `Session` is execution-scoped",
    ),
    (
        "rpc_pattern_repeated",
        "is already declared by `first` in this impl",
    ),
    (
        "use_guards_takes_no_string",
        "a key is a type, not a string",
    ),
    (
        "error_handler_factory_is_singleton",
        "is built once, not per execution or per resolution",
    ),
    (
        "error_handler_takes_no_closure",
        "an error handler is built once and shared, so the closure form has nothing to build per execution",
    ),
];

#[test]
fn diagnostics_say_what_to_do_instead() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/diagnostics/*.rs");
}

#[test]
fn every_case_reaches_its_diagnostic() {
    let wrong: Vec<String> = DOCUMENTED
        .iter()
        .filter_map(|(case, phrase)| {
            let path = dir().join(format!("{case}.stderr"));
            let Ok(stderr) = fs::read_to_string(&path) else {
                return Some(format!("  {case}: no {case}.stderr recorded"));
            };
            // trybuild wraps long lines; compare with whitespace collapsed so a
            // rewrap is not read as a changed message.
            let flat = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
            let want = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
            (!flat.contains(&want))
                .then(|| format!("  {case}: recorded output does not contain {phrase:?}"))
        })
        .collect();

    assert!(
        wrong.is_empty(),
        "cases failing for something other than the diagnostic they pin:\n{}\n\n\
         A case that fails earlier — a parse error, a missing import — still \
         produces stable output and still passes trybuild.",
        wrong.join("\n")
    );
}

/// A case file with no entry above is pinned only by its snapshot, which is the
/// gap this suite exists to close.
#[test]
fn every_case_declares_what_it_pins() {
    let declared: BTreeSet<&str> = DOCUMENTED.iter().map(|(case, _)| *case).collect();

    let on_disk: BTreeSet<String> = fs::read_dir(dir())
        .expect("the case directory is readable")
        .map(|e| e.expect("a directory entry is readable").path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .map(|p| {
            p.file_stem()
                .expect("a .rs path has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let undeclared: Vec<&String> = on_disk
        .iter()
        .filter(|case| !declared.contains(case.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "case files with no line in DOCUMENTED: {undeclared:?}\n\
         Add the phrase the case exists to pin."
    );

    let missing: Vec<&&str> = declared
        .iter()
        .filter(|case| !on_disk.contains(**case))
        .collect();
    assert!(
        missing.is_empty(),
        "DOCUMENTED names cases with no file: {missing:?}"
    );
}
