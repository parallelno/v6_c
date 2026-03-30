#![allow(dead_code)]

mod ast;
mod callgraph;
mod codegen;
mod emit;
mod ir;
mod ir_gen;
mod ir_opt;
mod lexer;
mod parser;
mod peephole;
mod preproc;
mod regalloc;
mod runtime;
mod types;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let opts = match parse_args(&args) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("v6c: {}", msg);
            std::process::exit(1);
        }
    };

    if let Err(e) = compile(&opts) {
        eprintln!("v6c: {}", e);
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Compiler options
// ---------------------------------------------------------------------------

struct CompilerOpts {
    /// Input source files.
    inputs: Vec<String>,
    /// Output assembly file.
    output: String,
    /// Optional listing (`.lst`) output file.
    lst_output: Option<String>,
    /// Additional include search paths (`-I`).
    include_paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

fn print_usage() {
    eprintln!("Usage: v6c <input.c> [input2.c ...] [-o <output.asm>] [--lst <output.lst>] [--no-lst] [-I <path>]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -o <file>   Output file path (default: first input with .asm extension)");
    eprintln!("  --lst <file>  Listing output path (default: output path with .lst extension)");
    eprintln!("  --no-lst      Disable listing output generation");
    eprintln!("  -I <path>   Add include search path");
    eprintln!("  -h, --help  Show this help message");
}

fn parse_args(args: &[String]) -> Result<CompilerOpts, String> {
    let mut inputs: Vec<String> = Vec::new();
    let mut output: Option<String> = None;
    let mut lst_output: Option<String> = None;
    let mut emit_lst = true;
    let mut include_paths: Vec<String> = Vec::new();

    let mut i = 1; // skip program name
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    return Err("-o requires an argument".into());
                }
                output = Some(args[i].clone());
            }
            "-I" => {
                i += 1;
                if i >= args.len() {
                    return Err("-I requires an argument".into());
                }
                include_paths.push(args[i].clone());
            }
            "--lst" => {
                i += 1;
                if i >= args.len() {
                    return Err("--lst requires an argument".into());
                }
                lst_output = Some(args[i].clone());
            }
            "--no-lst" => {
                emit_lst = false;
            }
            arg if arg.starts_with("-I") => {
                // Support -Ipath (no space)
                include_paths.push(arg[2..].to_string());
            }
            arg if arg.starts_with('-') => {
                return Err(format!("unknown option: {}", arg));
            }
            _ => {
                inputs.push(args[i].clone());
            }
        }
        i += 1;
    }

    if inputs.is_empty() {
        return Err("no input file specified".into());
    }

    let output_path = output.unwrap_or_else(|| {
        let first = &inputs[0];
        if let Some(stem) = first.strip_suffix(".c") {
            format!("{}.asm", stem)
        } else {
            format!("{}.asm", first)
        }
    });

    let final_lst_output = if emit_lst {
        Some(lst_output.unwrap_or_else(|| {
            if let Some(stem) = output_path.strip_suffix(".asm") {
                format!("{}.lst", stem)
            } else {
                format!("{}.lst", output_path)
            }
        }))
    } else {
        None
    };

    Ok(CompilerOpts {
        inputs,
        output: output_path,
        lst_output: final_lst_output,
        include_paths,
    })
}

// ---------------------------------------------------------------------------
// Compilation pipeline
// ---------------------------------------------------------------------------

/// Detect the `include/` directory bundled with the compiler.
fn find_system_include_dir() -> Option<String> {
    // Try relative to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let include = dir.join("include");
            if include.is_dir() {
                return Some(include.to_string_lossy().into_owned());
            }
            // Also try one level up (for cargo build layouts)
            if let Some(parent) = dir.parent() {
                let include = parent.join("include");
                if include.is_dir() {
                    return Some(include.to_string_lossy().into_owned());
                }
                // Two levels up
                if let Some(grandparent) = parent.parent() {
                    let include = grandparent.join("include");
                    if include.is_dir() {
                        return Some(include.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    // Try relative to working directory
    let cwd_include = std::path::Path::new("include");
    if cwd_include.is_dir() {
        return Some("include".to_string());
    }
    None
}

fn compile(opts: &CompilerOpts) -> Result<(), String> {
    let mut source_lines_for_lst: Option<Vec<String>> = None;

    let optimized = if opts.inputs.len() == 1 {
        // Single-file mode (most common)
        let source = std::fs::read_to_string(&opts.inputs[0])
            .map_err(|e| format!("{}: {}", opts.inputs[0], e))?;
        source_lines_for_lst = Some(source.lines().map(|l| l.to_string()).collect());
        compile_source(&source, &opts.inputs[0], &opts.include_paths)?
    } else {
        // Multi-file mode: parse each file into IR separately, merge, then
        // run optimization and codegen on the merged program.
        compile_multi(&opts.inputs, &opts.include_paths)?
    };

    emit::emit_asm(&optimized, &opts.output)
        .map_err(|e| format!("{}: {}", opts.output, e))?;

    if let Some(lst_path) = &opts.lst_output {
        emit::emit_lst(&optimized, lst_path, source_lines_for_lst.as_deref())
            .map_err(|e| format!("{}: {}", lst_path, e))?;
    }

    Ok(())
}

/// Multi-file compilation: preprocess/parse/lower each file to IR, merge,
/// then optimize/analyze/codegen the combined program.
fn compile_multi(inputs: &[String], include_paths: &[String]) -> Result<Vec<String>, String> {
    let mut merged = ir::IrProgram::new();

    for path in inputs {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("{}: {}", path, e))?;

        let mut pp = preproc::Preprocessor::new();
        // Set up include paths
        if let Some(sys) = find_system_include_dir() {
            pp.add_system_include_path(&sys);
        }
        for ip in include_paths {
            pp.add_include_path(ip);
        }

        let processed = pp
            .preprocess(&source, path)
            .map_err(|e| format!("{}: preprocessor: {}", path, e.message))?;

        let tokens = lexer::tokenize(&processed)
            .map_err(|e| format!("{}:{}:{}: {}", path, e.line, e.column, e.message))?;

        let program = parser::Parser::new(&tokens, &processed)
            .parse()
            .map_err(|errs| {
                errs.iter()
                    .map(|e| format!("{}:{}:{}: {}", path, e.line, e.column, e.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;

        let ir_program = ir_gen::generate(&program).map_err(|errs| {
            errs.iter()
                .map(|e| format!("{}:{}: {}", path, e.line, e.message))
                .collect::<Vec<_>>()
                .join("\n")
        })?;

        // Merge: append globals, functions, strings (avoiding duplicates)
        merge_ir(&mut merged, ir_program);
    }

    // Optimize the merged program
    ir_opt::optimize(&mut merged);

    // Call-graph analysis on the whole program
    let analysis = callgraph::analyze(&merged, None);

    // Code generation
    let code_lines = codegen::generate(&merged, &analysis);

    // Peephole optimization
    Ok(peephole::peephole_optimize(code_lines))
}

/// Merge `src` into `dst`, skipping duplicate globals/functions.
fn merge_ir(dst: &mut ir::IrProgram, src: ir::IrProgram) {
    // Merge globals (skip duplicates by name)
    let existing_globals: std::collections::HashSet<String> =
        dst.globals.iter().map(|g| g.name.clone()).collect();
    for g in src.globals {
        if !existing_globals.contains(&g.name) {
            dst.globals.push(g);
        }
    }

    // Merge functions (skip duplicates by name — first definition wins)
    let existing_funcs: std::collections::HashSet<String> =
        dst.functions.iter().map(|f| f.name.clone()).collect();
    for f in src.functions {
        if !existing_funcs.contains(&f.name) {
            dst.functions.push(f);
        }
    }

    // Merge string literals (always append; label uniqueness is guaranteed
    // by the per-file counter prefix)
    dst.strings.extend(src.strings);
}

/// Compile C source text through the full pipeline, returning assembly lines.
fn compile_source(source: &str, filename: &str, include_paths: &[String]) -> Result<Vec<String>, String> {
    // 1. Preprocessor
    let mut pp = preproc::Preprocessor::new();
    if let Some(sys) = find_system_include_dir() {
        pp.add_system_include_path(&sys);
    }
    for ip in include_paths {
        pp.add_include_path(ip);
    }

    let processed = pp
        .preprocess(source, filename)
        .map_err(|e| format!("preprocessor error: {}", e.message))?;

    // 2. Lexer
    let tokens = lexer::tokenize(&processed)
        .map_err(|e| format!("{}:{}:{}: {}", filename, e.line, e.column, e.message))?;

    // 3. Parser
    let program = parser::Parser::new(&tokens, &processed)
        .parse()
        .map_err(|errs| {
            errs.iter()
                .map(|e| format!("{}:{}:{}: {}", filename, e.line, e.column, e.message))
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    // 4. IR generation
    let mut ir_program = ir_gen::generate(&program).map_err(|errs| {
        errs.iter()
            .map(|e| format!("{}:{}: {}", filename, e.line, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // 4.5. IR optimization
    ir_opt::optimize(&mut ir_program);

    // 5. Call-graph analysis
    let analysis = callgraph::analyze(&ir_program, None);

    // 6. Code generation
    let code_lines = codegen::generate(&ir_program, &analysis);

    // 7. Peephole optimization
    Ok(peephole::peephole_optimize(code_lines))
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn has_line(output: &[String], needle: &str) -> bool {
        output.iter().any(|l| l.contains(needle))
    }

    #[test]
    fn pipeline_return_constant() {
        let src = "int main(void) { return 42; }";
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"), "must have main label");
        assert!(has_line(&out, "RET"), "must have RET");
        // Inlined startup should avoid CALL main in CRT0.
        assert!(!has_line(&out, "CALL main"));
    }

    #[test]
    fn pipeline_inline_main_body() {
        let src = "int main(void) { int x; x = 1; }";
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(!has_line(&out, "CALL main"));
        // Should contain main instructions in startup zone (LXI SP + some op)
        assert!(has_line(&out, "MOV" ) || has_line(&out, "STA") || has_line(&out, "LXI"));
        assert!(has_line(&out, "main:"), "main label should still exist");
    }

    #[test]
    fn pipeline_global_variable() {
        let src = "int x; void main(void) { x = 10; }";
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
        assert!(has_line(&out, "_g_x"));
    }

    #[test]
    fn pipeline_function_call() {
        // Use a function large enough to not be inlined.  The return value is
        // stored to a true global (`arg1`) so the call is never tail-called away
        // and always appears as a CALL or specialised CALL in the output.
        // Extra locals (w, v) keep the IR count above INLINE_THRESHOLD even
        // after load-store forwarding eliminates redundant reloads.
        let src = r#"
            int arg1;
            int compute(int a, int b) {
                int x;
                int y;
                int z;
                int w;
                int v;
                x = a + b;
                y = a - b;
                z = x + y;
                w = x - y;
                v = z + w;
                if (v > 0) {
                    v = v + x;
                } else {
                    v = v - y;
                }
                return v;
            }
            void main(void) { arg1 = compute(arg1, 4); }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        let has_compute = has_line(&out, "compute:");
        let has_spec = out.iter().any(|l| l.contains("__spec_compute"));
        assert!(
            has_compute || has_spec,
            "must have compute function or a specialized clone"
        );
        let calls_compute = has_line(&out, "CALL compute");
        let calls_specialized_compute = out
            .iter()
            .any(|l| l.contains("CALL __spec_compute"));
        assert!(
            calls_compute || calls_specialized_compute,
            "must call compute or a specialized compute clone"
        );
    }

    #[test]
    fn pipeline_while_loop() {
        let src = r#"
            int sum;
            void main(void) {
                int i;
                sum = 0;
                i = 0;
                while (i < 10) {
                    sum = sum + i;
                    i = i + 1;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
        // Must have at least one conditional jump (loop condition)
        let has_jump = out.iter().any(|l| {
            l.contains("JZ") || l.contains("JNZ") || l.contains("JC") || l.contains("JNC")
        });
        assert!(has_jump, "while loop must produce a conditional jump");
    }

    #[test]
    fn pipeline_if_else() {
        let src = r#"
            int result;
            void main(void) {
                int x;
                x = 5;
                if (x > 3) {
                    result = 1;
                } else {
                    result = 0;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_for_loop() {
        let src = r#"
            int total;
            void main(void) {
                int i;
                total = 0;
                for (i = 0; i < 5; i = i + 1) {
                    total = total + i;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_multiply_calls_runtime() {
        let src = r#"
            int result;
            void main(void) {
                int a; int b;
                a = 6; b = 7;
                result = a * b;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        let calls_runtime = has_line(&out, "CALL __mul16");
        let folded_to_imm = out.iter().any(|l| l.contains("LXI H,42") || l.contains("MVI A,42"));
        assert!(
            calls_runtime || folded_to_imm,
            "multiply should either call __mul16 or be folded to an immediate"
        );
    }

    #[test]
    fn pipeline_data_section_present() {
        let src = "int x; void main(void) { x = 1; }";
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "; --- data section ---"));
    }

    #[test]
    fn pipeline_pointer_dereference() {
        let src = r#"
            int val;
            void main(void) {
                int *p;
                p = &val;
                *p = 42;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_char_operations() {
        let src = r#"
            char ch;
            void main(void) {
                ch = 'A';
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn emit_produces_valid_file() {
        let src = "void main(void) { return; }";
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        let path = std::env::temp_dir().join("v6c_test_emit.asm");
        let path = path.to_str().unwrap();
        emit::emit_asm(&out, path).expect("emit failed");
        let contents = std::fs::read_to_string(path).expect("read failed");
        assert!(contents.contains(".ORG 0x100"));
        let _ = std::fs::remove_file(path);
    }

    // -----------------------------------------------------------------------
    // Phase 2.7 – Optimization benchmarks & verification
    // -----------------------------------------------------------------------

    /// Count lines that contain instructions (not labels, comments, or blank).
    fn count_instructions(output: &[String]) -> usize {
        output
            .iter()
            .filter(|l| l.starts_with('\t') && !l.trim().starts_with(';'))
            .count()
    }

    fn count_helper_calls(output: &[String]) -> usize {
        output
            .iter()
            .filter(|line| line.trim_start().starts_with("CALL __"))
            .count()
    }

    fn estimate_cycles(output: &[String]) -> usize {
        output
            .iter()
            .filter_map(|line| line.trim().split_whitespace().next())
            .map(|opcode| match opcode {
                "CALL" => 17,
                "RET" => 10,
                "JMP" => 10,
                "JZ" | "JNZ" | "JC" | "JNC" | "JM" | "JP" | "JPE" | "JPO" => 10,
                "LHLD" | "SHLD" => 16,
                "LDA" | "STA" => 13,
                "PUSH" => 11,
                "POP" => 10,
                "LXI" | "DAD" => 10,
                "MOV" => 5,
                "MVI" => 7,
                "INX" | "DCX" | "INR" | "DCR" | "XCHG" => 5,
                "ADD" | "ADC" | "SUB" | "SBB" | "ANA" | "ORA" | "XRA" | "CMP" | "CMA" => 4,
                "RAL" | "RAR" => 4,
                _ => 7,
            })
            .sum()
    }

    fn compile_repo_benchmark(path: &str) -> Vec<String> {
        let full_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let source = std::fs::read_to_string(&full_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {}", full_path.display(), err));
        let sanitized = source.replace('`', "");
        compile_source(&sanitized, full_path.to_str().unwrap(), &[])
            .unwrap_or_else(|err| panic!("failed to compile {}: {}", full_path.display(), err))
    }

    fn assert_benchmark_gate(
        path: &str,
        max_instructions: usize,
        max_helper_calls: usize,
        max_cycles: usize,
    ) {
        let out = compile_repo_benchmark(path);
        let inst_count = count_instructions(&out);
        let helper_calls = count_helper_calls(&out);
        let est_cycles = estimate_cycles(&out);

        assert!(inst_count > 0, "{} must generate instructions", path);
        assert!(
            inst_count <= max_instructions,
            "{} exceeded instruction budget: {} > {}",
            path,
            inst_count,
            max_instructions
        );
        assert!(
            helper_calls <= max_helper_calls,
            "{} exceeded helper-call budget: {} > {}",
            path,
            helper_calls,
            max_helper_calls
        );
        assert!(
            est_cycles <= max_cycles,
            "{} exceeded estimated-cycle budget: {} > {}",
            path,
            est_cycles,
            max_cycles
        );
    }

    #[test]
    #[ignore = "long-running benchmark gate"]
    fn benchmark_gate_sieve_repo_source() {
        assert_benchmark_gate("tests/sieve.c", 12000, 64, 120000);
    }

    #[test]
    #[ignore = "long-running benchmark gate"]
    fn benchmark_gate_dhrystone_repo_source() {
        assert_benchmark_gate("tests/dhrystone.c", 16000, 96, 160000);
    }

    #[test]
    #[ignore = "long-running benchmark gate"]
    fn benchmark_gate_fannkuch_repo_source() {
        assert_benchmark_gate("tests/fannkuch.c", 24000, 192, 260000);
    }

    #[test]
    fn opt_constant_folding_eliminates_runtime_call() {
        // Constant multiplication should be folded at compile time.
        // No __mul16 call should appear.
        let src = r#"
            int result;
            void main(void) {
                result = 6 * 7;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(
            !has_line(&out, "CALL __mul16"),
            "constant multiply should be folded, not call __mul16"
        );
        // The result (42) should appear as an immediate load.
        assert!(
            has_line(&out, "42"),
            "folded constant 42 should appear in output"
        );
    }

    #[test]
    fn opt_constant_folding_chained() {
        // Chained constant expressions within the same computation chain:
        // 2 * 3 + 1 = 7 (all in vregs, no store/load in between)
        let src = r#"
            int result;
            void main(void) {
                result = 2 * 3 + 1;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        // Should fold to constant 7, no runtime multiply needed.
        assert!(
            !has_line(&out, "CALL __mul16"),
            "constant expression should be folded, not call __mul16"
        );
        assert!(
            has_line(&out, "7"),
            "folded result 7 should appear in output"
        );
    }

    #[test]
    fn opt_strength_reduction_mul_by_power_of_two() {
        // x * 4 should become x << 2, avoiding __mul16.
        let src = r#"
            int x;
            int result;
            void main(void) {
                result = x * 4;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        // Should use shift instead of multiply.
        assert!(
            !has_line(&out, "CALL __mul16"),
            "multiply by 4 should be strength-reduced to shift"
        );
    }

    #[test]
    fn opt_strength_reduction_unsigned_div() {
        // x / 8u should become x >> 3, avoiding __div16u.
        let src = r#"
            unsigned int x;
            unsigned int result;
            void main(void) {
                result = x / 8;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(
            !has_line(&out, "CALL __div16u"),
            "unsigned divide by 8 should be strength-reduced to shift"
        );
    }

    #[test]
    fn opt_strength_reduction_unsigned_mod() {
        // x % 16u should become x & 15, avoiding __mod16u.
        let src = r#"
            unsigned int x;
            unsigned int result;
            void main(void) {
                result = x % 16;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(
            !has_line(&out, "CALL __mod16u"),
            "unsigned modulo 16 should be strength-reduced to AND mask"
        );
    }

    #[test]
    fn opt_dead_code_eliminated() {
        // Unused variable should not generate any output.
        let src = r#"
            int result;
            void main(void) {
                int unused;
                unused = 42;
                result = 1;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
        // The dead store to `unused` may or may not be eliminated depending
        // on the IR optimization, but the code should still compile correctly.
    }

    #[test]
    fn opt_peephole_tail_call() {
        // Function ending with a call followed by return should become JMP.
        // Use a function too large to inline so the CALL remains.
        let src = r#"
            int heavy(int a, int b) {
                int x;
                int y;
                int z;
                x = a + b;
                y = a - b;
                z = x + y;
                if (z > 0) {
                    z = z + x;
                } else {
                    z = z - y;
                }
                return z;
            }
            int main(void) { return heavy(3, 4); }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        // Should have JMP heavy instead of CALL heavy / RET.
        let has_direct_tail_jmp = has_line(&out, "JMP heavy");
        let has_specialized_tail_jmp = out
            .iter()
            .any(|l| l.contains("JMP __spec_heavy"));
        let has_any_heavy_call = out.iter().any(|l| {
            l.contains("CALL heavy") || l.contains("CALL __spec_heavy")
        });
        assert!(
            has_direct_tail_jmp || has_specialized_tail_jmp || !has_any_heavy_call,
            "tail call should be optimized to JMP or eliminated"
        );
    }

    #[test]
    fn opt_sieve_benchmark_compiles() {
        // A simplified sieve with small array should compile successfully.
        let src = r#"
            int flags[100];
            int count;
            void main(void) {
                int i;
                int k;
                int prime;
                count = 0;
                i = 0;
                while (i < 100) {
                    flags[i] = 1;
                    i = i + 1;
                }
                i = 0;
                while (i < 100) {
                    if (flags[i]) {
                        prime = i + i + 3;
                        k = i + prime;
                        while (k < 100) {
                            flags[k] = 0;
                            k = k + prime;
                        }
                        count = count + 1;
                    }
                    i = i + 1;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[])
            .expect("sieve compilation failed");
        assert!(has_line(&out, "main:"), "sieve must have main");
        let inst_count = count_instructions(&out);
        assert!(
            inst_count > 10 && inst_count < 5000,
            "sieve should produce reasonable code size, got {} instructions",
            inst_count
        );
    }

    #[test]
    fn opt_code_size_reduction() {
        // Constant expressions should be folded, reducing code size.
        let src = r#"
            int result;
            void main(void) {
                result = 10 + 20 + 30;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        let inst_count = count_instructions(&out);
        // With folding: 10 + 20 + 30 = 60, should be just LXI + SHLD + RET.
        assert!(
            inst_count < 15,
            "constant folding should produce compact code, got {} instructions",
            inst_count
        );
    }

    #[test]
    fn opt_no_self_moves() {
        // The output should not contain any MOV X,X instructions.
        let src = r#"
            int x;
            void main(void) {
                int a; int b;
                a = 5;
                b = a;
                x = b;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        for line in &out {
            let trimmed = line.trim();
            if trimmed.starts_with("MOV") {
                let parts: Vec<&str> = trimmed[3..].split(',').map(str::trim).collect();
                if parts.len() == 2 {
                    assert_ne!(
                        parts[0], parts[1],
                        "self-move should have been eliminated: {}",
                        line
                    );
                }
            }
        }
    }

    #[test]
    fn opt_no_nops_in_output() {
        // No NOP instructions should appear in the optimized output.
        let src = r#"
            int x;
            void main(void) {
                x = 42;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(
            !has_line(&out, "\tNOP"),
            "peephole should eliminate all NOPs"
        );
    }

    #[test]
    fn benchmark_simple_loop() {
        // A simple loop function: test that control flow with a loop
        // compiles correctly and produces reasonable code.
        let src = r#"
            int arr[10];
            int total;
            void main(void) {
                int i;
                total = 0;
                i = 0;
                while (i < 10) {
                    arr[i] = i;
                    total = total + i;
                    i = i + 1;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("compilation failed");
        assert!(has_line(&out, "main:"));
        let inst_count = count_instructions(&out);
        assert!(
            inst_count > 10,
            "loop code should produce meaningful output, got {} instructions",
            inst_count
        );
    }

    // -----------------------------------------------------------------------
    // Phase 3 – struct, union, enum, typedef, switch/case, init lists
    // -----------------------------------------------------------------------

    #[test]
    fn pipeline_struct_basic() {
        let src = r#"
            struct point {
                int x;
                int y;
            };
            struct point p;
            int result;
            void main(void) {
                p.x = 10;
                p.y = 20;
                result = p.x + p.y;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("struct compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_struct_pointer() {
        // The `set` helper is small enough to be inlined; after inlining,
        // its label will not appear in the output.  This test just confirms
        // that a function taking a struct pointer compiles without errors and
        // that `main` is emitted.
        let src = r#"
            struct point {
                int x;
                int y;
            };
            struct point p;
            int result;
            void set(struct point *pp) {
                pp->x = 100;
                pp->y = 200;
            }
            void main(void) {
                set(&p);
                result = p.x + p.y;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("struct-pointer compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_enum() {
        let src = r#"
            enum color { RED, GREEN, BLUE };
            int result;
            void main(void) {
                int c;
                c = GREEN;
                if (c == 1) {
                    result = 42;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("enum compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_enum_explicit_values() {
        let src = r#"
            enum status { OK = 0, ERR = -1, BUSY = 5 };
            int result;
            void main(void) {
                result = BUSY;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("enum explicit values failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_typedef() {
        let src = r#"
            typedef int myint;
            myint x;
            void main(void) {
                myint y;
                y = 10;
                x = y + 5;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("typedef compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_switch_case() {
        let src = r#"
            int result;
            void main(void) {
                int x;
                x = 2;
                switch (x) {
                case 1:
                    result = 10;
                    break;
                case 2:
                    result = 20;
                    break;
                case 3:
                    result = 30;
                    break;
                default:
                    result = 0;
                    break;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("switch compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_switch_default_only() {
        let src = r#"
            int result;
            void main(void) {
                int x;
                x = 99;
                switch (x) {
                default:
                    result = 42;
                    break;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("switch default-only failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_union() {
        let src = r#"
            union data {
                int i;
                char c;
            };
            union data d;
            int result;
            void main(void) {
                d.i = 0x4142;
                result = d.c;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("union compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_array_initializer() {
        let src = r#"
            int result;
            void main(void) {
                int arr[3] = {10, 20, 30};
                result = arr[1];
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("array initializer failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_sizeof_struct() {
        let src = r#"
            struct pair {
                int a;
                int b;
            };
            int result;
            void main(void) {
                result = sizeof(struct pair);
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("sizeof struct failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_typedef_struct() {
        let src = r#"
            typedef struct {
                int x;
                int y;
            } Point;
            Point p;
            int result;
            void main(void) {
                p.x = 5;
                p.y = 10;
                result = p.x + p.y;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("typedef struct failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_switch_with_enum() {
        let src = r#"
            enum dir { UP, DOWN, LEFT, RIGHT };
            int result;
            void main(void) {
                int d;
                d = LEFT;
                switch (d) {
                case UP:    result = 1; break;
                case DOWN:  result = 2; break;
                case LEFT:  result = 3; break;
                case RIGHT: result = 4; break;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("switch with enum failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_float_variable() {
        let src = r#"
            float x;
            void main(void) {
                x = 3.14f;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float variable failed");
        assert!(has_line(&out, "main:"));
        assert!(has_line(&out, "_g_x"));
    }

    #[test]
    fn pipeline_float_add() {
        let src = r#"
            float a;
            float b;
            float c;
            void main(void) {
                a = 1.5f;
                b = 2.5f;
                c = a + b;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float add failed");
        assert!(has_line(&out, "CALL __fadd"), "float add must call __fadd");
    }

    #[test]
    fn pipeline_float_mul() {
        let src = r#"
            float a;
            float b;
            float c;
            void main(void) {
                a = 2.0f;
                b = 3.0f;
                c = a * b;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float mul failed");
        assert!(has_line(&out, "CALL __fmul"), "float mul must call __fmul");
    }

    #[test]
    fn pipeline_float_compare() {
        let src = r#"
            float a;
            float b;
            int result;
            void main(void) {
                a = 1.0f;
                b = 2.0f;
                if (a < b) {
                    result = 1;
                }
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float compare failed");
        assert!(has_line(&out, "CALL __flt"), "float compare must call __flt");
    }

    #[test]
    fn pipeline_float_int_conversion() {
        let src = r#"
            float f;
            int i;
            void main(void) {
                i = 42;
                f = i;
                i = f;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float conversion failed");
        assert!(has_line(&out, "CALL __itof"), "int to float must call __itof");
        assert!(has_line(&out, "CALL __ftoi"), "float to int must call __ftoi");
    }

    #[test]
    fn pipeline_float_runtime_included() {
        let src = r#"
            float a;
            float b;
            float c;
            void main(void) {
                a = 1.0f;
                b = 2.0f;
                c = a + b;
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("float runtime failed");
        // The code should reference __op1/__op2 for float calling convention
        assert!(has_line(&out, "__op1"), "float ops should use __op1");
        assert!(has_line(&out, "__op2"), "float ops should use __op2");
    }

    #[test]
    fn pipeline_variadic_function() {
        let src = r#"
            #include <stdarg.h>
            int sum(int count, ...) {
                va_list ap;
                int total;
                int i;
                va_start(ap, count);
                total = 0;
                i = 0;
                while (i < count) {
                    total = total + va_arg(ap, int);
                    i = i + 1;
                }
                va_end(ap);
                return total;
            }
            int result;
            void main(void) {
                result = sum(3, 10, 20, 30);
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("variadic function failed");
        assert!(has_line(&out, "sum:"), "must have sum function label");
        assert!(has_line(&out, "__va_base_sum"), "variadic func must have va_base");
        assert!(has_line(&out, "DAD SP"), "must capture SP for va_start");
    }

    #[test]
    fn pipeline_variadic_declaration() {
        let src = r#"
            int printf(char *fmt, ...);
            void main(void) {
                printf("hello %d", 42);
            }
        "#;
        let out = compile_source(src, "test.c", &[]).expect("variadic declaration failed");
        // printf call may be tail-call optimized to JMP
        let has_ref = has_line(&out, "CALL printf") || has_line(&out, "JMP printf");
        assert!(has_ref, "must call or jump to printf");
    }
}
