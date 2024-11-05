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

    let expected_output = fs::read_to_string(&format!("{}.out", yml_file))
        .expect("Failed to read expected output");

    assert_eq!(full_output, expected_output);
}

#[test]
fn test_setting_and_debugging_vars() {
    run_test("test-ymls/setting-and-debugging-vars.yml", false);
}

#[test]
fn test_use_vars_in_command_and_shell() {
    run_test("test-ymls/use-vars-in-command-and-shell.yml", false);
}

#[test]
fn test_setting_working_directory_before_running_commands() {
    run_test("test-ymls/setting-working-directory-before-running-commands.yml", false);
}

#[test]
fn test_nested_json_parsing() {
    run_test("test-ymls/nested-json-parsing.yml", false);
}

#[test]
fn test_setting_global_working_directory_before_running_commands() {
    run_test("test-ymls/setting-global-working-directory-before-running-commands.yml", false);
}

#[test]
fn test_dont_run_2nd_deploy_if_1st_fails() {
    run_test("test-ymls/dont-run-2nd-task-or-2nd-deploy-if-1st-fails.yml", true);
}
