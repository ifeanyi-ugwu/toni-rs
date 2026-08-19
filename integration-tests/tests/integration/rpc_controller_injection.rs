use std::process::Command;

/// An RPC controller is a dispatch target: it is reached by pattern and nothing may hold it. The
/// fixture binary declares one in `controllers:` and injects it into an ordinary provider; the token
/// is not in the provider store, so resolution fails and init exits non-zero.
///
/// Runs as a subprocess because the only public trigger path (`ToniFactory::create*`) calls
/// `std::process::exit(1)` on init failure, which would abort the test runner in-process.
///
/// The other half of the refusal is not reachable from here: listing a dispatch target in
/// `providers:` does not compile, because the macro emits no provider factory for one.
#[test]
fn an_rpc_controller_is_not_resolvable_as_a_dependency() {
    let output = Command::new(env!("CARGO_BIN_EXE_rpc_controller_injection_fixture"))
        .output()
        .expect("fixture binary should run");

    assert!(
        !output.status.success(),
        "fixture should fail on the injected controller, exit status: {:?}",
        output.status
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        combined.contains("Dependency not found"),
        "expected an unresolved-dependency failure, got:\n{combined}"
    );
    assert!(
        combined.contains("OrdersController"),
        "the failure should name the controller, got:\n{combined}"
    );
}
