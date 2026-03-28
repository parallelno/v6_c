use std::env;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn run_optimization_small_test_suite() {
    let repo_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let script_path = repo_root.join("scripts").join("run_optimization_unit_checks.ps1");

    let output = Command::new("powershell")
        .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script_path)
        .arg("-UseSmall")
        .output()
        .expect("Failed to spawn optimization test runner");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== optimization_small runner stdout ===\n{}", stdout);
    println!("=== optimization_small runner stderr ===\n{}", stderr);

    assert!(output.status.success(), "Optimization small test runner failed");
}
