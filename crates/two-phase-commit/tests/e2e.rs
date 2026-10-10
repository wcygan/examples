use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(3);

fn finish_with_timeout(mut child: Child, scenario: &str) -> std::process::Output {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        match child.try_wait().expect("poll scenario child") {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                child
                    .stdout
                    .take()
                    .expect("piped stdout")
                    .read_to_end(&mut stdout)
                    .expect("read stdout");
                child
                    .stderr
                    .take()
                    .expect("piped stderr")
                    .read_to_end(&mut stderr)
                    .expect("read stderr");
                return std::process::Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "scenario {scenario} exceeded {PROCESS_TIMEOUT:?}; child was killed and reaped"
                );
            }
        }
    }
}

fn invoke(args: &[&str]) -> std::process::Output {
    let child = Command::new(env!("CARGO_BIN_EXE_two-phase-commit"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn demo process");
    finish_with_timeout(child, args.first().copied().unwrap_or("default"))
}

fn scenario(name: &str) -> String {
    let out = invoke(&[name]);
    assert!(
        out.status.success(),
        "scenario {name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("stdout is UTF-8")
}

#[test]
fn executable_commit_and_abort_preserve_exact_account_values() {
    for _ in 0..3 {
        let commit = scenario("commit");
        assert!(commit.contains("DECISION Commit"));
        assert!(commit.contains("IMMUTABLE decision remains Commit"));
        assert!(commit.contains("STATES source-account=Committed, destination-account=Committed"));
        assert!(commit.contains("BALANCES source=90 destination=30"));
        assert!(commit.contains("RESULT committed"));

        let abort = scenario("no-vote");
        assert!(abort.contains("DECISION Abort"));
        assert!(abort.contains("STATES source-account=Aborted, destination-account=Aborted"));
        assert!(abort.contains("BALANCES source=100 destination=20"));
        assert!(abort.contains("RESULT aborted"));
    }
}

#[test]
fn executable_failure_boundaries_assert_states_and_recovery() {
    for _ in 0..3 {
        let missing = scenario("missing-vote");
        assert!(missing.contains("votes=1/2; decision=None; prepared=2"));

        let loss = scenario("coordinator-loss");
        assert!(loss.contains("votes=1/2; prepared_after_timeout=2; decision=None"));

        let partial = scenario("partial-delivery");
        assert!(partial
            .contains("PARTIAL STATES source-account=Committed, destination-account=Prepared"));
        assert!(partial.contains("PARTIAL BALANCES source=90 destination=20"));
        assert!(partial.contains("IMMUTABLE decision remains Commit"));
        assert!(partial.contains("RECOVERED balances source=90 destination=30"));
        assert!(partial.contains("STATES source-account=Committed, destination-account=Committed"));

        let recovery = scenario("recovery");
        assert!(recovery
            .contains("PARTIAL STATES source-account=Committed, destination-account=Committed"));
        assert!(recovery.contains(
            "IDEMPOTENT balances unchanged by duplicate decision: source=90 destination=30"
        ));
    }
}

#[test]
fn invalid_cli_input_exits_nonzero_with_usage() {
    let out = invoke(&["not-a-scenario"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: two-phase-commit"));
}
