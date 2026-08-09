use expectrl::{process::Healthcheck, Expect, Session};
use std::fs;
use std::process::Command;
use std::sync::Once;

static INIT: Once = Once::new();

fn build_docker_image() {
    let output = Command::new("docker")
        .args(&["build", "-t", "deploy-helper-test", "tests/"])
        .output()
        .expect("Failed to build Docker image");
    assert!(
        output.status.success(),
        "Docker build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn start_docker_container() {
    let _ = Command::new("docker")
        .args(&["stop", "ssh_test_server"])
        .output();

    let start_output = Command::new("docker")
        .args(&[
            "run",
            "-d",
            "--rm",
            "-p",
            "2222:22",
            "--name",
            "ssh_test_server",
            "deploy-helper-test",
        ])
        .output()
        .expect("Failed to start Docker container");

    assert!(
        start_output.status.success(),
        "Docker run failed:\n{}",
        String::from_utf8_lossy(&start_output.stderr)
    );
}

fn run_test(yml_file: &str, should_fail: bool, extra_vars: &[&str], inventory_file: &str) {
    run_test_with_flags(yml_file, should_fail, extra_vars, inventory_file, &[], None);
}

// Builds a `cargo run --quiet -- <yml_file> --inventory <inventory>` command
// without stdin/stdout redirection so the process inherits the ConPTY console,
// which is required for TTY-prompt tests (rpassword reads from CONIN$).
fn pty_command(yml_file: &str, inventory_file: &str) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "run",
        "--quiet",
        "--",
        yml_file,
        "--inventory",
        inventory_file,
    ]);
    cmd
}

// Polls until the session's process exits or the deadline is exceeded.
fn wait_for_exit<P>(p: &Session<P, P::Stream>, timeout_secs: u64)
where
    P: expectrl::process::Process + Healthcheck,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "process did not exit within {}s",
            timeout_secs
        );
        if !p.is_alive().unwrap_or(true) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn run_test_with_flags(
    yml_file: &str,
    should_fail: bool,
    extra_vars: &[&str],
    inventory_file: &str,
    extra_flags: &[&str],
    stdin_input: Option<&str>,
) {
    let mut args: Vec<String> = vec!["run".into(), "--quiet".into(), "--".into(), yml_file.into()];
    for ev in extra_vars {
        args.push("--extra-vars".into());
        args.push((*ev).into());
    }
    args.push("--inventory".into());
    args.push(inventory_file.into());
    for f in extra_flags {
        args.push((*f).into());
    }

    let mut cmd = Command::new("cargo");
    cmd.args(args.iter().map(|s| s.as_str()));
    if stdin_input.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().expect("Failed to spawn cargo");
    if let Some(input) = stdin_input {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(input.as_bytes()).expect("write stdin");
    }
    let output = child.wait_with_output().expect("Failed to wait on cargo");

    if should_fail {
        assert!(output.status.code().unwrap() != 0);
    } else {
        assert!(
            output.status.success(),
            "non-zero exit: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let full_output = format!("{}{}", stdout, stderr);

    let expected_output =
        fs::read_to_string(&format!("{}.out", yml_file)).expect("Failed to read expected output");
    assert_eq!(full_output, expected_output);
}

fn setup() {
    INIT.call_once(|| {
        build_docker_image();
        start_docker_container();
        std::thread::sleep(std::time::Duration::from_secs(3));
    });
}

fn run_tests_for_both_inventories(yml_file: &str, should_fail: bool, extra_vars: &[&str]) {
    run_test(yml_file, should_fail, extra_vars, "tests/servers/local.yml");
    run_test(
        yml_file,
        should_fail,
        extra_vars,
        "tests/servers/remote.yml",
    );
}

fn run_test_with_flags_both_inventories(
    yml_file: &str,
    should_fail: bool,
    extra_vars: &[&str],
    extra_flags: &[&str],
    stdin_input: Option<&str>,
) {
    run_test_with_flags(
        yml_file,
        should_fail,
        extra_vars,
        "tests/servers/local.yml",
        extra_flags,
        stdin_input,
    );
    run_test_with_flags(
        yml_file,
        should_fail,
        extra_vars,
        "tests/servers/remote.yml",
        extra_flags,
        stdin_input,
    );
}

fn run_test_check<F>(
    yml_file: &str,
    should_fail: bool,
    extra_vars: &[&str],
    inventory_file: &str,
    check: F,
) where
    F: Fn(&str),
{
    run_test_check_with_flags(
        yml_file,
        should_fail,
        extra_vars,
        inventory_file,
        &[],
        check,
    );
}

fn run_test_check_with_flags<F>(
    yml_file: &str,
    should_fail: bool,
    extra_vars: &[&str],
    inventory_file: &str,
    extra_flags: &[&str],
    check: F,
) where
    F: Fn(&str),
{
    let mut args: Vec<String> = vec!["run".into(), "--quiet".into(), "--".into(), yml_file.into()];
    for ev in extra_vars {
        args.push("--extra-vars".into());
        args.push((*ev).into());
    }
    args.push("--inventory".into());
    args.push(inventory_file.into());
    for flag in extra_flags {
        args.push((*flag).into());
    }

    let output = Command::new("cargo")
        .args(args.iter().map(|s| s.as_str()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to spawn cargo");

    if should_fail {
        assert!(
            output.status.code().unwrap() != 0,
            "expected failure but command succeeded"
        );
    } else {
        assert!(
            output.status.success(),
            "non-zero exit\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let full_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    check(&full_output);
}

#[cfg(test)]
mod verify_tests {
    use super::*;

    #[test]
    fn verify_retries_matches_and_registers_output() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-success.yml",
                false,
                &[],
                inventory,
                |output| {
                    assert_eq!(
                        output.matches("Verification attempt").count(),
                        2,
                        "verify should retry twice before succeeding:\n{}",
                        output
                    );
                    assert!(
                        output.contains("Registering output to: verify_result")
                            && output.contains("healthy-ready"),
                        "verify should register the final matching output:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_reports_expected_and_actual_output() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-mismatch-error.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains("failed after 2 attempts")
                            && output.contains("expected stdout 'healthy', got 'starting'"),
                        "verify should report the final mismatch:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_reports_command_exit_and_stderr() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-command-error.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains("command exited with status 7: not-ready"),
                        "verify should report the command failure:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_stops_before_retrying_past_the_elapsed_time_limit() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-time-limit-error.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains("failed after 1 attempt")
                            && output.contains(
                                "elapsed time limit of 1 second prevented further retries"
                            )
                            && !output.contains("Verification attempt"),
                        "verify should stop before a retry beyond its elapsed time limit:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_allows_a_running_attempt_to_cross_the_elapsed_time_limit() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-running-attempt-crosses-time-limit.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains("failed after 1 attempt")
                            && output.contains(
                                "elapsed time limit of 1 second prevented further retries"
                            )
                            && !output.contains("Verification attempt"),
                        "verify should let a running attempt finish, then stop retrying:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_no_log_hides_command_and_failure_detail() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/verify/verify-no-log-error.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(output.contains("details hidden by no_log"));
                    assert!(
                        !output.contains("VERIFY_SUPER_SECRET"),
                        "no_log should hide verify command and output:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn verify_honors_become_environment_and_login_shell() {
        setup();
        run_test_check(
            "test-ymls/verify/verify-context.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("Preserve environment through privilege escalation")
                        && output.contains("Run through an interactive login shell"),
                    "verify should succeed with become, environment, and login_shell:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn verify_removes_password_authenticated_doas_prompt() {
        setup();
        run_test_check(
            "test-ymls/verify/verify-doas-with-password.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("Registering output to: verify_doas")
                        && output.contains("  root"),
                    "verify should match and register clean doas output:\n{}",
                    output
                );
                assert!(
                    !output.contains("password:"),
                    "verify should not register the doas prompt:\n{}",
                    output
                );
            },
        );
    }
}

mod vars {
    use super::*;

    #[cfg(unix)]
    fn run_with_fake_sops(yml_file: &str, extra_flags: &[&str]) -> std::process::Output {
        use std::os::unix::fs::PermissionsExt;

        let fake_dir = std::path::Path::new("target").join(format!(
            "deploy-helper-fake-sops-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("vars")
                .replace(':', "-")
        ));
        fs::create_dir_all(&fake_dir).unwrap();
        let fake_sops = fake_dir.join("sops");
        fs::write(
            &fake_sops,
            "#!/bin/sh\n[ \"$#\" -eq 2 ] && [ \"$1\" = -d ] || exit 64\n[ \"$SOPS_DISABLE_VERSION_CHECK\" = 1 ] || exit 65\nif grep -q '^TEST_SOPS_FAIL$' \"$2\"; then echo 'fixture decryption failed' >&2; exit 23; fi\nsed 's/^TEST_ENCRYPTED://' \"$2\"\n",
        )
        .unwrap();
        fs::set_permissions(&fake_sops, fs::Permissions::from_mode(0o755)).unwrap();

        let mut path = fake_dir.as_os_str().to_os_string();
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());

        let mut command = Command::new("cargo");
        command.args([
            "run",
            "--quiet",
            "--",
            yml_file,
            "--inventory",
            "tests/servers/local.yml",
        ]);
        command.args(extra_flags);
        command.env("PATH", path);
        let output = command.output().expect("Failed to run vars_files test");
        fs::remove_dir_all(fake_dir).unwrap();
        output
    }

    #[cfg(unix)]
    #[test]
    fn sops_vars_files_load_for_runs_and_task_listing() {
        for flags in [Vec::new(), vec!["--list-tasks"]] {
            let output = run_with_fake_sops("test-ymls/vars/sops-vars-file.yml", &flags);
            assert!(
                output.status.success(),
                "vars_files run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("Starting deployment: SOPS deployment"),
                "deployment vars were not loaded:\n{}",
                stdout
            );
            assert!(
                stdout.contains("Use loaded-secret with play-value"),
                "task vars were not rendered:\n{}",
                stdout
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn extra_vars_override_vars_files_for_runs_and_task_listing() {
        for flags in [
            vec!["--extra-vars", "deployment_name=CLI secret_value=cli"],
            vec![
                "--extra-vars",
                "deployment_name=CLI secret_value=cli",
                "--list-tasks",
            ],
        ] {
            let output = run_with_fake_sops("test-ymls/vars/sops-vars-file.yml", &flags);
            assert!(
                output.status.success(),
                "vars_files override run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("Starting deployment: CLI"), "{stdout}");
            assert!(
                stdout.contains("Use cli with play-value"),
                "extra vars should override vars_files:\n{stdout}"
            );
            assert!(!stdout.contains("SOPS deployment"), "{stdout}");
            assert!(!stdout.contains("Use loaded-secret"), "{stdout}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn extra_vars_stay_in_scope_while_loading_vars_files() {
        for flags in [
            vec![
                "--extra-vars",
                "deployment_name=CLI selected_vars_file=secrets-override.enc.yml",
            ],
            vec![
                "--extra-vars",
                "deployment_name=CLI selected_vars_file=secrets-override.enc.yml",
                "--list-tasks",
            ],
        ] {
            let output = run_with_fake_sops("test-ymls/vars/extra-vars-vars-file-src.yml", &flags);
            assert!(
                output.status.success(),
                "variable-backed vars_files source failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("Starting deployment: CLI"), "{stdout}");
            assert!(stdout.contains("Use loaded-secret"), "{stdout}");
            assert!(!stdout.contains("missing.enc.yml"), "{stdout}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn sops_vars_files_report_provider_errors_without_decrypted_output() {
        let output = run_with_fake_sops("test-ymls/vars/sops-vars-file-error.yml", &[]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("sops decryption failed"));
        assert!(stderr.contains("secrets-fail.enc.yml"));
        assert!(stderr.contains("exit status 23"));
        assert!(stderr.contains("fixture decryption failed"));
        assert!(!stderr.contains("TEST_SOPS_FAIL"));
    }

    #[cfg(unix)]
    #[test]
    fn sops_vars_files_do_not_leak_values_in_template_errors() {
        let output = run_with_fake_sops("test-ymls/vars/sops-vars-file-missing-var-error.yml", &[]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Available vars (values redacted):"));
        for key in [
            "deployment_name",
            "secret_value",
            "secret_group",
            "credentials",
            "username",
            "password",
            "tokens",
        ] {
            assert!(
                stderr.contains(key),
                "missing variable structure key: {key}"
            );
        }
        assert!(stderr.contains("<redacted>"));
        for value in [
            "SOPS deployment",
            "first-secret",
            "from-sops",
            "nested-user",
            "nested-secret",
            "first-token",
            "second-token",
        ] {
            assert!(!stderr.contains(value), "secret value leaked: {value}");
        }
    }

    #[test]
    fn setting_and_debugging_vars() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/setting-and-debugging-vars.yml", false, &[]);
    }

    #[test]
    fn use_vars_in_command_and_shell() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/use-vars-in-command-and-shell.yml",
            false,
            &[],
        );
    }

    #[test]
    fn nested_json_parsing() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/nested-json-parsing.yml", false, &[]);
    }

    #[test]
    fn nested_json_parsing_missing_property_error() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/nested-json-parsing-missing-property-error.yml",
            true,
            &[],
        );
    }

    #[test]
    fn structured_vars_support_parameterized_loops() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/vars/structured-vars.yml",
                false,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains(
                            "database scripts/production-database.py /tmp/database.py True"
                        ) && output
                            .contains("uploads scripts/production-uploads.py /tmp/uploads.py True")
                            && output.contains("parsed json-database")
                            && output.contains("parsed json-uploads"),
                        "structured vars should remain addressable inside the loop:\n{output}"
                    );
                },
            );
        }

        run_test_check_with_flags(
            "test-ymls/vars/structured-vars.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks"],
            |output| {
                assert!(
                    output.contains("Install helpers for pizen"),
                    "list-tasks should resolve structured include vars:\n{output}"
                );
            },
        );
    }

    #[test]
    fn loop_variable_must_resolve_to_a_list() {
        run_test_check(
            "test-ymls/vars/structured-vars-invalid-loop.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("loop must be a list or an expression that resolves to a list"),
                    "invalid loop values should have a clear error:\n{output}"
                );
                assert!(!output.contains("> echo unreachable"));
            },
        );
    }

    #[test]
    fn missing_var_error() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/missing-var-error.yml", true, &[]);
    }

    #[test]
    fn invalid_json_error() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/invalid-json-error.yml", true, &[]);
    }

    #[test]
    fn env_output_parsing() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/vars/env-output-parsing.yml",
                false,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains(
                            "database_sha256=abc123 files=42 url=https://example.com?a=b empty="
                        ),
                        "from_env should expose parsed fields:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn invalid_env_output_error() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check(
                "test-ymls/vars/invalid-env-output-error.yml",
                true,
                &[],
                inventory,
                |output| {
                    assert!(
                        output.contains("Error parsing env: line 2 must be KEY=VALUE"),
                        "from_env should explain malformed lines:\n{}",
                        output
                    );
                    assert!(
                        !output.contains("top-secret-value"),
                        "from_env should not expose malformed secret output:\n{}",
                        output
                    );
                },
            );
        }
    }

    #[test]
    fn extra_vars() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/extra-vars.yml", false, &["cat=1 bat=2"]);
        run_tests_for_both_inventories(
            "test-ymls/vars/extra-vars.yml",
            false,
            &["{ \"cat\": 1, \"bat\": 2 }"],
        );
        run_tests_for_both_inventories(
            "test-ymls/vars/extra-vars.yml",
            false,
            &["@test-ymls/vars/extra-vars.vars.yml"],
        );
    }

    #[test]
    fn extra_vars_multiple_e() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/extra-vars.yml",
            false,
            &[
                "@test-ymls/vars/extra-vars-multi-e.vars1.yml",
                "@test-ymls/vars/extra-vars-multi-e.vars2.yml",
            ],
        );
    }

    #[test]
    fn extra_vars_later_overrides_earlier() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/extra-vars.yml",
            false,
            &["cat=wrong bat=2", "cat=1"],
        );
    }

    #[test]
    fn extra_vars_override_deployment_vars_for_runs_and_task_listing() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            for flags in [Vec::new(), vec!["--list-tasks"]] {
                run_test_check_with_flags(
                    "test-ymls/vars/extra-vars-precedence.yml",
                    false,
                    &["deployment_name=CLI", "selected_value=cli"],
                    inventory,
                    &flags,
                    |output| {
                        assert!(output.contains("Starting deployment: CLI"), "{output}");
                        assert!(
                            output.contains("Use cli and cli-derived"),
                            "deployment vars should render from the CLI override:\n{output}"
                        );
                        assert!(!output.contains("Deployment default"), "{output}");
                        assert!(!output.contains("Use deployment"), "{output}");
                    },
                );
            }
        }
    }

    #[test]
    fn when_condition() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/when-condition.yml",
            false,
            &["condition=true"],
        );
    }

    #[test]
    fn undefined_when_comparison_fails_closed() {
        run_test_check(
            "test-ymls/vars/when-undefined-comparison.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("when condition failed: undefined value")
                        && output.contains("when: missing_value != \"\""),
                    "undefined-value error should identify the condition:\n{output}"
                );
                assert!(!output.contains("Executing task: Must not run"));
            },
        );
    }

    #[test]
    fn undefined_when_value_is_an_error() {
        run_test_check(
            "test-ymls/vars/when-undefined-value.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("when condition failed: undefined value")
                        && output.contains("when: missing_value"),
                    "undefined-value error should identify the condition:\n{output}"
                );
                assert!(!output.contains("Executing task: Must not run"));
            },
        );
    }

    #[test]
    fn undefined_when_comparison_honors_no_log() {
        run_test_check(
            "test-ymls/vars/when-undefined-comparison-no-log.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("when condition failed (details hidden by no_log)"),
                    "undefined-value error should be hidden:\n{output}"
                );
                assert!(
                    !output.contains("secret-only-in-when"),
                    "no_log should hide the condition:\n{output}"
                );
                assert!(!output.contains("Executing task: Must not run"));
            },
        );
    }

    #[test]
    fn run_level_vars() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/run-level-vars.yml", false, &[]);
    }

    #[test]
    fn use_vars_in_chdir() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/use-vars-in-chdir.yml", false, &[]);
    }

    #[test]
    fn use_vars_in_task_name() {
        setup();
        run_tests_for_both_inventories("test-ymls/vars/use-vars-in-task-name.yml", false, &[]);
    }

    #[test]
    fn use_vars_in_run_name() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/use-vars-in-run-name.yml",
            false,
            &["@test-ymls/vars/use-vars-in-run-name.vars.yml"],
        );
    }

    #[test]
    fn servers_yml_var_support() {
        setup();
        run_test(
            "test-ymls/vars/setting-and-debugging-vars.yml",
            false,
            &["test_host=localhost"],
            "tests/servers/local-templated.yml",
        );
    }

    #[test]
    fn servers_yml_var_support_remote_fields() {
        setup();
        run_test(
            "test-ymls/vars/setting-and-debugging-vars.yml",
            false,
            &[
                "remote_host=localhost",
                "remote_user=root",
                "remote_password=password",
            ],
            "tests/servers/remote-templated.yml",
        );
    }

    #[test]
    fn set_and_use_vars_immediately_in_shell_and_command() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/vars/set-and-use-vars-immediately-in-shell-and-command.yml",
            false,
            &[],
        );
    }
}

mod shell {
    use super::*;

    #[test]
    fn setting_working_directory_before_running_commands() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/shell/setting-working-directory-before-running-commands.yml",
            false,
            &[],
        );
    }

    #[test]
    fn setting_global_working_directory_before_running_commands() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/shell/setting-global-working-directory-before-running-commands.yml",
            false,
            &[],
        );
    }

    #[test]
    fn dont_run_2nd_deploy_if_1st_fails() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/shell/dont-run-2nd-task-or-2nd-deploy-if-1st-fails.yml",
            true,
            &[],
        );
    }

    #[test]
    fn shell_block_shares_state_across_lines() {
        setup();
        run_tests_for_both_inventories("test-ymls/shell/shell-block-shares-state.yml", false, &[]);
    }

    #[test]
    fn use_output_of_one_task_shell_in_another_task_shell() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/shell/use-output-of-one-task-shell-in-another-task-shell.yml",
            false,
            &[],
        );
    }

    #[test]
    fn debug_should_come_before_command_and_shell() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/shell/debug-should-come-before-command-and-shell.yml",
            false,
            &[],
        );
    }

    #[test]
    fn loop_item() {
        setup();
        run_tests_for_both_inventories("test-ymls/shell/loop-item.yml", false, &[]);
    }

    #[test]
    fn include_tasks() {
        setup();
        run_tests_for_both_inventories("test-ymls/shell/include-tasks.yml", false, &[]);
    }

    // shell_defaults: extra `set` flags injected ahead of every shell block.
    // All run against localhost since only command construction is at stake.

    #[test]
    fn shell_defaults_make_unset_variable_fatal() {
        run_test_check(
            "test-ymls/shell/shell-defaults-strict.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |_| {},
        );
    }

    #[test]
    fn shell_defaults_task_empty_string_opts_out() {
        run_test_check(
            "test-ymls/shell/shell-defaults-opt-out.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("OPT_OUT_RAN"),
                    "opted-out task should run without set -u:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn shell_defaults_on_a_single_task() {
        run_test_check(
            "test-ymls/shell/shell-defaults-task-level.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |_| {},
        );
    }

    #[test]
    fn shell_defaults_inherited_by_included_tasks() {
        run_test_check(
            "test-ymls/shell/shell-defaults-include.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |_| {},
        );
    }

    #[test]
    fn shell_defaults_include_level_opt_out_covers_included_tasks() {
        run_test_check(
            "test-ymls/shell/shell-defaults-include-opt-out.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("INCLUDE_OPT_OUT_RAN"),
                    "included block should run without the deployment's set -u:\n{}",
                    output
                );
            },
        );
    }
}

mod privilege {
    use super::*;

    #[test]
    fn become_nopasswd() {
        setup();
        run_test(
            "test-ymls/become/become-nopasswd.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
        );
    }

    #[test]
    fn play_level_become_applies_to_tasks() {
        setup();
        run_test_check(
            "test-ymls/become/play-level-become.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("root") && !output.contains("nopass"),
                    "expected whoami to report 'root' (deployment-level become), got:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn play_level_become_inherited_by_included_tasks() {
        setup();
        run_test_check(
            "test-ymls/become/play-level-become-include.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("root") && !output.contains("nopass"),
                    "expected included task's whoami to report 'root' (deployment become inherited through include_tasks), got:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn play_level_become_method_applies_to_tasks() {
        setup();
        run_test_check(
            "test-ymls/become/play-level-become-method.yml",
            false,
            &[],
            "tests/servers/become-doas.yml",
            |output| {
                assert!(
                    output.contains("root") && !output.contains("doasuser"),
                    "expected whoami to report 'root' via deployment-level become_method: doas, got:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn play_level_become_can_be_overridden_by_task() {
        setup();
        run_test_check(
            "test-ymls/become/play-level-become-task-opt-out.yml",
            false,
            &[],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("nopass") && !output.contains("root"),
                    "expected whoami to report 'nopass' (task became: false overrides deployment become), got:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn become_with_password() {
        setup();
        run_test(
            "test-ymls/become/become-with-password.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-withpass.yml",
        );
    }

    #[test]
    fn become_su_nopasswd() {
        setup();
        run_test(
            "test-ymls/become/become-su-nopasswd.yml",
            false,
            &["become_password="],
            "tests/servers/become-root.yml",
        );
    }

    #[test]
    fn become_invalid_method_error() {
        setup();
        run_test(
            "test-ymls/become/become-invalid-method-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
        );
    }

    #[test]
    fn become_su_with_password() {
        setup();
        run_test(
            "test-ymls/become/become-su-with-password.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-withpass.yml",
        );
    }

    #[test]
    fn become_doas() {
        setup();
        run_test(
            "test-ymls/become/become-doas.yml",
            false,
            &[],
            "tests/servers/become-doas.yml",
        );
    }

    #[test]
    fn become_doas_with_password() {
        setup();
        run_test_check(
            "test-ymls/become/become-doas-with-password.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("root"),
                    "expected whoami output 'root' in:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn command_with_become_doas() {
        setup();
        run_test_check(
            "test-ymls/become/command-with-become-doas.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("root"),
                    "expected whoami output 'root' in:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn become_password_prompted_via_tty() {
        setup();
        let mut p = Session::spawn(pty_command(
            "test-ymls/become/become-with-password.yml",
            "tests/servers/become-withpass.yml",
        ))
        .expect("spawn PTY session");
        // The trailing space in "BECOME password: " is re-encoded as ESC[1C by
        // the ConHost, so match without it.
        p.expect("BECOME password:")
            .expect("password prompt appeared on TTY");
        p.send_line("password").expect("send password");
        wait_for_exit(&p, 30);
    }
}

mod file_ops {
    use super::*;

    #[test]
    fn copy_content_basic() {
        setup();
        run_tests_for_both_inventories("test-ymls/file-ops/copy-content-basic.yml", false, &[]);
    }

    #[test]
    fn template_basic() {
        setup();
        run_tests_for_both_inventories("test-ymls/file-ops/template-basic.yml", false, &[]);
    }

    #[test]
    fn copy_with_src() {
        setup();
        run_tests_for_both_inventories("test-ymls/file-ops/copy-with-src.yml", false, &[]);
    }

    #[test]
    fn copy_dir_recursive() {
        setup();
        run_tests_for_both_inventories("test-ymls/file-ops/copy-dir-basic.yml", false, &[]);
    }

    // Overlay semantics: a directory copy overwrites matching files, adds new ones,
    // and leaves unrelated files (top-level and nested) untouched. Runs on localhost
    // only, so no Docker/SSH needed.
    #[test]
    fn copy_dir_overlay_preserves_existing_files() {
        run_test_check(
            "test-ymls/file-ops/copy-dir-overlay.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("KEEP_ME"),
                    "unrelated top-level file should be preserved by the overlay copy:\n{}",
                    output
                );
                assert!(
                    output.contains("KEEP_DEEP"),
                    "unrelated nested file should be preserved by the overlay copy:\n{}",
                    output
                );
                // OLD_ALPHA appears once in the seed echo; if the copy failed to
                // overwrite alpha.txt, reading it back would print OLD_ALPHA a 2nd time.
                assert!(
                    output.matches("OLD_ALPHA").count() == 1,
                    "alpha.txt should have been overwritten by the overlay copy:\n{}",
                    output
                );
            },
        );
    }

    // A directory copy under `become: true` (sudo, nopasswd). Covers the non-doas
    // become branch of the dir-skeleton mkdir: the nested dir must be created as root,
    // which `stat` confirms. Runs against the remote container.
    #[test]
    fn copy_dir_with_become() {
        setup();
        run_test_check(
            "test-ymls/file-ops/copy-dir-become.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("root"),
                    "files and dirs copied via become should be owned by root:\n{}",
                    output
                );
                assert!(
                    output.contains("gamma"),
                    "nested file copied via become should be readable:\n{}",
                    output
                );
            },
        );
    }

    // A directory copy under `become_method: doas` with a password. The mkdir step
    // for the dir skeleton must go through the doas PTY just like the file writes, so
    // the created dirs are root-owned. `stat` on the nested dir proves it was made as
    // root (not the login user), which would fail if the mkdir skipped doas.
    #[test]
    fn copy_dir_with_become_doas() {
        setup();
        run_test_check(
            "test-ymls/file-ops/copy-dir-become-doas.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("root"),
                    "files and dirs copied via doas should be owned by root:\n{}",
                    output
                );
                assert!(
                    output.contains("gamma"),
                    "nested file copied via doas should be readable:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn copy_both_src_and_content_error() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/file-ops/copy-both-src-and-content-error.yml",
            true,
            &[],
        );
    }

    #[test]
    fn copy_neither_src_nor_content_error() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/file-ops/copy-neither-src-nor-content-error.yml",
            true,
            &[],
        );
    }

    #[test]
    fn template_missing_src_error() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/file-ops/template-missing-src-error.yml",
            true,
            &[],
        );
    }

    #[test]
    fn copy_missing_src_error() {
        setup();
        run_tests_for_both_inventories("test-ymls/file-ops/copy-missing-src-error.yml", true, &[]);
    }

    #[test]
    fn template_with_become() {
        setup();
        run_test(
            "test-ymls/file-ops/template-with-become.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
        );
    }

    #[test]
    fn template_with_become_doas() {
        setup();
        run_test_check(
            "test-ymls/file-ops/template-with-become-doas.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("root"),
                    "expected 'root' as file owner in:\n{}",
                    output
                );
                assert!(
                    output.contains("owner=root"),
                    "expected template content 'owner=root' in:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn template_vars_in_src_and_dest() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/file-ops/template-vars-in-src-and-dest.yml",
            false,
            &[],
        );
    }

    #[test]
    fn copy_content_preserves_whitespace() {
        setup();
        run_tests_for_both_inventories(
            "test-ymls/file-ops/copy-content-preserves-whitespace.yml",
            false,
            &[],
        );
    }

    // mode: on copy/template lands the file with the exact permissions and no
    // window where it is readable beyond them. Verified on the container since
    // Windows-local chmod is a no-op.
    #[test]
    fn copy_mode_sets_permissions() {
        setup();
        run_test_check(
            "test-ymls/file-ops/copy-mode.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("PERMS=600"),
                    "copied file should have mode 600:\n{}",
                    output
                );
                assert!(
                    output.contains("TMP_GONE"),
                    "staging temp file should not be left behind:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn template_mode_sets_permissions() {
        setup();
        run_test_check(
            "test-ymls/file-ops/template-mode.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("PERMS=640"),
                    "templated file should have mode 640:\n{}",
                    output
                );
                assert!(
                    output.contains("token=template-mode-value"),
                    "templated content should be rendered:\n{}",
                    output
                );
            },
        );
    }

    // The become path stages via /tmp and places the file as root; mode must
    // survive that route too.
    #[test]
    fn copy_mode_with_become() {
        setup();
        run_test_check(
            "test-ymls/file-ops/copy-mode-become.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("PERMS=600") && output.contains("OWNER=root"),
                    "file written via become should be root-owned with mode 600:\n{}",
                    output
                );
            },
        );
    }

    // Same through the doas PTY placement path.
    #[test]
    fn copy_mode_with_become_doas() {
        setup();
        run_test_check(
            "test-ymls/file-ops/copy-mode-become-doas.yml",
            false,
            &["become_password=password"],
            "tests/servers/become-doas-withpass.yml",
            |output| {
                assert!(
                    output.contains("PERMS=600") && output.contains("OWNER=root"),
                    "file written via doas should be root-owned with mode 600:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn file_directory_creates_with_mode_and_owner() {
        setup();
        run_test_check(
            "test-ymls/file-ops/file-directory.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("DIR1=700"),
                    "nested directory should have mode 700:\n{}",
                    output
                );
                assert!(
                    output.contains("DIR2=750:nopass:nopass"),
                    "owned directory should have mode 750 and nopass:nopass ownership:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn copy_mode_invalid_error() {
        run_test_check(
            "test-ymls/file-ops/copy-mode-invalid-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("invalid mode"),
                    "non-octal mode should be rejected:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn copy_mode_unquoted_number_error() {
        run_test_check(
            "test-ymls/file-ops/copy-mode-unquoted-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("quoted"),
                    "a numeric mode should fail with a hint to quote it:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn copy_dir_with_mode_error() {
        run_test_check(
            "test-ymls/file-ops/copy-dir-mode-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("not supported when src is a directory"),
                    "mode with a directory src should be rejected:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn file_unsupported_state_error() {
        run_test_check(
            "test-ymls/file-ops/file-state-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("supports only state: directory"),
                    "non-directory state should be rejected:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn env_file_merges_values_and_sops_secrets() {
        setup();
        run_test_check(
            "test-ymls/file-ops/env-file.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("PERMS=600") && output.contains("ENV_FILE_OK"),
                    "env_file should merge overlays and install mode 600:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn env_file_rejects_values_secrets_collision() {
        setup();
        run_test_check(
            "test-ymls/file-ops/env-file-collision-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("key is defined in both values and secrets: SHARED"),
                    "env_file should name the conflicting key:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn env_file_sops_failure_preserves_destination_and_cleans_temps() {
        setup();
        run_test_check(
            "test-ymls/file-ops/env-file-sops-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("sops decryption failed: secrets.enc.env"),
                    "env_file should report the failed source without its content:\n{}",
                    output
                );
            },
        );

        let verification = Command::new("docker")
            .args([
                "exec",
                "ssh_test_server",
                "sh",
                "-c",
                "cd /tmp/deploy-helper-test-env-file-sops-error && grep -qx 'EXISTING=preserved' .env && ! find . -maxdepth 1 -name '.env.deploy-helper-*' | grep -q .",
            ])
            .output()
            .expect("Failed to verify the SOPS failure state");
        assert!(
            verification.status.success(),
            "SOPS failure should preserve .env and remove temporary files: {}",
            String::from_utf8_lossy(&verification.stderr)
        );
    }

    #[test]
    fn env_file_rejects_empty_sops_output() {
        setup();
        run_test_check(
            "test-ymls/file-ops/env-file-empty-sops.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("decrypted secrets contain no dotenv entries"),
                    "env_file should reject empty decrypted secrets:\n{}",
                    output
                );
            },
        );

        let verification = Command::new("docker")
            .args([
                "exec",
                "ssh_test_server",
                "sh",
                "-c",
                "cd /tmp/deploy-helper-test-env-file-empty-sops && grep -qx 'EXISTING=preserved' .env && ! find . -maxdepth 1 -name '.env.deploy-helper-*' | grep -q .",
            ])
            .output()
            .expect("Failed to verify the empty SOPS state");
        assert!(
            verification.status.success(),
            "Empty SOPS output should preserve .env and remove temporary files: {}",
            String::from_utf8_lossy(&verification.stderr)
        );
    }

    #[test]
    fn env_file_without_secrets_replaces_defaults() {
        setup();
        run_test_check(
            "test-ymls/file-ops/env-file-no-secrets.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("NO_SECRETS_PERMS=600"),
                    "env_file should support a values-only overlay:\n{}",
                    output
                );
            },
        );
    }
}

mod systemd {
    use super::*;

    #[test]
    fn systemd_manages_and_verifies_units() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("SYSTEMD_OK"),
                    "systemd should perform every requested operation:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn systemd_reports_unexpected_unit_result() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd-result-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains(
                        "systemd unit failed.service result: expected success, got failed"
                    ),
                    "systemd should report the expected and actual result:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn systemd_reports_inactive_unit() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd-active-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("systemd unit inactive.unit is not active"),
                    "systemd should identify the inactive unit:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn systemd_reports_disabled_unit() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd-enabled-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("systemd unit disabled.unit is not enabled"),
                    "systemd should identify the disabled unit:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn systemd_rejects_missing_unit_when_disabling() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd-missing-disable-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains(
                        "systemd failed to determine enabled state for unit missing.unit"
                    ),
                    "systemd should reject an enablement inspection error:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn systemd_rejects_static_unit_as_enabled() {
        setup();
        run_test_check(
            "test-ymls/systemd/systemd-static-enabled-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    output.contains("systemd unit static.unit is not enabled (state: static)"),
                    "systemd should not treat a static unit as enabled:\n{}",
                    output
                );
            },
        );
    }
}

mod tags {
    use super::*;

    #[test]
    fn tags_filter_runs_only_matching() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/tags/tags-filter.yml",
            false,
            &[],
            &["--tags", "build"],
            None,
        );
    }

    #[test]
    fn skip_tags_excludes_matches() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/tags/skip-tags.yml",
            false,
            &[],
            &["--skip-tags", "drop"],
            None,
        );
    }

    #[test]
    fn skip_tags_wins_over_tags_flag() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/tags/tags-and-skip-tags.yml",
            false,
            &[],
            &["--tags", "web", "--skip-tags", "tls"],
            None,
        );
    }

    #[test]
    fn always_tag_bypasses_tags_filter() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/tags/always-tag.yml",
            false,
            &[],
            &["--tags", "tls"],
            None,
        );
    }

    #[test]
    fn never_tag_hidden_by_default() {
        setup();
        run_test_with_flags(
            "test-ymls/tags/never-tag.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &[],
            None,
        );
    }

    #[test]
    fn never_tag_opt_in_via_other_tag() {
        setup();
        run_test_with_flags(
            "test-ymls/tags/never-tag-optin.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--tags", "nuke"],
            None,
        );
    }

    #[test]
    fn tags_inheritance_flows_from_include() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/tags/tags-inheritance.yml",
            false,
            &[],
            &["--tags", "nginx"],
            None,
        );
    }
}

mod execution {
    use super::*;

    #[test]
    fn start_at_task_skips_before_match() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/execution/start-at-task.yml",
            false,
            &[],
            &["--start-at-task", "Second"],
            None,
        );
    }

    #[test]
    fn step_prompt_y_n_c() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/execution/step.yml",
            false,
            &[],
            &["--step"],
            Some("y\nn\nc\n"),
        );
    }

    #[test]
    fn step_prompt_eof_skips_all() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/execution/step-eof.yml",
            false,
            &[],
            &["--step"],
            Some(""),
        );
    }

    #[test]
    fn step_prompt_unknown_reprompts() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/execution/step-reprompt.yml",
            false,
            &[],
            &["--step"],
            Some("?\ny\nn\n"),
        );
    }

    #[test]
    fn list_tasks_prints_tree_with_effective_tags() {
        setup();
        run_test_with_flags_both_inventories(
            "test-ymls/execution/list-tasks.yml",
            false,
            &[],
            &["--list-tasks"],
            None,
        );
    }

    #[test]
    fn list_tasks_respects_tags_filter() {
        setup();
        run_test_with_flags(
            "test-ymls/execution/list-tasks-filtered.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks", "--tags", "extras"],
            None,
        );
    }

    #[test]
    fn list_tasks_renders_names_and_matches_start_at_task() {
        setup();
        run_test_with_flags(
            "test-ymls/execution/list-tasks-templated.yml",
            false,
            &["env=prod"],
            "tests/servers/local.yml",
            &["--list-tasks", "--start-at-task", "Deploy prod"],
            None,
        );
    }

    #[test]
    fn list_tasks_templates_deployment_vars_in_names() {
        setup();
        run_test_with_flags(
            "test-ymls/execution/list-tasks-dep-vars.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks"],
            None,
        );
    }

    #[test]
    fn list_tasks_applies_include_vars_to_nested_tasks() {
        run_test_check_with_flags(
            "test-ymls/execution/list-tasks-include-vars.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks"],
            |output| {
                for task_name in [
                    "Deploy production-api",
                    "Verify production-api health",
                    "Report production-api",
                    "Parse runtime output",
                ] {
                    assert!(
                        output.contains(task_name),
                        "missing rendered included task '{task_name}':\n{output}"
                    );
                }
            },
        );
    }

    #[test]
    fn list_tasks_hides_no_log_variable_errors() {
        run_test_check_with_flags(
            "test-ymls/execution/list-tasks-no-log-vars.yml",
            true,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks"],
            |output| {
                assert!(
                    output.contains("template value resolution failed (details hidden by no_log)"),
                    "{}",
                    output
                );
                assert!(
                    !output.contains("secret-list-tasks-value-that-is-not-json"),
                    "{}",
                    output
                );
            },
        );
    }
}

mod recovery {
    use super::*;

    #[test]
    fn ansible_rescue_key_is_rejected() {
        run_test_check(
            "test-ymls/recovery/rescue-key-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("unknown field `rescue`"),
                    "old rescue key should be rejected:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn list_tasks_marks_on_failure_and_always_tasks() {
        run_test_check_with_flags(
            "test-ymls/recovery/list-tasks.yml",
            false,
            &[],
            "tests/servers/local.yml",
            &["--list-tasks"],
            |output| {
                assert!(
                    output.contains("[on_failure] Restore previous application  TAGS: [always]"),
                    "{}",
                    output
                );
                assert!(
                    output.contains("[always] Remove temporary files            TAGS: [always]"),
                    "{}",
                    output
                );
            },
        );
    }

    #[test]
    fn on_failure_runs_after_failure_and_always_runs_after_it() {
        setup();
        run_test_check_with_flags(
            "test-ymls/recovery/on-failure-and-always.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            &["--tags", "restore"],
            |output| {
                assert!(output.contains("Running on_failure tasks:"), "{}", output);
                assert!(output.contains("on_failure_ran"), "{}", output);
                assert!(output.contains("Running always tasks:"), "{}", output);
                assert!(output.contains("always_ran"), "{}", output);
                assert!(!output.contains("main_after_failure"), "{}", output);
                assert!(output.contains("exit status: 7"), "{}", output);
            },
        );
    }

    #[test]
    fn on_failure_is_skipped_after_success_and_always_still_runs() {
        setup();
        run_test_check(
            "test-ymls/recovery/success.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(output.contains("main_succeeded"), "{}", output);
                assert!(!output.contains("Running on_failure tasks:"), "{}", output);
                assert!(!output.contains("on_failure_should_not_run"), "{}", output);
                assert!(output.contains("always_after_success"), "{}", output);
            },
        );
    }

    #[test]
    fn on_failure_error_preserves_main_error_and_still_runs_always() {
        setup();
        run_test_check(
            "test-ymls/recovery/on-failure-error.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(output.contains("on_failure_started"), "{}", output);
                assert!(!output.contains("on_failure_after_error"), "{}", output);
                assert!(
                    output.contains("always_after_on_failure_error"),
                    "{}",
                    output
                );
                assert!(output.contains("main tasks failed:"), "{}", output);
                assert!(output.contains("on_failure tasks failed:"), "{}", output);
                assert!(output.contains("always tasks failed:"), "{}", output);
                assert!(output.contains("exit status: 2"), "{}", output);
                assert!(output.contains("exit status: 3"), "{}", output);
                assert!(output.contains("exit status: 4"), "{}", output);
            },
        );
    }

    #[test]
    fn always_failure_fails_an_otherwise_successful_deployment() {
        setup();
        run_test_check(
            "test-ymls/recovery/always-failure.yml",
            true,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(output.contains("main_succeeded"), "{}", output);
                assert!(!output.contains("on_failure_should_not_run"), "{}", output);
                assert!(output.contains("exit status: 4"), "{}", output);
            },
        );
    }

    #[test]
    fn loop_resolution_failure_honors_recovery_and_no_log() {
        run_test_check(
            "test-ymls/recovery/loop-resolution-failure.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("template value resolution failed (details hidden by no_log)"),
                    "{}",
                    output
                );
                assert!(output.contains("loop_on_failure_ran"), "{}", output);
                assert!(output.contains("loop_always_ran"), "{}", output);
                assert!(!output.contains("loop_task_should_not_run"), "{}", output);
                assert!(
                    !output.contains("secret-helper-value-that-is-not-json"),
                    "{}",
                    output
                );
            },
        );
    }

    #[test]
    fn task_vars_resolution_failure_honors_recovery_and_no_log() {
        run_test_check(
            "test-ymls/recovery/task-vars-resolution-failure.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("template value resolution failed (details hidden by no_log)"),
                    "{}",
                    output
                );
                assert!(output.contains("task_vars_on_failure_ran"), "{}", output);
                assert!(output.contains("task_vars_always_ran"), "{}", output);
                assert!(
                    !output.contains("task_vars_task_should_not_run"),
                    "{}",
                    output
                );
                assert!(
                    !output.contains("secret-task-vars-value-that-is-not-json"),
                    "{}",
                    output
                );
            },
        );
    }
}

// environment: at play and task level - exported for shell: blocks and
// command: lines without appearing in any echoed output.
mod environment {
    use super::*;

    #[test]
    fn play_and_task_environment_merge() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check("test-ymls/environment/env-shell.yml", false, &[], inventory, |output| {
                assert!(
                    output.contains("play=play-value shared=from-play"),
                    "play-level environment should reach the first block:\n{}",
                    output
                );
                assert!(
                    output.contains("play=play-value shared=from-task task=task-value"),
                    "task entries should merge over the play map:\n{}",
                    output
                );
            });
        }
    }

    #[test]
    fn command_task_sees_environment() {
        setup();
        for inventory in ["tests/servers/local.yml", "tests/servers/remote.yml"] {
            run_test_check("test-ymls/environment/env-command.yml", false, &[], inventory, |output| {
                assert!(
                    output.contains("command-value"),
                    "command: should see the environment on {}:\n{}",
                    inventory,
                    output
                );
            });
        }
    }

    #[test]
    fn environment_values_are_templated() {
        run_test_check(
            "test-ymls/environment/env-templated.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("rendered=rendered-secret"),
                    "environment values should render vars:\n{}",
                    output
                );
            },
        );
    }

    // sudo resets the caller's environment, so this proves the exports ride
    // inside the become wrapper rather than on the outer process.
    #[test]
    fn environment_survives_become() {
        setup();
        run_test_check(
            "test-ymls/environment/env-become.yml",
            false,
            &["become_password="],
            "tests/servers/become-nopass.yml",
            |output| {
                assert!(
                    output.contains("user=root env=root-value"),
                    "become shell should see the environment:\n{}",
                    output
                );
                assert!(
                    output.contains("root-cmd-value"),
                    "become command should see the environment:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn environment_inherited_by_included_tasks() {
        run_test_check(
            "test-ymls/environment/env-include.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("inherited=include-value"),
                    "included block should see the play environment:\n{}",
                    output
                );
            },
        );
    }

    // The env export prefix on remote commands is grouped in braces - without
    // that, `cd bad-dir && export ...; cmd` would run cmd despite the failed cd.
    #[test]
    fn environment_does_not_defeat_chdir_guard() {
        setup();
        run_test_check(
            "test-ymls/environment/env-command-chdir-guard.yml",
            true,
            &[],
            "tests/servers/remote.yml",
            |_| {},
        );
    }

    #[test]
    fn environment_key_with_shell_syntax_is_an_error() {
        run_test_check(
            "test-ymls/environment/env-bad-key-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("invalid environment key"),
                    "bad key should be rejected with a clear error:\n{}",
                    output
                );
                assert!(
                    !output.contains("never"),
                    "nothing should run after the key validation fails:\n{}",
                    output
                );
            },
        );
    }
}

// Unknown keys anywhere in a deploy file or inventory are parse errors, so a
// typo like dst: for dest: can't silently do nothing. Localhost only.
mod unknown_keys {
    use super::*;

    #[test]
    fn unknown_task_key_is_an_error() {
        run_test_check(
            "test-ymls/unknown-keys/task-key-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("unknown field `wibblesplat`"),
                    "error should name the unknown task key:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn unknown_play_key_is_an_error() {
        run_test_check(
            "test-ymls/unknown-keys/play-key-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("unknown field `flibbertigibbet`"),
                    "error should name the unknown play key:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn copy_dest_typo_is_an_error_naming_expected_keys() {
        run_test_check(
            "test-ymls/unknown-keys/copy-key-error.yml",
            true,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("unknown field `dst`") && output.contains("dest"),
                    "error should name the typo and the expected keys:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn unknown_inventory_key_is_an_error() {
        run_test_check(
            "test-ymls/unknown-keys/ok.yml",
            true,
            &[],
            "tests/servers/unknown-key.yml",
            |output| {
                assert!(
                    output.contains("unknown field `ssh_keypath`"),
                    "error should name the unknown inventory key:\n{}",
                    output
                );
                assert!(
                    !output.contains("INVENTORY_OK"),
                    "no task should run when the inventory fails to parse:\n{}",
                    output
                );
            },
        );
    }
}

// creates/removes idempotency guards run against localhost, so no Docker/SSH needed.
mod idempotency {
    use super::*;

    #[test]
    fn creates_skips_task_when_path_exists() {
        run_test_check(
            "test-ymls/creates-removes/creates-skip.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    !output.contains("CREATES_MARKER"),
                    "task should have been skipped (creates path exists), but it ran:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn creates_runs_task_when_path_absent() {
        run_test_check(
            "test-ymls/creates-removes/creates-run.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("CREATES_MARKER"),
                    "task should have run (creates path absent), but the marker is missing:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn removes_runs_task_when_path_exists() {
        run_test_check(
            "test-ymls/creates-removes/removes-run.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("REMOVES_MARKER"),
                    "task should have run (removes path exists), but the marker is missing:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn removes_skips_task_when_path_absent() {
        run_test_check(
            "test-ymls/creates-removes/removes-skip.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    !output.contains("REMOVES_MARKER"),
                    "task should have been skipped (removes path absent), but it ran:\n{}",
                    output
                );
            },
        );
    }

    // Exercises path_exists_on_target's remote/SSH branch (Docker container as root).
    #[test]
    fn creates_skips_task_on_remote_target() {
        setup();
        run_test_check(
            "test-ymls/creates-removes/creates-skip-remote.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    !output.contains("REMOTE_CREATES_MARKER"),
                    "task should have been skipped (creates path exists on remote), but it ran:\n{}",
                    output
                );
            },
        );
    }
}

// no_log suppresses a task's command echo and output; run against localhost (no Docker).
mod logging {
    use super::*;

    #[test]
    fn no_log_suppresses_shell_output() {
        run_test_check(
            "test-ymls/no-log/no-log-shell.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    !output.contains("NOLOG_SHELL_SECRET"),
                    "no_log should suppress the shell echo and output, but the secret leaked:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn no_log_suppresses_command_output() {
        run_test_check(
            "test-ymls/no-log/no-log-command.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    !output.contains("NOLOG_COMMAND_SECRET"),
                    "no_log should suppress the command echo and output, but the secret leaked:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn no_log_suppresses_debug_output() {
        run_test_check(
            "test-ymls/no-log/no-log-debug.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    !output.contains("NOLOG_DEBUG_SECRET"),
                    "no_log should suppress debug output, but the secret leaked:\n{}",
                    output
                );
            },
        );
    }

    #[test]
    fn output_shown_without_no_log() {
        run_test_check(
            "test-ymls/no-log/no-log-absent.yml",
            false,
            &[],
            "tests/servers/local.yml",
            |output| {
                assert!(
                    output.contains("NOLOG_VISIBLE_OUTPUT"),
                    "without no_log the command output should be shown, but it is missing:\n{}",
                    output
                );
            },
        );
    }

    // Exercises no_log over the SSH path (display_output suppression in execute_ssh_command).
    #[test]
    fn no_log_suppresses_output_on_remote_target() {
        setup();
        run_test_check(
            "test-ymls/no-log/no-log-shell-remote.yml",
            false,
            &[],
            "tests/servers/remote-ssh.yml",
            |output| {
                assert!(
                    !output.contains("REMOTE_NOLOG_SECRET"),
                    "no_log should suppress shell output over SSH, but the secret leaked:\n{}",
                    output
                );
            },
        );
    }
}
