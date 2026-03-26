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
    // 1. Read the source file
    let source = std::fs::read_to_string(input_path)
        .map_err(|e| format!("{}: {}", input_path, e))?;

    // 2. Preprocessor
    let mut pp = preproc::Preprocessor::new();
    let processed = pp
        .preprocess(&source, input_path)
        .map_err(|e| format!("preprocessor error: {}", e.message))?;

    // 3. Lexer
    let tokens = lexer::tokenize(&processed)
        .map_err(|e| format!("{}:{}:{}: {}", input_path, e.line, e.column, e.message))?;

    // 4. Parser
    let program = parser::Parser::new(&tokens)
        .parse()
        .map_err(|errs| {
            errs.iter()
                .map(|e| format!("{}:{}:{}: {}", input_path, e.line, e.column, e.message))
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    // 5. IR generation
    let ir_program = ir_gen::generate(&program).map_err(|errs| {
        errs.iter()
            .map(|e| format!("{}:{}: {}", input_path, e.line, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // 6. Call-graph analysis
    let analysis = callgraph::analyze(&ir_program, None);

    // 7. Code generation
    let code_lines = codegen::generate(&ir_program, &analysis);

    // 8. Peephole optimization
    let optimized = peephole::peephole_optimize(code_lines);

    // 9. Emit
    emit::emit_asm(&optimized, output_path)
        .map_err(|e| format!("{}: {}", output_path, e))?;

    Ok(())
}
