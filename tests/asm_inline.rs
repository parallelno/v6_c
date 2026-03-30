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

fn compile(subfolder: &str, name: &str) -> (bool, String) {
    let root = repo_root();
    let src = root.join("tests").join("unit").join("asm").join(subfolder).join(name);
    let out_dir = root.join("out").join("tests").join("unit").join("asm").join(subfolder);
    fs::create_dir_all(&out_dir).unwrap();
    let base = name.trim_end_matches(".c");
    let asm_path = out_dir.join(format!("{}.asm", base));

    let output = Command::new(env!("CARGO_BIN_EXE_v6c"))
        .arg(src)
        .arg("-o")
        .arg(&asm_path)
        .output()
        .expect("failed to run v6c");

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return (false, stderr);
    }
    let asm = fs::read_to_string(&asm_path).unwrap_or_default();
    (true, asm)
}

// ---------------------------------------------------------------------------
// full_body/
// ---------------------------------------------------------------------------

#[test]
fn full_body_add() {
    let (ok, asm) = compile("full_body", "add.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("DAD D"), "add's DAD D should appear in output");
    assert!(
        !asm.contains("SHLD _l_add_"),
        "full-body add should have no param prologue"
    );
}

#[test]
fn full_body_identity() {
    let (ok, _) = compile("full_body", "identity.c");
    assert!(ok, "compilation failed");
}

#[test]
fn full_body_negate_byte() {
    let (ok, asm) = compile("full_body", "negate_byte.c");
    assert!(ok, "compilation failed");
    // Verify the asm body (CMA + INR A) appears in the output
    assert!(asm.contains("CMA"), "negate_byte should contain CMA");
    assert!(asm.contains("INR A"), "negate_byte should contain INR A");
}

#[test]
fn full_body_explicit_ret() {
    let (ok, asm) = compile("full_body", "explicit_ret.c");
    assert!(ok, "compilation failed");
    // The asm body contains DAD D + RET; verify both are present
    assert!(asm.contains("DAD D"), "explicit_ret should contain DAD D");
    assert!(asm.contains("\tRET") || asm.contains("\tHLT"),
        "explicit_ret should contain RET or HLT (control flow terminator)");
}

#[test]
fn full_body_internal_labels() {
    let (ok, asm) = compile("full_body", "internal_labels.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("_abs_done:"), "internal label _abs_done should be present");
}

#[test]
fn full_body_embedded_data() {
    let (ok, asm) = compile("full_body", "embedded_data.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("_lc_data:"), "data label _lc_data should be present");
}

// ---------------------------------------------------------------------------
// param/
// ---------------------------------------------------------------------------

#[test]
fn param_char() {
    let (ok, asm) = compile("param", "char_param.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("OUT 42"), "OUT 42 should appear in output");
}

#[test]
fn param_int() {
    let (ok, _) = compile("param", "int_param.c");
    assert!(ok, "compilation failed");
}

#[test]
fn param_two() {
    let (ok, asm) = compile("param", "two_params.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("DAD D"), "DAD D should appear from two-param add");
}

#[test]
fn param_empty() {
    let (ok, asm) = compile("param", "empty_params.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("\tEI"), "EI should appear from empty-param block");
}

#[test]
fn param_char_sta() {
    let (ok, asm) = compile("param", "char_sta.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("STA"), "STA should appear in output");
}

// ---------------------------------------------------------------------------
// raw/
// ---------------------------------------------------------------------------

#[test]
fn raw_local_labels() {
    let (ok, asm) = compile("raw", "local_labels.c");
    assert!(ok, "compilation failed");
    assert!(
        asm.contains("_l_sum_via_asm_a"),
        "should reference _l_sum_via_asm_a"
    );
}

#[test]
fn raw_global_labels() {
    let (ok, asm) = compile("raw", "global_labels.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("_g_global_a"), "should reference _g_global_a");
    assert!(asm.contains("_g_global_b"), "should reference _g_global_b");
}

#[test]
fn raw_internal_labels() {
    let (ok, asm) = compile("raw", "internal_labels.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("_ctt_loop:"), "internal loop label should be present");
}

#[test]
fn raw_multi_block() {
    let (ok, _) = compile("raw", "multi_block.c");
    assert!(ok, "compilation failed");
}

// ---------------------------------------------------------------------------
// selfmod/
// ---------------------------------------------------------------------------

#[test]
fn selfmod_equate() {
    let (ok, asm) = compile("selfmod", "equate.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("= * + 1"), "self-mod equate pattern should be present");
}

#[test]
fn selfmod_storage() {
    let (ok, asm) = compile("selfmod", "storage.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("_rt_temp:"), ".STORAGE label should be present");
}

#[test]
fn selfmod_reg_only() {
    let (ok, _) = compile("selfmod", "reg_only.c");
    assert!(ok, "compilation failed");
}

// ---------------------------------------------------------------------------
// loop/
// ---------------------------------------------------------------------------

#[test]
fn loop_raw_while() {
    let (ok, _) = compile("loop", "raw_while.c");
    assert!(ok, "compilation failed");
}

#[test]
fn loop_param_for() {
    let (ok, _) = compile("loop", "param_for.c");
    assert!(ok, "compilation failed");
}

#[test]
fn loop_noclobber() {
    let (ok, asm) = compile("loop", "noclobber.c");
    assert!(ok, "compilation failed");
    assert!(asm.contains("\tNOP"), "NOP inside asm block should survive peephole");
}

#[test]
fn loop_nested() {
    let (ok, _) = compile("loop", "nested.c");
    assert!(ok, "compilation failed");
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
