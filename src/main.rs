mod ast;
mod callgraph;
mod codegen;
mod emit;
mod ir;
mod ir_gen;
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
    let ir_program = ir_gen::generate(&program).map_err(|errs| {
        errs.iter()
            .map(|e| format!("{}:{}: {}", filename, e.line, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

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
}
