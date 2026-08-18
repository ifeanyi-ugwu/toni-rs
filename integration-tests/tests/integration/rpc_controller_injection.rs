use std::process::Command;

/// An RPC controller is a dispatch target: it is reached by pattern and nothing may hold it. The
/// fixture binary injects one into an ordinary provider and exits non-zero; this asserts the
/// message it logs says why rather than reporting the token as missing.
///
/// Runs as a subprocess because the only public trigger path (`ToniFactory::create*`) calls
/// `std::process::exit(1)` on init failure, which would abort the test runner in-process.
#[test]
fn injecting_an_rpc_controller_is_refused() {
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
        combined.contains("is an RPC controller and cannot be injected"),
        "expected the dispatch-target diagnostic, got:\n{combined}"
    );
    assert!(
        combined.contains("OrdersController"),
        "diagnostic should name the controller, got:\n{combined}"
    );
}
