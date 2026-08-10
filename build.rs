use std::env;
use std::path::PathBuf;
use std::process::Command;

fn git_output(manifest_dir: &PathBuf, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let commit = git_output(&manifest_dir, &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=DEPLOY_HELPER_GIT_COMMIT={commit}");

    if let Some(git_dir) = git_output(&manifest_dir, &["rev-parse", "--git-dir"]) {
        let git_dir = manifest_dir.join(git_dir);
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());

        if let Some(reference) = git_output(&manifest_dir, &["symbolic-ref", "-q", "HEAD"]) {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference).display()
            );
        }
    }
}
