use std::process::Command;
use std::fs;

fn run_test(yml_file: &str) {
    let output = Command::new("cargo")
        .args(&["run", yml_file])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let expected_output = fs::read_to_string(&format!("{}.out", yml_file))
        .expect("Failed to read expected output");

    assert_eq!(stdout, expected_output);
}

#[test]
fn test_setting_and_debugging_vars() {
    run_test("test-ymls/setting-and-debugging-vars.yml");
}

#[test]
fn test_use_vars_in_command_and_shell() {
    run_test("test-ymls/use-vars-in-command-and-shell.yml");
}

#[test]
fn test_setting_working_directory_before_running_commands() {
    run_test("test-ymls/setting-working-directory-before-running-commands.yml");
}

#[test]
fn test_nested_json_parsing() {
    run_test("test-ymls/nested-json-parsing.yml");
}

#[test]
fn test_setting_global_working_directory_before_running_commands() {
    run_test("test-ymls/setting-global-working-directory-before-running-commands.yml");
}
