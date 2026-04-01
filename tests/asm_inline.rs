//! Integration tests for inline assembly support.
//!
//! Compiles each `.c` file under `tests/unit/asm/<subfolder>/` with v6c
//! and verifies that compilation succeeds and expected assembly patterns appear.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
}

fn compile(subfolder: &str, name: &str) -> String {
    let root = repo_root();
    let src = root.join("tests").join("unit").join("asm").join(subfolder).join(name);
    let out_dir = root.join("out").join("tests").join("unit").join("asm").join(subfolder);
    fs::create_dir_all(&out_dir).unwrap();
    let base = name.trim_end_matches(".c");
    let asm_path = out_dir.join(format!("{}.asm", base));

    // Step 1: compile .c → .asm with v6c
    let output = Command::new(env!("CARGO_BIN_EXE_v6c"))
        .arg(&src)
        .arg("-o")
        .arg(&asm_path)
        .output()
        .expect("failed to run v6c");

    assert!(
        output.status.success(),
        "v6c failed on {}:\n{}",
        src.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let asm = fs::read_to_string(&asm_path).unwrap_or_default();

    // Step 2: assemble .asm with v6asm (must succeed with no errors)
    let v6asm = root.join("tools").join("v6asm").join("v6asm.exe");
    if v6asm.exists() {
        let asm_file = format!("{}.asm", base);
        let v6asm_out = Command::new(&v6asm)
            .arg(&asm_file)
            .arg("--lst")
            .current_dir(&out_dir)
            .output()
            .expect("failed to run v6asm");
        assert!(
            v6asm_out.status.success(),
            "v6asm failed on {}:\n{}\n{}",
            asm_path.display(),
            String::from_utf8_lossy(&v6asm_out.stdout),
            String::from_utf8_lossy(&v6asm_out.stderr)
        );
    }

    asm
}

// ---------------------------------------------------------------------------
// full_body/
// ---------------------------------------------------------------------------

#[test]
fn full_body_add() {
    let asm = compile("full_body", "add.c");
    assert!(asm.contains("DAD D"), "add's DAD D should appear in output");
    assert!(
        !asm.contains("SHLD _l_add_"),
        "full-body add should have no param prologue"
    );
}

#[test]
fn full_body_identity() {
    compile("full_body", "identity.c");
}

#[test]
fn full_body_negate_byte() {
    let asm = compile("full_body", "negate_byte.c");
    assert!(asm.contains("CMA"), "negate_byte should contain CMA");
    assert!(asm.contains("INR A"), "negate_byte should contain INR A");
}

#[test]
fn full_body_explicit_ret() {
    let asm = compile("full_body", "explicit_ret.c");
    assert!(asm.contains("DAD D"), "explicit_ret should contain DAD D");
    assert!(asm.contains("\tRET") || asm.contains("\tHLT"),
        "explicit_ret should contain RET or HLT (control flow terminator)");
}

#[test]
fn full_body_internal_labels() {
    let asm = compile("full_body", "internal_labels.c");
    assert!(asm.contains("_abs_done:"), "internal label _abs_done should be present");
}

#[test]
fn full_body_embedded_data() {
    let asm = compile("full_body", "embedded_data.c");
    assert!(asm.contains("_lc_data:"), "data label _lc_data should be present");
}

#[test]
fn full_body_embedded_data_local_labels() {
    let asm = compile("full_body", "embedded_data_local_labels.c");
    assert!(asm.contains("@data:"), "v6asm local label @data should be present");
}

#[test]
fn full_body_remove_unused_asm_funcs() {
    let asm = compile("full_body", "remove_unused_asm funcs.c");
    assert!(!asm.contains("unused_func"), "unused asm function should be eliminated");
}

// ---------------------------------------------------------------------------
// param/
// ---------------------------------------------------------------------------

#[test]
fn param_char() {
    let asm = compile("param", "char_param.c");
    assert!(asm.contains("OUT 42"), "OUT 42 should appear in output");
}

#[test]
fn param_int() {
    compile("param", "int_param.c");
}

#[test]
fn param_two() {
    let asm = compile("param", "two_params.c");
    assert!(asm.contains("DAD D"), "DAD D should appear from two-param add");
}

#[test]
fn param_empty() {
    let asm = compile("param", "empty_params.c");
    assert!(asm.contains("\tEI"), "EI should appear from empty-param block");
}

#[test]
fn param_char_sta() {
    let asm = compile("param", "char_sta.c");
    assert!(asm.contains("STA"), "STA should appear in output");
}

// ---------------------------------------------------------------------------
// raw/
// ---------------------------------------------------------------------------

#[test]
fn raw_local_labels() {
    let asm = compile("raw", "local_labels.c");
    assert!(
        asm.contains("_l_sum_via_asm_a"),
        "should reference _l_sum_via_asm_a"
    );
}

#[test]
fn raw_global_labels() {
    let asm = compile("raw", "global_labels.c");
    assert!(asm.contains("_g_global_a"), "should reference _g_global_a");
    assert!(asm.contains("_g_global_b"), "should reference _g_global_b");
}

#[test]
fn raw_internal_labels() {
    let asm = compile("raw", "internal_labels.c");
    assert!(asm.contains("_ctt_loop:"), "internal loop label should be present");
}

#[test]
fn raw_multi_block() {
    compile("raw", "multi_block.c");
}

// ---------------------------------------------------------------------------
// selfmod/
// ---------------------------------------------------------------------------

#[test]
fn selfmod_equate() {
    let asm = compile("selfmod", "equate.c");
    assert!(asm.contains("= * + 1"), "self-mod equate pattern should be present");
}

#[test]
fn selfmod_storage() {
    let asm = compile("selfmod", "storage.c");
    assert!(asm.contains("_rt_temp:"), ".STORAGE label should be present");
}

#[test]
fn selfmod_reg_only() {
    compile("selfmod", "reg_only.c");
}

// ---------------------------------------------------------------------------
// loop/
// ---------------------------------------------------------------------------

#[test]
fn loop_raw_while() {
    compile("loop", "raw_while.c");
}

#[test]
fn loop_param_for() {
    compile("loop", "param_for.c");
}

#[test]
fn loop_noclobber() {
    let asm = compile("loop", "noclobber.c");
    assert!(asm.contains("\tNOP"), "NOP inside asm block should survive peephole");
}

#[test]
fn loop_nested() {
    compile("loop", "nested.c");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the assembly text for a named function (from its label to the
/// next `; function` comment or `; ---` section marker).
#[allow(dead_code)]
fn extract_function(asm: &str, name: &str) -> String {
    let label = format!("{}:", name);
    let mut found = false;
    let mut result = Vec::new();
    for line in asm.lines() {
        if !found {
            if line.trim() == label {
                found = true;
                result.push(line);
            }
        } else {
            if line.starts_with("; function ") || line.starts_with("; ---") {
                break;
            }
            result.push(line);
        }
    }
    result.join("\n")
}
