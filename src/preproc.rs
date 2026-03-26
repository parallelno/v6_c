//! C preprocessor for the v6c compiler targeting the Intel 8080.
//!
//! Operates on raw source text before lexing.  Handles:
//! - `#include "file"` and `#include <file>` directives
//! - `#define` / `#undef` for object-like and function-like macros
//! - `#ifdef` / `#ifndef` / `#if` / `#elif` / `#else` / `#endif`
//! - `#error "message"`
//! - Line continuations (backslash-newline)
//! - Nested conditionals and macro expansion in `#if` expressions
//! - Include guard detection and infinite-recursion prevention

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An error produced by the preprocessor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprocError {
    pub message: String,
    pub filename: String,
    pub line: u32,
    pub column: u32,
}

impl PreprocError {
    fn new(message: impl Into<String>, filename: &str, line: u32, column: u32) -> Self {
        Self {
            message: message.into(),
            filename: filename.to_string(),
            line,
            column,
        }
    }
}

impl fmt::Display for PreprocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: error: {}",
            self.filename, self.line, self.column, self.message
        )
    }
}

impl std::error::Error for PreprocError {}

// ---------------------------------------------------------------------------
// Macro definitions
// ---------------------------------------------------------------------------

/// A preprocessor macro definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroDef {
    /// `#define NAME replacement`
    ObjectLike { body: String },
    /// `#define NAME(a, b) replacement`
    FunctionLike { params: Vec<String>, body: String },
}

// ---------------------------------------------------------------------------
// Conditional compilation state
// ---------------------------------------------------------------------------

/// State of one level in the `#if`/`#ifdef` nesting stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CondState {
    /// We are in the active (true) branch and have not yet seen a true branch.
    Active,
    /// We have already found a true branch; skip remaining `#elif`/`#else`.
    SeenTrue,
    /// We are in an inactive branch (condition was false, or parent inactive).
    Inactive,
}

// ---------------------------------------------------------------------------
// Include file resolver
// ---------------------------------------------------------------------------

/// Resolves `#include` paths.  Reads file contents via a pluggable callback
/// so the preprocessor can be tested without real filesystem access.
pub type FileReader = Box<dyn Fn(&Path) -> Result<String, String>>;

/// Default file reader that reads from the real filesystem.
fn default_file_reader(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

/// Maximum `#include` nesting depth.
const MAX_INCLUDE_DEPTH: usize = 64;

/// The C preprocessor.
pub struct Preprocessor {
    /// Defined macros: name → definition.
    macros: HashMap<String, MacroDef>,
    /// Search paths for `#include "file"` (relative) and `#include <file>`.
    include_paths: Vec<PathBuf>,
    /// System include paths for `#include <file>`.
    system_include_paths: Vec<PathBuf>,
    /// Callback to read a file.
    file_reader: FileReader,
}

impl Preprocessor {
    /// Create a new preprocessor with default settings.
    pub fn new() -> Self {
        Self {
            macros: HashMap::new(),
            include_paths: Vec::new(),
            system_include_paths: Vec::new(),
            file_reader: Box::new(default_file_reader),
        }
    }

    /// Add a search path for quoted includes (`#include "file"`).
    pub fn add_include_path(&mut self, path: impl Into<PathBuf>) {
        self.include_paths.push(path.into());
    }

    /// Add a system include path for angle-bracket includes (`#include <file>`).
    pub fn add_system_include_path(&mut self, path: impl Into<PathBuf>) {
        self.system_include_paths.push(path.into());
    }

    /// Pre-define a macro (equivalent to `-DNAME=value` on the command line).
    pub fn define(&mut self, name: &str, value: &str) {
        self.macros.insert(
            name.to_string(),
            MacroDef::ObjectLike {
                body: value.to_string(),
            },
        );
    }

    /// Replace the file reader (for testing without real files).
    pub fn set_file_reader(&mut self, reader: FileReader) {
        self.file_reader = reader;
    }

    /// Preprocess the given source text.
    ///
    /// Returns the fully preprocessed text, or an error if a directive is
    /// malformed, an include cannot be found, etc.
    pub fn preprocess(
        &mut self,
        source: &str,
        filename: &str,
    ) -> Result<String, PreprocError> {
        self.preprocess_impl(source, filename, 0)
    }

    // -- Core implementation --------------------------------------------

    fn preprocess_impl(
        &mut self,
        source: &str,
        filename: &str,
        depth: usize,
    ) -> Result<String, PreprocError> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err(PreprocError::new(
                format!("maximum include depth ({MAX_INCLUDE_DEPTH}) exceeded"),
                filename,
                1,
                1,
            ));
        }

        // Splice line continuations first.
        let source = splice_line_continuations(source);

        let mut output = String::with_capacity(source.len());
        let mut cond_stack: Vec<CondState> = Vec::new();
        let mut line_num: u32 = 1;

        for raw_line in source.split('\n') {
            let current_line = line_num;
            line_num += 1;

            let trimmed = raw_line.trim();

            // Is this a preprocessor directive?
            if let Some(directive) = trimmed.strip_prefix('#') {
                let directive = directive.trim_start();
                self.handle_directive(
                    directive,
                    &mut cond_stack,
                    &mut output,
                    filename,
                    current_line,
                    depth,
                )?;
                // Emit an empty line to keep line numbers in sync.
                output.push('\n');
                continue;
            }

            // Normal line — only emit if we are in an active branch.
            if is_active(&cond_stack) {
                let expanded = self.expand_macros_in_text(raw_line, filename, current_line)?;
                output.push_str(&expanded);
            }
            output.push('\n');
        }

        if !cond_stack.is_empty() {
            return Err(PreprocError::new(
                format!("unterminated conditional directive ({} level(s) still open)", cond_stack.len()),
                filename,
                line_num.saturating_sub(1),
                1,
            ));
        }

        // Remove the trailing newline that our loop always adds.
        if output.ends_with('\n') {
            output.pop();
        }

        Ok(output)
    }

    // -- Directive dispatch ----------------------------------------------

    fn handle_directive(
        &mut self,
        directive: &str,
        cond_stack: &mut Vec<CondState>,
        output: &mut String,
        filename: &str,
        line: u32,
        depth: usize,
    ) -> Result<(), PreprocError> {
        // Even when inactive, we must process conditional directives to track nesting.
        let word_end = directive
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(directive.len());
        let (keyword, rest) = directive.split_at(word_end);
        let rest = rest.trim_start();

        match keyword {
            "ifdef" => self.handle_ifdef(rest, cond_stack, filename, line, false),
            "ifndef" => self.handle_ifdef(rest, cond_stack, filename, line, true),
            "if" => self.handle_if(rest, cond_stack, filename, line),
            "elif" => self.handle_elif(rest, cond_stack, filename, line),
            "else" => self.handle_else(cond_stack, filename, line),
            "endif" => self.handle_endif(cond_stack, filename, line),
            _ => {
                // All other directives are only processed in active branches.
                if !is_active(cond_stack) {
                    return Ok(());
                }
                match keyword {
                    "define" => self.handle_define(rest, filename, line),
                    "undef" => self.handle_undef(rest, filename, line),
                    "include" => self.handle_include(rest, output, filename, line, depth),
                    "error" => self.handle_error(rest, filename, line),
                    "" => Ok(()), // lone `#` on a line is a null directive
                    _ => Err(PreprocError::new(
                        format!("unknown preprocessor directive '#{keyword}'"),
                        filename,
                        line,
                        1,
                    )),
                }
            }
        }
    }

    // -- #define / #undef ------------------------------------------------

    fn handle_define(
        &mut self,
        rest: &str,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        if rest.is_empty() {
            return Err(PreprocError::new(
                "expected macro name after #define",
                filename,
                line,
                1,
            ));
        }

        // Parse the macro name.
        let name_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        let after_name = &rest[name_end..];

        if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            return Err(PreprocError::new(
                format!("invalid macro name '{name}'"),
                filename,
                line,
                1,
            ));
        }

        // Function-like macro: `NAME(` with no space before `(`.
        if after_name.starts_with('(') {
            let close = after_name.find(')').ok_or_else(|| {
                PreprocError::new(
                    "unterminated parameter list in #define",
                    filename,
                    line,
                    1,
                )
            })?;
            let params_str = &after_name[1..close];
            let params: Vec<String> = if params_str.trim().is_empty() {
                Vec::new()
            } else {
                params_str
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .collect()
            };
            // Validate parameter names.
            for p in &params {
                if !p.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_') {
                    return Err(PreprocError::new(
                        format!("invalid parameter name '{p}' in macro '{name}'"),
                        filename,
                        line,
                        1,
                    ));
                }
            }
            let body = after_name[close + 1..].trim().to_string();
            self.macros.insert(
                name.to_string(),
                MacroDef::FunctionLike { params, body },
            );
        } else {
            // Object-like macro.
            let body = after_name.trim().to_string();
            self.macros.insert(
                name.to_string(),
                MacroDef::ObjectLike { body },
            );
        }

        Ok(())
    }

    fn handle_undef(
        &mut self,
        rest: &str,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        let name = rest.split_whitespace().next().ok_or_else(|| {
            PreprocError::new("expected macro name after #undef", filename, line, 1)
        })?;
        self.macros.remove(name);
        Ok(())
    }

    // -- #include --------------------------------------------------------

    fn handle_include(
        &mut self,
        rest: &str,
        output: &mut String,
        filename: &str,
        line: u32,
        depth: usize,
    ) -> Result<(), PreprocError> {
        let rest = rest.trim();
        let (inc_path, is_system) = if rest.starts_with('"') {
            // #include "file"
            let end = rest[1..].find('"').ok_or_else(|| {
                PreprocError::new(
                    "unterminated string in #include directive",
                    filename,
                    line,
                    1,
                )
            })?;
            (&rest[1..1 + end], false)
        } else if rest.starts_with('<') {
            // #include <file>
            let end = rest[1..].find('>').ok_or_else(|| {
                PreprocError::new(
                    "unterminated angle bracket in #include directive",
                    filename,
                    line,
                    1,
                )
            })?;
            (&rest[1..1 + end], true)
        } else {
            return Err(PreprocError::new(
                "expected '\"' or '<' after #include",
                filename,
                line,
                1,
            ));
        };

        let resolved = self.resolve_include(inc_path, filename, is_system).ok_or_else(|| {
            PreprocError::new(
                format!("cannot find include file '{inc_path}'"),
                filename,
                line,
                1,
            )
        })?;

        let contents = (self.file_reader)(&resolved).map_err(|e| {
            PreprocError::new(
                format!("error reading '{inc_path}': {e}"),
                filename,
                line,
                1,
            )
        })?;

        let included = self.preprocess_impl(
            &contents,
            resolved.to_str().unwrap_or(inc_path),
            depth + 1,
        )?;

        output.push_str(&included);
        Ok(())
    }

    fn resolve_include(
        &self,
        inc_path: &str,
        current_file: &str,
        is_system: bool,
    ) -> Option<PathBuf> {
        if !is_system {
            // Try relative to the current file first.
            let current_dir = Path::new(current_file)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            let candidate = current_dir.join(inc_path);
            if self.file_exists(&candidate) {
                return Some(candidate);
            }
            // Then try user include paths.
            for dir in &self.include_paths {
                let candidate = dir.join(inc_path);
                if self.file_exists(&candidate) {
                    return Some(candidate);
                }
            }
        }
        // System include paths.
        for dir in &self.system_include_paths {
            let candidate = dir.join(inc_path);
            if self.file_exists(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    fn file_exists(&self, path: &Path) -> bool {
        // Try reading the file via our reader to support the test harness.
        (self.file_reader)(path).is_ok()
    }

    // -- #ifdef / #ifndef ------------------------------------------------

    fn handle_ifdef(
        &self,
        rest: &str,
        cond_stack: &mut Vec<CondState>,
        filename: &str,
        line: u32,
        negated: bool,
    ) -> Result<(), PreprocError> {
        let name = rest.split_whitespace().next().ok_or_else(|| {
            PreprocError::new(
                format!(
                    "expected macro name after #{}",
                    if negated { "ifndef" } else { "ifdef" }
                ),
                filename,
                line,
                1,
            )
        })?;

        if !is_active(cond_stack) {
            // Parent is inactive; push inactive unconditionally.
            cond_stack.push(CondState::Inactive);
            return Ok(());
        }

        let defined = self.macros.contains_key(name);
        let condition = if negated { !defined } else { defined };

        cond_stack.push(if condition {
            CondState::Active
        } else {
            CondState::Inactive
        });
        Ok(())
    }

    // -- #if / #elif -----------------------------------------------------

    fn handle_if(
        &mut self,
        rest: &str,
        cond_stack: &mut Vec<CondState>,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        if !is_active(cond_stack) {
            cond_stack.push(CondState::Inactive);
            return Ok(());
        }

        let val = self.evaluate_condition(rest, filename, line)?;
        cond_stack.push(if val != 0 {
            CondState::Active
        } else {
            CondState::Inactive
        });
        Ok(())
    }

    fn handle_elif(
        &mut self,
        rest: &str,
        cond_stack: &mut Vec<CondState>,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        let state = cond_stack.last().ok_or_else(|| {
            PreprocError::new("#elif without matching #if", filename, line, 1)
        })?;

        let new_state = match state {
            CondState::Active => {
                // Current branch was taken; skip the rest.
                CondState::SeenTrue
            }
            CondState::SeenTrue => {
                // Already had a true branch; stay skipping.
                CondState::SeenTrue
            }
            CondState::Inactive => {
                // Check if parent is active — we need to look at the stack
                // *without* the current level.
                let parent_active = if cond_stack.len() >= 2 {
                    is_active_slice(&cond_stack[..cond_stack.len() - 1])
                } else {
                    true
                };
                if parent_active {
                    let val = self.evaluate_condition(rest, filename, line)?;
                    if val != 0 {
                        CondState::Active
                    } else {
                        CondState::Inactive
                    }
                } else {
                    CondState::Inactive
                }
            }
        };

        *cond_stack.last_mut().unwrap() = new_state;
        Ok(())
    }

    fn handle_else(
        &self,
        cond_stack: &mut Vec<CondState>,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        let state = cond_stack.last().ok_or_else(|| {
            PreprocError::new("#else without matching #if", filename, line, 1)
        })?;

        let new_state = match state {
            CondState::Active => CondState::SeenTrue,
            CondState::SeenTrue => CondState::SeenTrue,
            CondState::Inactive => {
                let parent_active = if cond_stack.len() >= 2 {
                    is_active_slice(&cond_stack[..cond_stack.len() - 1])
                } else {
                    true
                };
                if parent_active {
                    CondState::Active
                } else {
                    CondState::Inactive
                }
            }
        };

        *cond_stack.last_mut().unwrap() = new_state;
        Ok(())
    }

    fn handle_endif(
        &self,
        cond_stack: &mut Vec<CondState>,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        if cond_stack.pop().is_none() {
            return Err(PreprocError::new(
                "#endif without matching #if",
                filename,
                line,
                1,
            ));
        }
        Ok(())
    }

    // -- #error ----------------------------------------------------------

    fn handle_error(
        &self,
        rest: &str,
        filename: &str,
        line: u32,
    ) -> Result<(), PreprocError> {
        let msg = rest.trim().trim_matches('"');
        Err(PreprocError::new(
            format!("#error: {msg}"),
            filename,
            line,
            1,
        ))
    }

    // -- Macro expansion -------------------------------------------------

    /// Expand all macros in a line of text.
    fn expand_macros_in_text(
        &self,
        text: &str,
        filename: &str,
        line: u32,
    ) -> Result<String, PreprocError> {
        self.expand_macros_impl(text, filename, line, 0)
    }

    fn expand_macros_impl(
        &self,
        text: &str,
        filename: &str,
        line: u32,
        recursion: usize,
    ) -> Result<String, PreprocError> {
        if recursion > 256 {
            return Err(PreprocError::new(
                "macro expansion recursion limit exceeded",
                filename,
                line,
                1,
            ));
        }

        let mut result = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            // Skip string literals.
            if bytes[i] == b'"' {
                result.push('"');
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        result.push(bytes[i] as char);
                        result.push(bytes[i + 1] as char);
                        i += 2;
                    } else {
                        result.push(bytes[i] as char);
                        i += 1;
                    }
                }
                if i < len {
                    result.push('"');
                    i += 1;
                }
                continue;
            }

            // Skip character literals.
            if bytes[i] == b'\'' {
                result.push('\'');
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        result.push(bytes[i] as char);
                        result.push(bytes[i + 1] as char);
                        i += 2;
                    } else {
                        result.push(bytes[i] as char);
                        i += 1;
                    }
                }
                if i < len {
                    result.push('\'');
                    i += 1;
                }
                continue;
            }

            // Skip line comments.
            if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
                // Rest of line is a comment; omit from output.
                break;
            }

            // Skip block comments.
            if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < len {
                    i += 2; // skip */
                }
                result.push(' '); // replace comment with a space
                continue;
            }

            // Identifier — potential macro to expand.
            if is_ident_start(bytes[i]) {
                let start = i;
                while i < len && is_ident_cont(bytes[i]) {
                    i += 1;
                }
                let ident = &text[start..i];

                if let Some(def) = self.macros.get(ident) {
                    match def {
                        MacroDef::ObjectLike { body } => {
                            let expanded =
                                self.expand_macros_impl(body, filename, line, recursion + 1)?;
                            result.push_str(&expanded);
                        }
                        MacroDef::FunctionLike { params, body } => {
                            // Only expand if followed by `(`.
                            let mut j = i;
                            while j < len && bytes[j] == b' ' {
                                j += 1;
                            }
                            if j < len && bytes[j] == b'(' {
                                let (args, end) =
                                    parse_macro_args(&text[j..], filename, line)?;
                                i = j + end;
                                if args.len() != params.len() {
                                    return Err(PreprocError::new(
                                        format!(
                                            "macro '{}' expects {} argument(s), got {}",
                                            ident,
                                            params.len(),
                                            args.len()
                                        ),
                                        filename,
                                        line,
                                        1,
                                    ));
                                }
                                let substituted =
                                    substitute_params(body, params, &args);
                                let expanded = self.expand_macros_impl(
                                    &substituted,
                                    filename,
                                    line,
                                    recursion + 1,
                                )?;
                                result.push_str(&expanded);
                            } else {
                                // No `(` follows — not a macro invocation.
                                result.push_str(ident);
                            }
                        }
                    }
                } else {
                    result.push_str(ident);
                }
                continue;
            }

            // Any other character — pass through.
            result.push(bytes[i] as char);
            i += 1;
        }

        Ok(result)
    }

    // -- Constant expression evaluation for #if / #elif ------------------

    fn evaluate_condition(
        &mut self,
        expr_text: &str,
        filename: &str,
        line: u32,
    ) -> Result<i64, PreprocError> {
        // First expand `defined(NAME)` and `defined NAME`.
        let with_defined = self.expand_defined(expr_text);
        // Then expand remaining macros.
        let expanded = self.expand_macros_in_text(&with_defined, filename, line)?;
        // Replace any remaining identifiers with 0 (per C standard).
        let replaced = replace_undefined_idents(&expanded);
        // Parse and evaluate.
        let mut parser = ExprParser::new(&replaced, filename, line);
        let val = parser.parse_ternary()?;
        Ok(val)
    }

    /// Replace `defined(NAME)` and `defined NAME` with `1` or `0`.
    fn expand_defined(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if i + 7 <= len && &text[i..i + 7] == "defined" {
                let after = i + 7;
                let mut j = after;
                while j < len && bytes[j] == b' ' {
                    j += 1;
                }
                if j < len && bytes[j] == b'(' {
                    // defined(NAME)
                    j += 1;
                    while j < len && bytes[j] == b' ' {
                        j += 1;
                    }
                    let name_start = j;
                    while j < len && is_ident_cont(bytes[j]) {
                        j += 1;
                    }
                    let name = &text[name_start..j];
                    while j < len && bytes[j] == b' ' {
                        j += 1;
                    }
                    if j < len && bytes[j] == b')' {
                        j += 1;
                    }
                    let val = if self.macros.contains_key(name) { "1" } else { "0" };
                    result.push_str(val);
                    i = j;
                    continue;
                } else if j < len && is_ident_start(bytes[j]) {
                    // defined NAME (without parens)
                    let name_start = j;
                    while j < len && is_ident_cont(bytes[j]) {
                        j += 1;
                    }
                    let name = &text[name_start..j];
                    let val = if self.macros.contains_key(name) { "1" } else { "0" };
                    result.push_str(val);
                    i = j;
                    continue;
                }
            }

            // Not a `defined` — check if this is a different identifier
            // (to skip it atomically) or just pass through the byte.
            if is_ident_start(bytes[i]) {
                let start = i;
                while i < len && is_ident_cont(bytes[i]) {
                    i += 1;
                }
                result.push_str(&text[start..i]);
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }

        result
    }
}

impl Default for Preprocessor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Check if the current conditional stack means we are in an active region.
fn is_active(stack: &[CondState]) -> bool {
    is_active_slice(stack)
}

fn is_active_slice(stack: &[CondState]) -> bool {
    stack.iter().all(|s| *s == CondState::Active)
}

/// Splice backslash-newline continuations.
fn splice_line_continuations(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'\\' && i + 1 < len && bytes[i + 1] == b'\n' {
            // Skip backslash-newline; the next line is a continuation.
            i += 2;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Parse the arguments to a function-like macro invocation.
/// `text` starts at the opening `(`.
/// Returns (list of argument strings, number of bytes consumed including `)`).
fn parse_macro_args(
    text: &str,
    filename: &str,
    line: u32,
) -> Result<(Vec<String>, usize), PreprocError> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    debug_assert!(bytes[0] == b'(');

    let mut i = 1; // skip '('
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    while i < len {
        match bytes[i] {
            b'(' => {
                depth += 1;
                current.push('(');
                i += 1;
            }
            b')' if depth == 0 => {
                // End of argument list.
                i += 1; // consume ')'
                // If we have any content or we already have args, push.
                if !current.is_empty() || !args.is_empty() {
                    args.push(current.trim().to_string());
                }
                return Ok((args, i));
            }
            b')' => {
                depth -= 1;
                current.push(')');
                i += 1;
            }
            b',' if depth == 0 => {
                args.push(current.trim().to_string());
                current = String::new();
                i += 1;
            }
            b'"' => {
                // String literal — don't split on commas inside.
                current.push('"');
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        current.push(bytes[i] as char);
                        current.push(bytes[i + 1] as char);
                        i += 2;
                    } else {
                        current.push(bytes[i] as char);
                        i += 1;
                    }
                }
                if i < len {
                    current.push('"');
                    i += 1;
                }
            }
            b'\'' => {
                current.push('\'');
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        current.push(bytes[i] as char);
                        current.push(bytes[i + 1] as char);
                        i += 2;
                    } else {
                        current.push(bytes[i] as char);
                        i += 1;
                    }
                }
                if i < len {
                    current.push('\'');
                    i += 1;
                }
            }
            _ => {
                current.push(bytes[i] as char);
                i += 1;
            }
        }
    }

    Err(PreprocError::new(
        "unterminated macro argument list",
        filename,
        line,
        1,
    ))
}

/// Substitute parameter names with argument values in a macro body.
fn substitute_params(body: &str, params: &[String], args: &[String]) -> String {
    let mut result = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if is_ident_start(bytes[i]) {
            let start = i;
            while i < len && is_ident_cont(bytes[i]) {
                i += 1;
            }
            let ident = &body[start..i];
            if let Some(idx) = params.iter().position(|p| p == ident) {
                result.push_str(&args[idx]);
            } else {
                result.push_str(ident);
            }
        } else if bytes[i] == b'"' {
            // Pass string literals through without substitution.
            result.push('"');
            i += 1;
            while i < len && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < len {
                    result.push(bytes[i] as char);
                    result.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i < len {
                result.push('"');
                i += 1;
            }
        } else if bytes[i] == b'\'' {
            result.push('\'');
            i += 1;
            while i < len && bytes[i] != b'\'' {
                if bytes[i] == b'\\' && i + 1 < len {
                    result.push(bytes[i] as char);
                    result.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i < len {
                result.push('\'');
                i += 1;
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

/// Replace remaining identifiers (not macros) with `0` in a preprocessor
/// constant expression, per the C standard.  Preserves number literals
/// (including hex `0xFF`, octal `077`, suffixes `UL`), and character
/// literals (`'A'`).
fn replace_undefined_idents(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Character literal — pass through unchanged.
        if bytes[i] == b'\'' {
            result.push('\'');
            i += 1;
            while i < len && bytes[i] != b'\'' {
                if bytes[i] == b'\\' && i + 1 < len {
                    result.push(bytes[i] as char);
                    result.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    result.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i < len {
                result.push('\'');
                i += 1;
            }
            continue;
        }

        // Number literal — consume the whole thing including hex prefix,
        // digits, and optional U/L suffix.
        if bytes[i].is_ascii_digit() {
            let start = i;
            if bytes[i] == b'0' && i + 1 < len && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
            {
                // Hex: 0xDIGITS
                i += 2;
                while i < len && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else if bytes[i] == b'0' {
                // Octal or just 0.
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                // Decimal.
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            // Optional suffix: U, L, UL, LU (case-insensitive).
            if i < len && (bytes[i] == b'u' || bytes[i] == b'U') {
                i += 1;
                if i < len && (bytes[i] == b'l' || bytes[i] == b'L') {
                    i += 1;
                }
            } else if i < len && (bytes[i] == b'l' || bytes[i] == b'L') {
                i += 1;
                if i < len && (bytes[i] == b'u' || bytes[i] == b'U') {
                    i += 1;
                }
            }
            result.push_str(&text[start..i]);
            continue;
        }

        // Identifier — replace with 0.
        if is_ident_start(bytes[i]) {
            while i < len && is_ident_cont(bytes[i]) {
                i += 1;
            }
            result.push('0');
            continue;
        }

        // Anything else — pass through.
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

// ---------------------------------------------------------------------------
// Constant expression parser for #if / #elif
// ---------------------------------------------------------------------------

/// Recursive-descent parser for preprocessor constant expressions.
///
/// Supports: integer literals (decimal, hex, octal), `+`, `-`, `*`, `/`, `%`,
/// `<<`, `>>`, `<`, `>`, `<=`, `>=`, `==`, `!=`, `&`, `^`, `|`, `&&`, `||`,
/// `!`, `~`, unary `+`/`-`, parenthesised sub-expressions, ternary `? :`.
struct ExprParser<'a> {
    input: &'a [u8],
    pos: usize,
    filename: &'a str,
    line: u32,
}

impl<'a> ExprParser<'a> {
    fn new(text: &'a str, filename: &'a str, line: u32) -> Self {
        Self {
            input: text.as_bytes(),
            pos: 0,
            filename,
            line,
        }
    }

    fn error(&self, msg: impl Into<String>) -> PreprocError {
        PreprocError::new(msg, self.filename, self.line, (self.pos as u32) + 1)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn peek2(&self) -> Option<u8> {
        if self.pos + 1 < self.input.len() {
            Some(self.input[self.pos + 1])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<u8> {
        if self.pos < self.input.len() {
            let b = self.input[self.pos];
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    // -- Precedence climbing: ternary (lowest) → logical OR → ... → primary

    fn parse_ternary(&mut self) -> Result<i64, PreprocError> {
        let cond = self.parse_logical_or()?;
        self.skip_ws();
        if self.peek() == Some(b'?') {
            self.advance(); // ?
            let then_val = self.parse_ternary()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':' in ternary expression"));
            }
            self.advance(); // :
            let else_val = self.parse_ternary()?;
            Ok(if cond != 0 { then_val } else { else_val })
        } else {
            Ok(cond)
        }
    }

    fn parse_logical_or(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_logical_and()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'|') && self.peek2() == Some(b'|') {
                self.advance();
                self.advance();
                let rhs = self.parse_logical_and()?;
                val = if val != 0 || rhs != 0 { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_logical_and(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_bitor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'&') && self.peek2() == Some(b'&') {
                self.advance();
                self.advance();
                let rhs = self.parse_bitor()?;
                val = if val != 0 && rhs != 0 { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_bitor(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_bitxor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'|') && self.peek2() != Some(b'|') {
                self.advance();
                let rhs = self.parse_bitxor()?;
                val |= rhs;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_bitxor(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_bitand()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'^') {
                self.advance();
                let rhs = self.parse_bitand()?;
                val ^= rhs;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_bitand(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_equality()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'&') && self.peek2() != Some(b'&') {
                self.advance();
                let rhs = self.parse_equality()?;
                val &= rhs;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_equality(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_relational()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'=') && self.peek2() == Some(b'=') {
                self.advance();
                self.advance();
                let rhs = self.parse_relational()?;
                val = if val == rhs { 1 } else { 0 };
            } else if self.peek() == Some(b'!') && self.peek2() == Some(b'=') {
                self.advance();
                self.advance();
                let rhs = self.parse_relational()?;
                val = if val != rhs { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_relational(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_shift()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'<') && self.peek2() == Some(b'=') {
                self.advance();
                self.advance();
                let rhs = self.parse_shift()?;
                val = if val <= rhs { 1 } else { 0 };
            } else if self.peek() == Some(b'>') && self.peek2() == Some(b'=') {
                self.advance();
                self.advance();
                let rhs = self.parse_shift()?;
                val = if val >= rhs { 1 } else { 0 };
            } else if self.peek() == Some(b'<') && self.peek2() != Some(b'<') {
                self.advance();
                let rhs = self.parse_shift()?;
                val = if val < rhs { 1 } else { 0 };
            } else if self.peek() == Some(b'>') && self.peek2() != Some(b'>') {
                self.advance();
                let rhs = self.parse_shift()?;
                val = if val > rhs { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_shift(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_additive()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'<') && self.peek2() == Some(b'<') {
                self.advance();
                self.advance();
                let rhs = self.parse_additive()?;
                val <<= rhs;
            } else if self.peek() == Some(b'>') && self.peek2() == Some(b'>') {
                self.advance();
                self.advance();
                let rhs = self.parse_additive()?;
                val >>= rhs;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_additive(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_multiplicative()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.advance();
                    val += self.parse_multiplicative()?;
                }
                Some(b'-') => {
                    self.advance();
                    val -= self.parse_multiplicative()?;
                }
                _ => break,
            }
        }
        Ok(val)
    }

    fn parse_multiplicative(&mut self) -> Result<i64, PreprocError> {
        let mut val = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.advance();
                    val *= self.parse_unary()?;
                }
                Some(b'/') => {
                    self.advance();
                    let rhs = self.parse_unary()?;
                    if rhs == 0 {
                        return Err(self.error("division by zero in preprocessor expression"));
                    }
                    val /= rhs;
                }
                Some(b'%') => {
                    self.advance();
                    let rhs = self.parse_unary()?;
                    if rhs == 0 {
                        return Err(self.error("modulo by zero in preprocessor expression"));
                    }
                    val %= rhs;
                }
                _ => break,
            }
        }
        Ok(val)
    }

    fn parse_unary(&mut self) -> Result<i64, PreprocError> {
        self.skip_ws();
        match self.peek() {
            Some(b'!') => {
                self.advance();
                let val = self.parse_unary()?;
                Ok(if val == 0 { 1 } else { 0 })
            }
            Some(b'~') => {
                self.advance();
                let val = self.parse_unary()?;
                Ok(!val)
            }
            Some(b'-') => {
                self.advance();
                let val = self.parse_unary()?;
                Ok(-val)
            }
            Some(b'+') => {
                self.advance();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<i64, PreprocError> {
        self.skip_ws();

        // Parenthesised expression.
        if self.peek() == Some(b'(') {
            self.advance();
            let val = self.parse_ternary()?;
            self.skip_ws();
            if self.peek() != Some(b')') {
                return Err(self.error("expected ')' in expression"));
            }
            self.advance();
            return Ok(val);
        }

        // Character literal.
        if self.peek() == Some(b'\'') {
            self.advance(); // opening '
            let val = if self.peek() == Some(b'\\') {
                self.advance(); // backslash
                match self.advance() {
                    Some(b'n') => b'\n' as i64,
                    Some(b't') => b'\t' as i64,
                    Some(b'r') => b'\r' as i64,
                    Some(b'0') => 0,
                    Some(b'\\') => b'\\' as i64,
                    Some(b'\'') => b'\'' as i64,
                    Some(b'"') => b'"' as i64,
                    Some(b'a') => 0x07,
                    Some(b'b') => 0x08,
                    Some(ch) => ch as i64,
                    None => return Err(self.error("unexpected end of character literal")),
                }
            } else {
                match self.advance() {
                    Some(ch) => ch as i64,
                    None => return Err(self.error("empty character literal")),
                }
            };
            if self.peek() == Some(b'\'') {
                self.advance(); // closing '
            }
            return Ok(val);
        }

        // Number literal.
        if let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                return self.parse_number();
            }
        }

        // If we get here, we have an unexpected token.
        if self.pos >= self.input.len() {
            Err(self.error("unexpected end of expression"))
        } else {
            let ch = self.input[self.pos] as char;
            Err(self.error(format!("unexpected character '{ch}' in expression")))
        }
    }

    fn parse_number(&mut self) -> Result<i64, PreprocError> {
        let start = self.pos;

        if self.peek() == Some(b'0') {
            if self.peek2() == Some(b'x') || self.peek2() == Some(b'X') {
                // Hex.
                self.advance(); // 0
                self.advance(); // x
                while self
                    .peek()
                    .map_or(false, |b| b.is_ascii_hexdigit())
                {
                    self.advance();
                }
                // Skip optional suffix.
                self.skip_int_suffix();
                let text = std::str::from_utf8(&self.input[start + 2..self.pos])
                    .unwrap()
                    .trim_end_matches(|c: char| c == 'u' || c == 'U' || c == 'l' || c == 'L');
                return i64::from_str_radix(text, 16)
                    .map_err(|_| self.error(format!("invalid hex literal '0x{text}'")));
            }

            if self.peek2().map_or(false, |b| (b'0'..=b'7').contains(&b)) {
                // Octal.
                self.advance(); // leading 0
                while self
                    .peek()
                    .map_or(false, |b| (b'0'..=b'7').contains(&b))
                {
                    self.advance();
                }
                self.skip_int_suffix();
                let text = std::str::from_utf8(&self.input[start + 1..self.pos])
                    .unwrap()
                    .trim_end_matches(|c: char| c == 'u' || c == 'U' || c == 'l' || c == 'L');
                if text.is_empty() {
                    return Ok(0);
                }
                return i64::from_str_radix(text, 8)
                    .map_err(|_| self.error(format!("invalid octal literal '0{text}'")));
            }
        }

        // Decimal.
        while self.peek().map_or(false, |b| b.is_ascii_digit()) {
            self.advance();
        }
        self.skip_int_suffix();
        let text = std::str::from_utf8(&self.input[start..self.pos])
            .unwrap()
            .trim_end_matches(|c: char| c == 'u' || c == 'U' || c == 'l' || c == 'L');
        text.parse::<i64>()
            .map_err(|_| self.error(format!("invalid integer literal '{text}'")))
    }

    fn skip_int_suffix(&mut self) {
        // Optional U/u, L/l, or combination.
        match self.peek() {
            Some(b'u' | b'U') => {
                self.advance();
                if matches!(self.peek(), Some(b'l' | b'L')) {
                    self.advance();
                }
            }
            Some(b'l' | b'L') => {
                self.advance();
                if matches!(self.peek(), Some(b'u' | b'U')) {
                    self.advance();
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pp() -> Preprocessor {
        Preprocessor::new()
    }

    // -- Helpers --------------------------------------------------------

    /// Preprocess with an empty filename.
    fn run(pp: &mut Preprocessor, src: &str) -> Result<String, PreprocError> {
        pp.preprocess(src, "test.c")
    }

    // -- Basic pass-through ---------------------------------------------

    #[test]
    fn passthrough_no_directives() {
        let mut p = pp();
        let result = run(&mut p, "int x = 1;").unwrap();
        assert_eq!(result, "int x = 1;");
    }

    #[test]
    fn passthrough_multiline() {
        let mut p = pp();
        let src = "int x;\nint y;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "int x;\nint y;");
    }

    // -- Line continuations ---------------------------------------------

    #[test]
    fn line_continuation() {
        let mut p = pp();
        let src = "#define LONG_MACRO 1 + \\\n2 + 3\nint x = LONG_MACRO;";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("1 + 2 + 3"));
    }

    // -- #define / object-like macros -----------------------------------

    #[test]
    fn define_object_like() {
        let mut p = pp();
        let src = "#define WIDTH 80\nint w = WIDTH;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\nint w = 80;");
    }

    #[test]
    fn define_empty_body() {
        let mut p = pp();
        let src = "#define FLAG\nint x = 1;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\nint x = 1;");
    }

    #[test]
    fn define_chain() {
        let mut p = pp();
        let src = "#define A 10\n#define B A\nint x = B;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\n\nint x = 10;");
    }

    // -- #define / function-like macros ---------------------------------

    #[test]
    fn define_function_like() {
        let mut p = pp();
        let src = "#define MAX(a, b) ((a) > (b) ? (a) : (b))\nint x = MAX(3, 5);";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\nint x = ((3) > (5) ? (3) : (5));");
    }

    #[test]
    fn define_function_like_no_args() {
        let mut p = pp();
        let src = "#define FOO() 42\nint x = FOO();";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\nint x = 42;");
    }

    #[test]
    fn function_macro_not_invoked() {
        let mut p = pp();
        let src = "#define FOO(x) (x+1)\nint y = FOO;";
        let result = run(&mut p, src).unwrap();
        // FOO without () should not be expanded.
        assert_eq!(result, "\nint y = FOO;");
    }

    #[test]
    fn function_macro_nested_parens_in_arg() {
        let mut p = pp();
        let src = "#define CALL(f) f\nint x = CALL(func(1, 2));";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\nint x = func(1, 2);");
    }

    #[test]
    fn function_macro_wrong_arg_count() {
        let mut p = pp();
        let src = "#define ADD(a, b) (a+b)\nint x = ADD(1);";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("expects 2"));
    }

    // -- #undef ---------------------------------------------------------

    #[test]
    fn undef_removes_macro() {
        let mut p = pp();
        let src = "#define X 1\n#undef X\nint a = X;";
        let result = run(&mut p, src).unwrap();
        // X should not be expanded after #undef.
        assert_eq!(result, "\n\nint a = X;");
    }

    // -- #ifdef / #ifndef / #endif --------------------------------------

    #[test]
    fn ifdef_true() {
        let mut p = pp();
        let src = "#define FOO\n#ifdef FOO\nint x = 1;\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("int x = 1;"));
    }

    #[test]
    fn ifdef_false() {
        let mut p = pp();
        let src = "#ifdef FOO\nint x = 1;\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("int x = 1;"));
    }

    #[test]
    fn ifndef_true() {
        let mut p = pp();
        let src = "#ifndef FOO\nint x = 1;\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("int x = 1;"));
    }

    #[test]
    fn ifndef_false() {
        let mut p = pp();
        let src = "#define FOO\n#ifndef FOO\nint x = 1;\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("int x = 1;"));
    }

    // -- #if / #elif / #else / #endif -----------------------------------

    #[test]
    fn if_true() {
        let mut p = pp();
        let src = "#if 1\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_false() {
        let mut p = pp();
        let src = "#if 0\nno\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("no"));
    }

    #[test]
    fn if_else() {
        let mut p = pp();
        let src = "#if 0\nno\n#else\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("no"));
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_elif_else() {
        let mut p = pp();
        let src = "#if 0\nfirst\n#elif 1\nsecond\n#else\nthird\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("first"));
        assert!(result.contains("second"));
        assert!(!result.contains("third"));
    }

    #[test]
    fn if_elif_chain_first_true() {
        let mut p = pp();
        let src = "#if 1\nfirst\n#elif 1\nsecond\n#else\nthird\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("first"));
        assert!(!result.contains("second"));
        assert!(!result.contains("third"));
    }

    #[test]
    fn nested_conditionals() {
        let mut p = pp();
        let src = "#if 1\n#if 0\ninner_no\n#else\ninner_yes\n#endif\nouter\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("inner_no"));
        assert!(result.contains("inner_yes"));
        assert!(result.contains("outer"));
    }

    #[test]
    fn nested_ifdef_in_false_branch() {
        let mut p = pp();
        let src = "#if 0\n#ifdef X\ninner\n#endif\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("inner"));
    }

    // -- #if with defined() ---------------------------------------------

    #[test]
    fn if_defined_true() {
        let mut p = pp();
        let src = "#define FOO 1\n#if defined(FOO)\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_defined_false() {
        let mut p = pp();
        let src = "#if defined(BAR)\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("yes"));
    }

    #[test]
    fn if_not_defined() {
        let mut p = pp();
        let src = "#if !defined(BAR)\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_defined_no_parens() {
        let mut p = pp();
        let src = "#define FOO\n#if defined FOO\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    // -- #if with arithmetic -------------------------------------------

    #[test]
    fn if_arithmetic() {
        let mut p = pp();
        let src = "#if (2 + 3) == 5\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_comparison() {
        let mut p = pp();
        let src = "#if 10 > 5\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_logical() {
        let mut p = pp();
        let src = "#if 1 && 0\nno\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("no"));
    }

    #[test]
    fn if_bitwise() {
        let mut p = pp();
        let src = "#if (0xFF & 0x0F) == 0x0F\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_shift() {
        let mut p = pp();
        let src = "#if (1 << 3) == 8\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_ternary() {
        let mut p = pp();
        let src = "#if (1 ? 10 : 20) == 10\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_macro_expanded() {
        let mut p = pp();
        let src = "#define VER 3\n#if VER >= 2\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_hex_literal() {
        let mut p = pp();
        let src = "#if 0x10 == 16\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_octal_literal() {
        let mut p = pp();
        let src = "#if 010 == 8\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_char_literal() {
        let mut p = pp();
        let src = "#if 'A' == 65\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_unary_minus() {
        let mut p = pp();
        let src = "#if -1 < 0\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_logical_not() {
        let mut p = pp();
        let src = "#if !0\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn if_bitwise_not() {
        let mut p = pp();
        // ~0 is -1 in two's complement.
        let src = "#if ~0 == -1\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    // -- #error ---------------------------------------------------------

    #[test]
    fn error_directive() {
        let mut p = pp();
        let src = "#error \"something went wrong\"";
        let result = run(&mut p, src);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("something went wrong"));
    }

    #[test]
    fn error_not_reached() {
        let mut p = pp();
        let src = "#if 0\n#error \"should not fire\"\n#endif\nok";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("ok"));
    }

    // -- Unterminated conditional ----------------------------------------

    #[test]
    fn unterminated_if() {
        let mut p = pp();
        let src = "#if 1\ncode";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("unterminated"));
    }

    #[test]
    fn endif_without_if() {
        let mut p = pp();
        let src = "#endif";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("without matching"));
    }

    // -- #include -------------------------------------------------------

    #[test]
    fn include_quoted() {
        let mut p = pp();
        p.set_file_reader(Box::new(|path: &Path| {
            if path.ends_with("header.h") {
                Ok("int included_var;".to_string())
            } else {
                Err("not found".to_string())
            }
        }));
        let src = "#include \"header.h\"\nint main;";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("int included_var;"));
        assert!(result.contains("int main;"));
    }

    #[test]
    fn include_angle_bracket() {
        let mut p = pp();
        p.add_system_include_path("/usr/include");
        p.set_file_reader(Box::new(|path: &Path| {
            if path == Path::new("/usr/include/stdio.h") {
                Ok("typedef int FILE;".to_string())
            } else {
                Err("not found".to_string())
            }
        }));
        let src = "#include <stdio.h>\nint main;";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("typedef int FILE;"));
    }

    #[test]
    fn include_not_found() {
        let mut p = pp();
        p.set_file_reader(Box::new(|_: &Path| Err("not found".to_string())));
        let src = "#include \"nonexistent.h\"";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("cannot find"));
    }

    #[test]
    fn include_depth_limit() {
        let mut p = pp();
        // Every file includes itself → should hit depth limit.
        p.set_file_reader(Box::new(|_: &Path| {
            Ok("#include \"self.h\"".to_string())
        }));
        let src = "#include \"self.h\"";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("maximum include depth"));
    }

    // -- Include guards -------------------------------------------------

    #[test]
    fn include_guard_pattern() {
        let mut p = pp();
        p.set_file_reader(Box::new(|path: &Path| {
            if path.ends_with("guarded.h") {
                Ok("#ifndef GUARDED_H\n#define GUARDED_H\nint guard_val;\n#endif".to_string())
            } else {
                Err("not found".to_string())
            }
        }));
        let src = "#include \"guarded.h\"\n#include \"guarded.h\"\nint main;";
        let result = run(&mut p, src).unwrap();
        // Should only appear once because the guard prevents re-inclusion.
        let count = result.matches("int guard_val;").count();
        assert_eq!(count, 1);
    }

    // -- Comments in directives -----------------------------------------

    #[test]
    fn comment_in_macro_body() {
        let mut p = pp();
        let src = "#define X 42 /* the answer */\nint y = X;";
        let result = run(&mut p, src).unwrap();
        // The macro body includes the comment text, but when expanded
        // the comment is stripped from the output.
        assert!(result.contains("42"));
    }

    #[test]
    fn line_comment_stripped() {
        let mut p = pp();
        let src = "int x = 1; // comment\nint y = 2;";
        let result = run(&mut p, src).unwrap();
        assert!(!result.contains("// comment"));
        assert!(result.contains("int x = 1;"));
        assert!(result.contains("int y = 2;"));
    }

    #[test]
    fn block_comment_replaced_with_space() {
        let mut p = pp();
        let src = "int /* hello */ x;";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("int"));
        assert!(result.contains("x;"));
        assert!(!result.contains("hello"));
    }

    // -- Macro expansion in string literals (should NOT happen) ----------

    #[test]
    fn no_expansion_in_string() {
        let mut p = pp();
        let src = "#define X 42\nchar *s = \"X is X\";";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("\"X is X\""));
        assert!(!result.contains("\"42 is 42\""));
    }

    #[test]
    fn no_expansion_in_char_literal() {
        let mut p = pp();
        let src = "#define X 42\nchar c = 'X';";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("'X'"));
    }

    // -- Pre-defined macros via API -------------------------------------

    #[test]
    fn predefine_macro() {
        let mut p = pp();
        p.define("VERSION", "3");
        let src = "int v = VERSION;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "int v = 3;");
    }

    // -- Null directive -------------------------------------------------

    #[test]
    fn null_directive() {
        let mut p = pp();
        let src = "#\nint x;";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("int x;"));
    }

    // -- Unknown directive ----------------------------------------------

    #[test]
    fn unknown_directive_is_error() {
        let mut p = pp();
        let src = "#pragma once";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("unknown"));
    }

    // -- Expression parser edge cases -----------------------------------

    #[test]
    fn expr_division_by_zero() {
        let mut p = pp();
        let src = "#if 1/0\nyes\n#endif";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("division by zero"));
    }

    #[test]
    fn expr_modulo_by_zero() {
        let mut p = pp();
        let src = "#if 1%0\nyes\n#endif";
        let result = run(&mut p, src);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("modulo by zero"));
    }

    #[test]
    fn expr_nested_parens() {
        let mut p = pp();
        let src = "#if ((((1))))\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn expr_logical_or() {
        let mut p = pp();
        let src = "#if 0 || 1\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn expr_complex() {
        let mut p = pp();
        // (2 + 3) * 4 == 20 && 1
        let src = "#if (2 + 3) * 4 == 20 && 1\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn expr_precedence() {
        let mut p = pp();
        // 2 + 3 * 4 should be 14, not 20
        let src = "#if 2 + 3 * 4 == 14\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn expr_inequality_operators() {
        let mut p = pp();
        let src = "#if 5 != 3 && 2 <= 2 && 3 >= 3\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    #[test]
    fn expr_undefined_ident_becomes_zero() {
        let mut p = pp();
        // Undefined macro in #if should be replaced with 0.
        let src = "#if UNDEFINED_THING == 0\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    // -- splice_line_continuations ------------------------------------

    #[test]
    fn splice_basic() {
        let result = splice_line_continuations("hello\\\nworld");
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn splice_multiple() {
        let result = splice_line_continuations("a\\\nb\\\nc");
        assert_eq!(result, "abc");
    }

    #[test]
    fn splice_no_continuation() {
        let result = splice_line_continuations("hello\nworld");
        assert_eq!(result, "hello\nworld");
    }

    // -- substitute_params --------------------------------------------

    #[test]
    fn subst_basic() {
        let body = "a + b";
        let params = vec!["a".to_string(), "b".to_string()];
        let args = vec!["1".to_string(), "2".to_string()];
        assert_eq!(substitute_params(body, &params, &args), "1 + 2");
    }

    #[test]
    fn subst_no_expansion_in_string() {
        let body = "\"a\" + b";
        let params = vec!["a".to_string(), "b".to_string()];
        let args = vec!["1".to_string(), "2".to_string()];
        assert_eq!(substitute_params(body, &params, &args), "\"a\" + 2");
    }

    // -- replace_undefined_idents -------------------------------------

    #[test]
    fn replace_idents_basic() {
        assert_eq!(replace_undefined_idents("FOO + 1"), "0 + 1");
    }

    #[test]
    fn replace_idents_numbers_preserved() {
        assert_eq!(replace_undefined_idents("42"), "42");
    }

    #[test]
    fn replace_idents_mixed() {
        assert_eq!(replace_undefined_idents("X && 1 || Y"), "0 && 1 || 0");
    }

    // -- Display for PreprocError -------------------------------------

    #[test]
    fn error_display() {
        let err = PreprocError::new("bad thing", "foo.c", 10, 5);
        assert_eq!(format!("{err}"), "foo.c:10:5: error: bad thing");
    }

    // -- Default trait --------------------------------------------------

    #[test]
    fn default_preprocessor() {
        let mut p = Preprocessor::default();
        let result = run(&mut p, "int x;").unwrap();
        assert_eq!(result, "int x;");
    }

    // -- Integer suffix in #if ----------------------------------------

    #[test]
    fn if_with_suffix() {
        let mut p = pp();
        let src = "#if 1UL == 1\nyes\n#endif";
        let result = run(&mut p, src).unwrap();
        assert!(result.contains("yes"));
    }

    // -- Macro redefinition --------------------------------------------

    #[test]
    fn redefine_macro() {
        let mut p = pp();
        let src = "#define X 1\n#define X 2\nint a = X;";
        let result = run(&mut p, src).unwrap();
        assert_eq!(result, "\n\nint a = 2;");
    }
}
