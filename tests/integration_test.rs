use std::fs;
use std::process::Command;

fn run_test(yml_file: &str, should_fail: bool) {
    let output = Command::new("cargo")
        .args(&["run", "--quiet", yml_file])
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

#[test]
fn setting_and_debugging_vars() {
    run_test("test-ymls/setting-and-debugging-vars.yml", false);
}

#[test]
fn use_vars_in_command_and_shell() {
    run_test("test-ymls/use-vars-in-command-and-shell.yml", false);
}

#[test]
fn setting_working_directory_before_running_commands() {
    run_test(
        "test-ymls/setting-working-directory-before-running-commands.yml",
        false,
    );
}

#[test]
fn nested_json_parsing() {
    run_test("test-ymls/nested-json-parsing.yml", false);
}

#[test]
fn setting_global_working_directory_before_running_commands() {
    run_test(
        "test-ymls/setting-global-working-directory-before-running-commands.yml",
        false,
    );
}

#[test]
fn dont_run_2nd_deploy_if_1st_fails() {
    run_test(
        "test-ymls/dont-run-2nd-task-or-2nd-deploy-if-1st-fails.yml",
        true,
    );
}

#[test]
fn use_output_of_one_task_shell_in_another_task_shell() {
    run_test(
        "test-ymls/use-output-of-one-task-shell-in-another-task-shell.yml",
        false,
    );
}

#[test]
fn set_and_use_vars_immediately_in_shell_and_command() {
    run_test(
        "test-ymls/set-and-use-vars-immediately-in-shell-and-command.yml",
        false,
    );
}

#[test]
fn debug_should_come_before_command_and_shell() {
    run_test(
        "test-ymls/debug-should-come-before-command-and-shell.yml",
        false,
    );
}

#[test]
fn nested_json_parsing_missing_property_error() {
    run_test(
        "test-ymls/nested-json-parsing-missing-property-error.yml",
        true,
    );
}

#[test]
fn missing_var_error() {
    run_test("test-ymls/missing-var-error.yml", true);
}

#[test]
fn invalid_json_error() {
    run_test("test-ymls/invalid-json-error.yml", true);
}
