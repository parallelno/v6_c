use std::env;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn run_optimization_small_test_suite() {
    let repo_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let v6asm_exe = repo_root.join("tools").join("v6asm").join("v6asm.exe");
    let v6asm_bin = repo_root.join("tools").join("v6asm").join("v6asm");
    let v6emul_exe = repo_root.join("tools").join("v6emul").join("v6emul.exe");
    let v6emul_bin = repo_root.join("tools").join("v6emul").join("v6emul");

    let has_v6asm = if cfg!(windows) {
        v6asm_exe.exists()
    } else {
        v6asm_bin.exists()
    };
    let has_v6emul = if cfg!(windows) {
        v6emul_exe.exists()
    } else {
        v6emul_bin.exists()
    };

    if !has_v6asm {
        eprintln!("Skipping optimization_small test: v6asm not found in tools/v6asm");
        return;
    }
    if !has_v6emul {
        eprintln!("Skipping optimization_small test: v6emul not found in tools/v6emul");
        return;
    }

    let script_path = repo_root.join("scripts").join("run_optimization_unit_checks.ps1");
    let shell = if cfg!(windows) { "powershell" } else { "pwsh" };

    let output = Command::new(shell)
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
