use std::process::Command;

#[test]
fn test_setting_and_debugging_vars() {
    let output = Command::new("cargo")
        .args(&["run", "test-ymls/setting-and-debugging-vars.yml"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("The property value is This is cat This is hey"));
}

#[test]
fn test_use_vars_in_command_and_shell() {
    let output = Command::new("cargo")
        .args(&["run", "test-ymls/use-vars-in-command-and-shell.yml"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let expected_output = "\
Starting deployment: Test

Executing task: Set vars

Executing task: Run first command
> echo 'Hello'
Output:
Hello


Executing task: Run second command
> echo 'World' | tr '[:lower:]' '[:upper:]'
Output:
WORLD";

    assert_eq!(stdout, expected_output);
}

#[test]
fn test_setting_working_directory_before_running_commands() {
    let output = Command::new("cargo")
        .args(&["run", "test-ymls/setting-working-directory-before-running-commands.yml"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let expected_output = "\
Starting deployment: Test

Executing task: Set vars
> ls
Output:
file1.txt
file2.txt


Executing task: Set vars
> ls
Output:
file1.txt
file2.txt";

    assert_eq!(stdout, expected_output);
}
