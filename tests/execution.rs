//! End-to-end execution tests for unit C programs.
//!
//! Pipeline per test:
//! 1. Validate source has `int main(...)` and at least one `return` in main.
//! 2. Compile with v6c to `.asm`.
//! 3. Validate `.asm` has `.ORG` and `HLT`.
//! 4. Assemble with v6asm to `.rom`.
//! 5. Execute with v6emul in headless mode and assert HL == 0.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"))
}

fn collect_c_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_c_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("c")) {
            out.push(path);
        }
    }
}

fn source_execution_ready(src: &str) -> Result<(), &'static str> {
    let normalized = src.replace('\r', "");
    let has_int_main = normalized.contains("int main(") || normalized.contains("int\nmain(");
    if !has_int_main {
        return Err("missing 'int main(...)'");
    }

    if !normalized.contains("return") {
        return Err("main has no return statement");
    }

    Ok(())
}

fn first_org_value(asm: &str) -> Option<String> {
    for line in asm.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(".ORG") {
            let value = rest.trim();
            if value.is_empty() {
                continue;
            }
            let token = value.split_whitespace().next().unwrap_or_default();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn asm_execution_ready(asm: &str) -> Result<(), &'static str> {
    if first_org_value(asm).is_none() {
        return Err("missing .ORG directive");
    }

    let has_hlt = asm
        .lines()
        .any(|line| line.trim_start().starts_with("HLT") || line.trim_start().starts_with("HLT "));
    if !has_hlt {
        return Err("missing HLT instruction");
    }

    Ok(())
}

fn parse_hl_return(output: &str) -> Option<u16> {
    let mut cpu_line = None;
    for line in output.lines() {
        if line.trim_start().starts_with("CPU:") {
            cpu_line = Some(line);
            break;
        }
    }
    let cpu_line = cpu_line?;

    let mut h: Option<u8> = None;
    let mut l: Option<u8> = None;
    for token in cpu_line.split_whitespace() {
        if let Some(v) = token.strip_prefix("H=") {
            h = u8::from_str_radix(v, 16).ok();
        } else if let Some(v) = token.strip_prefix("L=") {
            l = u8::from_str_radix(v, 16).ok();
        }
    }

    Some(((h? as u16) << 8) | (l? as u16))
}

#[test]
fn run_unit_rom_execution_suite() {
    let root = repo_root();
    let tests_root = root.join("tests").join("unit");
    let out_root = root.join("out").join("tests").join("unit").join("execution");
    fs::create_dir_all(&out_root).expect("failed to create output directory for execution tests");

    let v6asm = root.join("tools").join("v6asm").join("v6asm.exe");
    if !v6asm.exists() {
        eprintln!("Skipping execution suite: missing {}", v6asm.display());
        return;
    }
    if !cfg!(windows) {
        eprintln!("Skipping execution suite: v6asm is Windows executable, running only on Windows hosts");
        return;
    }

    let v6emul = root.join("tools").join("v6emul").join("v6emul.exe");
    if !v6emul.exists() {
        eprintln!("Skipping execution suite: missing {}", v6emul.display());
        return;
    }

    let mut cases = Vec::new();
    collect_c_files(&tests_root, &mut cases);
    cases.sort();
    assert!(!cases.is_empty(), "no C tests found under {}", tests_root.display());

    let mut executed_count = 0usize;

    for src_path in cases {
        let rel = src_path
            .strip_prefix(&root)
            .unwrap_or(&src_path)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&src_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", src_path.display()));

        if let Err(reason) = source_execution_ready(&source) {
            eprintln!("SKIP {}: {}", rel, reason);
            continue;
        }

        let rel_from_unit = src_path
            .strip_prefix(&tests_root)
            .unwrap_or(&src_path);
        let mut asm_path = out_root.join(rel_from_unit);
        asm_path.set_extension("asm");

        if let Some(parent) = asm_path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("failed to create {}: {e}", parent.display()));
        }

        let base_name = src_path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("invalid source filename");

        let compile = Command::new(env!("CARGO_BIN_EXE_v6c"))
            .arg(&src_path)
            .arg("-o")
            .arg(&asm_path)
            .output()
            .expect("failed to run v6c");

        assert!(
            compile.status.success(),
            "v6c failed for {}\nstdout:\n{}\nstderr:\n{}",
            rel,
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        );

        let asm = fs::read_to_string(&asm_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", asm_path.display()));

        if let Err(reason) = asm_execution_ready(&asm) {
            eprintln!("SKIP {}: {}", rel, reason);
            continue;
        }

        let org = first_org_value(&asm).unwrap_or_else(|| "0".to_string());

        let asm_dir = asm_path.parent().expect("asm output has no parent directory");
        let asm_file = asm_path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("asm filename is invalid");

        let assemble = Command::new(&v6asm)
            .arg(asm_file)
            .arg("--lst")
            .current_dir(asm_dir)
            .output()
            .expect("failed to run v6asm");

        assert!(
            assemble.status.success(),
            "v6asm failed for {}\nstdout:\n{}\nstderr:\n{}",
            rel,
            String::from_utf8_lossy(&assemble.stdout),
            String::from_utf8_lossy(&assemble.stderr)
        );

        let rom_path = asm_dir.join(format!("{}.rom", base_name));
        assert!(
            rom_path.exists(),
            "expected ROM output missing for {}: {}",
            rel,
            rom_path.display()
        );

        let emul = Command::new(&v6emul)
            .arg("--rom")
            .arg(&rom_path)
            .arg("--load-addr")
            .arg(&org)
            .arg("--halt-exit")
            .arg("--dump-cpu")
            .arg("--run-cycles")
            .arg("1000000")
            .output()
            .expect("failed to run v6emul");

        let mut emul_output = String::new();
        emul_output.push_str(&String::from_utf8_lossy(&emul.stdout));
        emul_output.push_str(&String::from_utf8_lossy(&emul.stderr));

        assert!(
            emul.status.success(),
            "v6emul failed for {}\n{}",
            rel,
            emul_output
        );

        let hl = parse_hl_return(&emul_output)
            .unwrap_or_else(|| panic!("failed to parse H/L from emulator output for {}\n{}", rel, emul_output));

        assert_eq!(
            hl, 0,
            "execution failed for {}: HL={} (0x{:04X})\n{}",
            rel,
            hl,
            hl,
            emul_output
        );

        executed_count += 1;
    }

    assert!(
        executed_count > 0,
        "no tests were executed. migrate at least one tests/unit/*.c case to int main + return checks"
    );
}
