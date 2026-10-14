//! `--version` and `-V` answer before .env or any setting is read: the release
//! workflow smoke-tests the binary that way, with no configuration or database.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs the relay with `flag` and returns its standard output, failing the
/// test unless it exits successfully within 5 seconds.
fn run(flag: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_artemis-server"))
        .arg(flag)
        .stdout(Stdio::piped())
        .spawn()
        .expect("start artemis-server");
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().expect("wait for artemis-server").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("artemis-server {flag} was still running after 5 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child
        .wait_with_output()
        .expect("read artemis-server output");
    assert!(
        output.status.success(),
        "artemis-server {flag} exited with {}",
        output.status
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

#[test]
fn version_flags_print_the_name_and_version() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            run(flag),
            format!("artemis-server {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
