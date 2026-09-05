use std::{
    fs,
    io::Read,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn closed_stdout_does_not_abort_during_timing_cleanup() {
    let dir = std::env::temp_dir().join(format!(
        "deploy-helper-timing-pipe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("servers.yml"),
        "hosts:\n  local:\n    host: localhost\n",
    )
    .unwrap();
    fs::write(
        dir.join("deploy.yml"),
        serde_json::to_vec(&serde_json::json!([{
            "name": "Closed pipe", "hosts": "local", "tasks": [{
                "name": "Fill pipe", "debug": {"msg": "x".repeat(128 * 1024)}
            }]
        }]))
        .unwrap(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_deploy-helper"))
        .arg(dir.join("deploy.yml"))
        .arg("-i")
        .arg(dir.join("servers.yml"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    stdout.read_exact(&mut [0]).unwrap();
    drop(stdout);
    let output = child.wait_with_output().unwrap();
    fs::remove_dir_all(&dir).unwrap();
    // Existing task output still panics on a closed pipe. Timing cleanup must
    // let that unwind normally instead of causing a second panic and aborting.
    assert_eq!(
        output.status.code(),
        Some(101),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn timings_cover_failure_recovery_skips_and_includes() {
    let dir = std::env::temp_dir().join(format!(
        "deploy-helper-timing-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("servers.yml"),
        "hosts:\n  local:\n    host: localhost\n",
    )
    .unwrap();
    fs::write(
        dir.join("included.yml"),
        "- name: Nested\n  debug:\n    msg: hello\n",
    )
    .unwrap();
    fs::write(
        dir.join("deploy.yml"),
        r#"
- name: Timing test
  hosts: local
  tasks:
    - name: Parent
      include_tasks: included.yml
    - name: Skipped
      when: false
      debug:
        msg: skipped
    - name: Secret
      no_log: true
      debug:
        msg: secret-value
    - name: Fails
      command: deploy-helper-nonexistent-timing-test-command
  on_failure:
    - name: Recovery
      debug:
        msg: recovering
  always:
    - name: Cleanup
      debug:
        msg: cleanup
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_deploy-helper"))
        .arg(dir.join("deploy.yml"))
        .arg("-i")
        .arg(dir.join("servers.yml"))
        .output()
        .unwrap();
    fs::remove_dir_all(&dir).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!output.status.success());
    assert!(stdout.contains("[failed] Fails"), "{stdout}");
    assert!(stdout.contains("[ok] Nested"));
    assert!(stdout.contains("[ok] Parent"));
    assert!(stdout.contains("[ok] [no_log]"));
    assert!(!stdout.contains("secret-value"));
    assert!(
        stdout.contains("Run summary: 5 succeeded, 1 failed, 1 skipped, 2 recovery tasks;"),
        "{stdout}"
    );
}
