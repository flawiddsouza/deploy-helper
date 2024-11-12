use std::fs;
use std::process::Command;
use std::sync::Once;

static INIT: Once = Once::new();

struct DockerGuard;

impl Drop for DockerGuard {
    fn drop(&mut self) {
        stop_docker_container();
    }
}

fn start_docker_container() {
    let start_output = Command::new("docker")
        .args(&[
            "run",
            "-d",
            "--rm",
            "-p",
            "2222:2222",
            "--name",
            "ssh_test_server",
            "-e",
            "USER_NAME=root",
            "-e",
            "USER_PASSWD=password",
            "forumi0721/alpine-sshd:x64",
        ])
        .output()
        .expect("Failed to start Docker container");

    assert!(start_output.status.success());
}

fn stop_docker_container() {
    let _stop_output = Command::new("docker")
        .args(&["stop", "ssh_test_server"])
        .output();
}

fn run_test(yml_file: &str, should_fail: bool, extra_vars: &str, inventory_file: &str) {
    let output = Command::new("cargo")
        .args(&[
            "run",
            "--quiet",
            "--",
            yml_file,
            "--extra-vars",
            extra_vars,
            "--inventory",
            inventory_file,
        ])
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

fn setup() -> DockerGuard {
    INIT.call_once(|| {
        start_docker_container();
    });
    DockerGuard
}

fn run_tests_for_both_inventories(yml_file: &str, should_fail: bool, extra_vars: &str) {
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
    run_tests_for_both_inventories("test-ymls/setting-and-debugging-vars.yml", false, "");
}

#[test]
fn use_vars_in_command_and_shell() {
    setup();
    run_tests_for_both_inventories("test-ymls/use-vars-in-command-and-shell.yml", false, "");
}

#[test]
fn setting_working_directory_before_running_commands() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/setting-working-directory-before-running-commands.yml",
        false,
        "",
    );
}

#[test]
fn nested_json_parsing() {
    setup();
    run_tests_for_both_inventories("test-ymls/nested-json-parsing.yml", false, "");
}

#[test]
fn setting_global_working_directory_before_running_commands() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/setting-global-working-directory-before-running-commands.yml",
        false,
        "",
    );
}

#[test]
fn dont_run_2nd_deploy_if_1st_fails() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/dont-run-2nd-task-or-2nd-deploy-if-1st-fails.yml",
        true,
        "",
    );
}

#[test]
fn use_output_of_one_task_shell_in_another_task_shell() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/use-output-of-one-task-shell-in-another-task-shell.yml",
        false,
        "",
    );
}

#[test]
fn set_and_use_vars_immediately_in_shell_and_command() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/set-and-use-vars-immediately-in-shell-and-command.yml",
        false,
        "",
    );
}

#[test]
fn debug_should_come_before_command_and_shell() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/debug-should-come-before-command-and-shell.yml",
        false,
        "",
    );
}

#[test]
fn nested_json_parsing_missing_property_error() {
    setup();
    run_tests_for_both_inventories(
        "test-ymls/nested-json-parsing-missing-property-error.yml",
        true,
        "",
    );
}

#[test]
fn missing_var_error() {
    setup();
    run_tests_for_both_inventories("test-ymls/missing-var-error.yml", true, "");
}

#[test]
fn invalid_json_error() {
    setup();
    run_tests_for_both_inventories("test-ymls/invalid-json-error.yml", true, "");
}

#[test]
fn extra_vars() {
    setup();
    run_tests_for_both_inventories("test-ymls/extra-vars.yml", false, "cat=1 bat=2");
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        "{ \"cat\": 1, \"bat\": 2 }",
    );
    run_tests_for_both_inventories(
        "test-ymls/extra-vars.yml",
        false,
        "@test-ymls/extra-vars.vars.yml",
    );
}

#[test]
fn when_condition() {
    setup();
    run_tests_for_both_inventories("test-ymls/when-condition.yml", false, "condition=true");
}

#[test]
fn loop_item() {
    setup();
    run_tests_for_both_inventories("test-ymls/loop-item.yml", false, "");
}
