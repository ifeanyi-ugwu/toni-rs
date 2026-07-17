use std::process::Command;

/// A cross-module provider cycle must fail with a diagnostic that names the exact providers
/// in the cycle, not just the modules involved. The fixture binary builds such a cycle and
/// exits non-zero; this asserts the message it logs.
///
/// Runs as a subprocess because the only public trigger path (`ToniFactory::create*`) calls
/// `std::process::exit(1)` on init failure, which would abort the test runner in-process.
#[test]
fn cross_module_provider_cycle_names_the_exact_providers() {
    let output = Command::new(env!("CARGO_BIN_EXE_circular_dep_fixture"))
        .output()
        .expect("fixture binary should run");

    assert!(
        !output.status.success(),
        "fixture should fail on the cross-module cycle, exit status: {:?}",
        output.status
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        combined.contains("Circular dependency detected between providers"),
        "expected the sharpened cycle diagnostic, got:\n{combined}"
    );
    assert!(
        combined.contains("ServiceA") && combined.contains("ServiceB"),
        "diagnostic should name both providers in the cycle, got:\n{combined}"
    );
    assert!(
        combined.contains("Break the cycle"),
        "diagnostic should include the remediation guidance, got:\n{combined}"
    );
}
