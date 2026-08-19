use std::process::Command;

/// A gRPC service is a dispatch target: it is reached by its transport and nothing may hold it. The
/// fixture binary declares one in `controllers:` and injects it into an ordinary provider; the token
/// is not in the provider store, so resolution fails and init exits non-zero.
///
/// Runs as a subprocess because the only public trigger path (`ToniFactory::create*`) calls
/// `std::process::exit(1)` on init failure, which would abort the test runner in-process.
///
/// The other half of the refusal is not reachable from here: listing a dispatch target in
/// `providers:` does not compile, because the macro emits no provider factory for one.
#[test]
fn a_grpc_service_is_not_resolvable_as_a_dependency() {
    let output = Command::new(env!("CARGO_BIN_EXE_grpc_service_injection_fixture"))
        .output()
        .expect("fixture binary should run");

    assert!(
        !output.status.success(),
        "fixture should fail on the injected service, exit status: {:?}",
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
        combined.contains("OrdersGrpcService"),
        "the failure should name the service, got:\n{combined}"
    );
}
