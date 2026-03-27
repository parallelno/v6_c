//! Runtime library embedding and selective inclusion.
//!
//! Embeds all runtime `.asm` files as string constants and provides an API
//! to determine which routines are referenced by the generated code, so the
//! emitter can include only the needed portions.

use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Embedded runtime assembly sources
// ---------------------------------------------------------------------------

const CRT0_ASM: &str = include_str!("../runtime/crt0.asm");
const MUL16_ASM: &str = include_str!("../runtime/mul16.asm");
const DIV16_ASM: &str = include_str!("../runtime/div16.asm");
const MUL32_ASM: &str = include_str!("../runtime/mul32.asm");
const DIV32_ASM: &str = include_str!("../runtime/div32.asm");
const SHIFT_ASM: &str = include_str!("../runtime/shift.asm");
const SHIFT32_ASM: &str = include_str!("../runtime/shift32.asm");
const CMP_ASM: &str = include_str!("../runtime/cmp.asm");
const MEMCPY_ASM: &str = include_str!("../runtime/memcpy.asm");
const STRING_ASM: &str = include_str!("../runtime/string.asm");
const STDIO_ASM: &str = include_str!("../runtime/stdio.asm");
const STDLIB_ASM: &str = include_str!("../runtime/stdlib.asm");
const FLOAT_ASM: &str = include_str!("../runtime/float.asm");

// ---------------------------------------------------------------------------
// Runtime module descriptors
// ---------------------------------------------------------------------------

/// A runtime module: a group of related assembly routines in one .asm file.
struct RuntimeModule {
    /// Labels (entry points) defined in this module.
    symbols: &'static [&'static str],
    /// Assembly source text.
    asm: &'static str,
    /// Other modules this module depends on (by symbol name).
    deps: &'static [&'static str],
}

/// All available runtime modules.
static MODULES: &[RuntimeModule] = &[
    RuntimeModule {
        symbols: &["_start"],
        asm: CRT0_ASM,
        deps: &["main"],
    },
    RuntimeModule {
        symbols: &["__mul16"],
        asm: MUL16_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &[
            "__div16u", "__div16s", "__mod16u", "__mod16s", "__divmod16u",
        ],
        asm: DIV16_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["__mul32"],
        asm: MUL32_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &[
            "__div32u", "__div32s", "__mod32u", "__mod32s",
            "__abs32_op1", "__abs32_op2", "__neg32_op1", "__divmod32u",
        ],
        asm: DIV32_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["__shl16", "__shr16u", "__shr16s"],
        asm: SHIFT_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["__shl32", "__shr32u", "__shr32s"],
        asm: SHIFT32_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["__cmp16u", "__cmp16s"],
        asm: CMP_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["memcpy", "memset", "strlen", "strcmp"],
        asm: MEMCPY_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &[
            "memmove", "strcpy", "strncpy", "strcat", "strncat",
            "strncmp", "strchr", "strrchr", "memcmp",
        ],
        asm: STRING_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["putchar", "__putchar_a", "getchar", "puts", "printf"],
        asm: STDIO_ASM,
        deps: &[],
    },
    RuntimeModule {
        symbols: &["abs", "atoi", "rand", "srand", "malloc", "free"],
        asm: STDLIB_ASM,
        deps: &["__mul16"], // rand depends on __mul16
    },
    RuntimeModule {
        symbols: &[
            "__fadd", "__fsub", "__fmul", "__fdiv",
            "__feq", "__fne", "__flt", "__fle", "__fgt", "__fge",
            "__itof", "__ftoi",
        ],
        asm: FLOAT_ASM,
        deps: &[],
    },
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan the generated assembly lines for references to runtime symbols.
/// Returns the assembly text for all needed runtime modules (with
/// transitive dependencies resolved).
pub fn collect_runtime(code_lines: &[String]) -> String {
    let referenced = find_referenced_symbols(code_lines);
    let needed = resolve_dependencies(&referenced);
    emit_runtime_modules(&needed)
}

/// Get the CRT0 startup code unconditionally.
pub fn crt0_asm() -> &'static str {
    CRT0_ASM
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Scan assembly lines for `CALL <symbol>` and `JMP <symbol>` references.
fn find_referenced_symbols(code_lines: &[String]) -> HashSet<String> {
    let mut refs = HashSet::new();

    for line in code_lines {
        let trimmed = line.trim();

        // Match CALL/JMP instructions
        for prefix in &["CALL ", "JMP "] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let symbol = rest.split_whitespace().next().unwrap_or(rest);
                // Only consider symbols that look like runtime or library labels
                let sym = symbol.trim();
                if !sym.is_empty() {
                    refs.insert(sym.to_string());
                }
            }
        }

        // Also check for LXI H,<label> patterns that reference data labels
        if let Some(rest) = trimmed.strip_prefix("LXI H,") {
            let sym = rest.trim();
            if !sym.is_empty() && !sym.starts_with("0x") && !sym.chars().next().unwrap_or('0').is_ascii_digit() {
                refs.insert(sym.to_string());
            }
        }
    }

    refs
}

/// Given a set of referenced symbols, determine which runtime modules are
/// needed (including transitive dependencies via symbols).
fn resolve_dependencies(referenced: &HashSet<String>) -> HashSet<usize> {
    let mut needed: HashSet<usize> = HashSet::new();
    let mut changed = true;

    // Collect initially needed modules
    for (idx, module) in MODULES.iter().enumerate() {
        for &sym in module.symbols {
            if referenced.contains(sym) {
                needed.insert(idx);
                break;
            }
        }
    }

    // Resolve transitive dependencies
    while changed {
        changed = false;
        let current: Vec<usize> = needed.iter().copied().collect();
        for idx in current {
            for &dep_sym in MODULES[idx].deps {
                // Find the module that provides this dependency
                for (dep_idx, dep_mod) in MODULES.iter().enumerate() {
                    if !needed.contains(&dep_idx) && dep_mod.symbols.contains(&dep_sym) {
                        needed.insert(dep_idx);
                        changed = true;
                    }
                }
            }
        }
    }

    needed
}

/// Concatenate the assembly text for the needed modules.
fn emit_runtime_modules(needed: &HashSet<usize>) -> String {
    let mut out = String::new();

    for (idx, module) in MODULES.iter().enumerate() {
        if needed.contains(&idx) {
            // Skip crt0 — it's handled separately by the emitter header
            if module.symbols.contains(&"_start") {
                continue;
            }
            out.push_str(module.asm);
            out.push('\n');
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_call_symbols() {
        let lines = vec![
            "\tCALL __mul16".to_string(),
            "\tCALL puts".to_string(),
            "\tJMP main".to_string(),
            "main:".to_string(),
            "\tLXI H,42".to_string(),
        ];
        let refs = find_referenced_symbols(&lines);
        assert!(refs.contains("__mul16"));
        assert!(refs.contains("puts"));
        assert!(refs.contains("main"));
    }

    #[test]
    fn mul16_included_when_referenced() {
        let lines = vec!["\tCALL __mul16".to_string()];
        let rt = collect_runtime(&lines);
        assert!(rt.contains("__mul16:"), "mul16 should be included");
    }

    #[test]
    fn nothing_included_when_no_refs() {
        let lines = vec![
            "main:".to_string(),
            "\tRET".to_string(),
        ];
        let rt = collect_runtime(&lines);
        // Should not include any runtime modules (no CALL/JMP to runtime symbols)
        assert!(!rt.contains("__mul16:"));
        assert!(!rt.contains("putchar:"));
    }

    #[test]
    fn stdlib_pulls_in_mul16() {
        // rand depends on __mul16
        let lines = vec!["\tCALL rand".to_string()];
        let rt = collect_runtime(&lines);
        assert!(rt.contains("rand:"), "rand should be included");
        assert!(rt.contains("__mul16:"), "__mul16 should be included as dependency");
    }

    #[test]
    fn string_functions_included() {
        let lines = vec!["\tCALL strcpy".to_string()];
        let rt = collect_runtime(&lines);
        assert!(rt.contains("strcpy:"));
    }

    #[test]
    fn printf_included() {
        let lines = vec!["\tCALL printf".to_string()];
        let rt = collect_runtime(&lines);
        assert!(rt.contains("printf:"));
    }
}
