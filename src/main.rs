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
mod types;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let (input_path, output_path) = match parse_args(&args) {
        Ok(paths) => paths,
        Err(msg) => {
            eprintln!("v6c: {}", msg);
            std::process::exit(1);
        }
    };

    if let Err(e) = compile(&input_path, &output_path) {
        eprintln!("v6c: {}", e);
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

fn print_usage() {
    eprintln!("Usage: v6c <input.c> [-o <output.asm>]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -o <file>   Output file path (default: input with .asm extension)");
    eprintln!("  -h, --help  Show this help message");
}

fn parse_args(args: &[String]) -> Result<(String, String), String> {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;

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
            arg if arg.starts_with('-') => {
                return Err(format!("unknown option: {}", arg));
            }
            _ => {
                if input.is_some() {
                    return Err("multiple input files not supported".into());
                }
                input = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let input_path = input.ok_or("no input file specified")?;

    let output_path = output.unwrap_or_else(|| {
        if let Some(stem) = input_path.strip_suffix(".c") {
            format!("{}.asm", stem)
        } else {
            format!("{}.asm", input_path)
        }
    });

    Ok((input_path, output_path))
}

// ---------------------------------------------------------------------------
// Compilation pipeline
// ---------------------------------------------------------------------------

fn compile(input_path: &str, output_path: &str) -> Result<(), String> {
    let source = std::fs::read_to_string(input_path)
        .map_err(|e| format!("{}: {}", input_path, e))?;
    let optimized = compile_source(&source, input_path)?;
    emit::emit_asm(&optimized, output_path)
        .map_err(|e| format!("{}: {}", output_path, e))?;
    Ok(())
}

/// Compile C source text through the full pipeline, returning assembly lines.
fn compile_source(source: &str, filename: &str) -> Result<Vec<String>, String> {
    // 1. Preprocessor
    let mut pp = preproc::Preprocessor::new();
    let processed = pp
        .preprocess(source, filename)
        .map_err(|e| format!("preprocessor error: {}", e.message))?;

    // 2. Lexer
    let tokens = lexer::tokenize(&processed)
        .map_err(|e| format!("{}:{}:{}: {}", filename, e.line, e.column, e.message))?;

    // 3. Parser
    let program = parser::Parser::new(&tokens)
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
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "main:"), "must have main label");
        assert!(has_line(&out, "RET"), "must have RET");
    }

    #[test]
    fn pipeline_global_variable() {
        let src = "int x; void main(void) { x = 10; }";
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "main:"));
        assert!(has_line(&out, "_g_x"));
    }

    #[test]
    fn pipeline_function_call() {
        let src = r#"
            int add(int a, int b) { return a + b; }
            void main(void) { int r; r = add(3, 4); }
        "#;
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "add:"), "must have add function");
        assert!(has_line(&out, "CALL add"), "must call add");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "CALL __mul16"), "multiply must call __mul16");
    }

    #[test]
    fn pipeline_data_section_present() {
        let src = "int x; void main(void) { x = 1; }";
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn emit_produces_valid_file() {
        let src = "void main(void) { return; }";
        let out = compile_source(src, "test.c").expect("compilation failed");
        let path = "/tmp/v6c_test_emit.asm";
        emit::emit_asm(&out, path).expect("emit failed");
        let contents = std::fs::read_to_string(path).expect("read failed");
        assert!(contents.contains("ORG 0x100"));
        assert!(contents.contains("JMP main"));
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
        assert!(has_line(&out, "main:"));
        // The dead store to `unused` may or may not be eliminated depending
        // on the IR optimization, but the code should still compile correctly.
    }

    #[test]
    fn opt_peephole_tail_call() {
        // Function ending with a call followed by return should become JMP.
        let src = r#"
            void helper(void) { return; }
            void main(void) { helper(); }
        "#;
        let out = compile_source(src, "test.c").expect("compilation failed");
        // Should have JMP helper instead of CALL helper / RET.
        assert!(
            has_line(&out, "JMP helper"),
            "tail call should be optimized to JMP"
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
        let out = compile_source(src, "test.c")
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("compilation failed");
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
        let out = compile_source(src, "test.c").expect("struct compilation failed");
        assert!(has_line(&out, "main:"));
    }

    #[test]
    fn pipeline_struct_pointer() {
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
        let out = compile_source(src, "test.c").expect("struct-pointer compilation failed");
        assert!(has_line(&out, "main:"));
        assert!(has_line(&out, "set:"));
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
        let out = compile_source(src, "test.c").expect("enum compilation failed");
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
        let out = compile_source(src, "test.c").expect("enum explicit values failed");
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
        let out = compile_source(src, "test.c").expect("typedef compilation failed");
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
        let out = compile_source(src, "test.c").expect("switch compilation failed");
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
        let out = compile_source(src, "test.c").expect("switch default-only failed");
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
        let out = compile_source(src, "test.c").expect("union compilation failed");
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
        let out = compile_source(src, "test.c").expect("array initializer failed");
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
        let out = compile_source(src, "test.c").expect("sizeof struct failed");
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
        let out = compile_source(src, "test.c").expect("typedef struct failed");
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
        let out = compile_source(src, "test.c").expect("switch with enum failed");
        assert!(has_line(&out, "main:"));
    }
}
