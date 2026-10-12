use std::process::Command;

#[test]
fn scenarios_are_selected_in_the_ui_instead_of_cli_arguments() {
    for scenario in [
        "commit",
        "no-vote",
        "missing-vote",
        "coordinator-loss",
        "partial-delivery",
        "recovery",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_two-phase-commit"))
            .arg(scenario)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("usage: two-phase-commit [--port PORT]"));
    }
}
