//! Programs the transport attributes must refuse, each beside the diagnostic it gets:
//! metadata of one transport on another transport's handler (X24), and gRPC handlers whose
//! signatures disagree with their method marker's shape.
//!
//! The `.stderr` snapshots hold rustc's own wording, which changes between releases, so the test
//! runs only on the compiler that wrote them; `build.rs` sets `ulo_snapshot_rustc` there. On any
//! other compiler it is reported ignored. Regenerate on that compiler after an intended change:
//!
//! ```text
//! TRYBUILD=overwrite cargo +1.98.1 test -p ulo-codegen-tests --test compile_fail
//! ```

#[test]
#[cfg_attr(not(ulo_snapshot_rustc), ignore = "the .stderr snapshots are rustc 1.98.1's")]
fn mismatched_handlers_fail_to_compile() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
