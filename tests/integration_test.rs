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
