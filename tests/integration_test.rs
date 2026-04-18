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
    let mut args = vec!["run", "--quiet", "--", yml_file];
    for ev in extra_vars {
        args.push("--extra-vars");
        args.push(ev);
    }
    args.extend_from_slice(&["--inventory", inventory_file]);

    let output = Command::new("cargo")
        .args(&args)
        .output()
        .expect("Failed to execute command");

    if should_fail {
        assert!(output.status.code().unwrap() != 0);
    } else {
        assert!(output.status.success());
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

#[test]
fn setting_and_debugging_vars() {
    setup();
    run_tests_for_both_inventories("test-ymls/setting-and-debugging-vars.yml", false, &[]);
}

#[test]
fn use_vars_in_command_and_shell() {
    setup();
    run_tests_for_both_inventories("test-ymls/use-vars-in-command-and-shell.yml", false, &[]);
}

#[test]
fn setting_working_directory_before_running_commands() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/setting-working-directory-before-running-commands.yml",
        false,
        &[],
    );
}

#[test]
fn nested_json_parsing() {
    setup();
    run_tests_for_both_inventories("test-ymls/nested-json-parsing.yml", false, &[]);
}

#[test]
fn setting_global_working_directory_before_running_commands() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/setting-global-working-directory-before-running-commands.yml",
        false,
        &[],
    );
}

#[test]
fn dont_run_2nd_deploy_if_1st_fails() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/dont-run-2nd-task-or-2nd-deploy-if-1st-fails.yml",
        true,
        &[],
    );
}

#[test]
fn shell_block_shares_state_across_lines() {
    setup();
    run_tests_for_both_inventories("test-ymls/shell-block-shares-state.yml", false, &[]);
}

#[test]
fn use_output_of_one_task_shell_in_another_task_shell() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/use-output-of-one-task-shell-in-another-task-shell.yml",
        false,
        &[],
    );
}

#[test]
fn set_and_use_vars_immediately_in_shell_and_command() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/set-and-use-vars-immediately-in-shell-and-command.yml",
        false,
        &[],
    );
}

#[test]
fn debug_should_come_before_command_and_shell() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/debug-should-come-before-command-and-shell.yml",
        false,
        &[],
    );
}

#[test]
fn nested_json_parsing_missing_property_error() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/nested-json-parsing-missing-property-error.yml",
        true,
        &[],
    );
}

#[test]
fn missing_var_error() {
    setup();
    run_tests_for_both_inventories("test-ymls/missing-var-error.yml", true, &[]);
}

#[test]
fn invalid_json_error() {
    setup();
    run_tests_for_both_inventories("test-ymls/invalid-json-error.yml", true, &[]);
}

#[test]
fn extra_vars() {
    setup();
    run_tests_for_both_inventories("test-ymls/extra-vars.yml", false, &["cat=1 bat=2"]);
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        &["{ \"cat\": 1, \"bat\": 2 }"],
    );
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        &["@test-ymls/extra-vars.vars.yml"],
    );
}

#[test]
fn extra_vars_multiple_e() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        &[
            "@test-ymls/extra-vars-multi-e.vars1.yml",
            "@test-ymls/extra-vars-multi-e.vars2.yml",
        ],
    );
}

#[test]
fn extra_vars_later_overrides_earlier() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        &["cat=wrong bat=2", "cat=1"],
    );
}

#[test]
fn when_condition() {
    setup();
    run_tests_for_both_inventories("test-ymls/when-condition.yml", false, &["condition=true"]);
}

#[test]
fn loop_item() {
    setup();
    run_tests_for_both_inventories("test-ymls/loop-item.yml", false, &[]);
}

#[test]
fn include_tasks() {
    setup();
    run_tests_for_both_inventories("test-ymls/include-tasks.yml", false, &[]);
}

#[test]
fn run_level_vars() {
    setup();
    run_tests_for_both_inventories("test-ymls/run-level-vars.yml", false, &[]);
}

#[test]
fn use_vars_in_chdir() {
    setup();
    run_tests_for_both_inventories("test-ymls/use-vars-in-chdir.yml", false, &[]);
}

#[test]
fn use_vars_in_task_name() {
    setup();
    run_tests_for_both_inventories("test-ymls/use-vars-in-task-name.yml", false, &[]);
}

#[test]
fn use_vars_in_run_name() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/use-vars-in-run-name.yml",
        false,
        &["@test-ymls/use-vars-in-run-name.vars.yml"],
    );
}

#[test]
fn become_nopasswd() {
    setup();
    run_test(
        "test-ymls/become-nopasswd.yml",
        false,
        &["become_password="],
        "tests/servers/become-nopass.yml",
    );
}

#[test]
fn become_with_password() {
    setup();
    run_test(
        "test-ymls/become-with-password.yml",
        false,
        &["become_password=password"],
        "tests/servers/become-withpass.yml",
    );
}

#[test]
fn become_su_nopasswd() {
    setup();
    run_test(
        "test-ymls/become-su-nopasswd.yml",
        false,
        &["become_password="],
        "tests/servers/become-root.yml",
    );
}

#[test]
fn become_invalid_method_error() {
    setup();
    run_test(
        "test-ymls/become-invalid-method-error.yml",
        true,
        &[],
        "tests/servers/local.yml",
    );
}

#[test]
fn become_su_with_password() {
    setup();
    run_test(
        "test-ymls/become-su-with-password.yml",
        false,
        &["become_password=password"],
        "tests/servers/become-withpass.yml",
    );
}

#[test]
fn become_doas() {
    setup();
    run_test(
        "test-ymls/become-doas.yml",
        false,
        &[],
        "tests/servers/become-doas.yml",
    );
}

#[test]
fn become_doas_with_password_error() {
    setup();
    run_test(
        "test-ymls/become-doas-with-password-error.yml",
        true,
        &["become_password=secret"],
        "tests/servers/local.yml",
    );
}

#[test]
fn servers_yml_var_support() {
    setup();
    run_test(
        "test-ymls/setting-and-debugging-vars.yml",
        false,
        &["test_host=localhost"],
        "tests/servers/local-templated.yml",
    );
}

#[test]
fn servers_yml_var_support_remote_fields() {
    setup();
    run_test(
        "test-ymls/setting-and-debugging-vars.yml",
        false,
        &[
            "remote_host=localhost",
            "remote_user=root",
            "remote_password=password",
        ],
        "tests/servers/remote-templated.yml",
    );
}
