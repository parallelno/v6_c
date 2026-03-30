//! Recursive-descent parser for the v6c C compiler targeting Intel 8080.
//!
//! Converts a token stream (from [`crate::lexer`]) into an AST
//! (defined in [`crate::ast`]).  The parser implements the Phase-1 C
//! subset grammar with proper operator precedence, type parsing, and
//! error recovery.

use std::collections::HashMap;
use std::fmt;

use crate::ast::*;
use crate::lexer::{Token, TokenKind};
use crate::types::{CType, StorageClass};

// ---------------------------------------------------------------------------
// Parse error
// ---------------------------------------------------------------------------

/// A single error encountered during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl ParseError {
    fn new(message: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            message: message.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: error: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Recursive-descent parser that produces an AST from a token slice.
pub struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
    errors: Vec<ParseError>,
    /// Typedef aliases: `typedef_name → resolved CType`.
    typedefs: HashMap<String, CType>,
    /// Struct tag definitions: `tag_name → CType::Struct { .. }`.
    struct_tags: HashMap<String, CType>,
    /// Union tag definitions: `tag_name → CType::Union { .. }`.
    union_tags: HashMap<String, CType>,
    /// Enum tag definitions: `tag_name → CType::Enum { .. }`.
    enum_tags: HashMap<String, CType>,
    /// Enum constant values: `enumerator_name → integer_value`.
    enum_constants: HashMap<String, i64>,
    /// Set when a `#pragma unroll` was seen and should apply to the next loop.
    pending_unroll_hint: bool,
    /// Original source text for raw-text extraction (asm blocks).
    source: &'t str,
}

impl<'t> Parser<'t> {
    /// Create a new parser over the given token slice and source text.
    pub fn new(tokens: &'t [Token], source: &'t str) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            typedefs: HashMap::new(),
            struct_tags: HashMap::new(),
            union_tags: HashMap::new(),
            enum_tags: HashMap::new(),
            enum_constants: HashMap::new(),
            pending_unroll_hint: false,
            source,
        }
    }

    /// Parse the entire translation unit and return the AST, or accumulated
    /// errors if any were encountered.
    pub fn parse(mut self) -> Result<Program, Vec<ParseError>> {
        let decls = self.parse_program();
        if self.errors.is_empty() {
            Ok(Program::new(decls, self.enum_constants.clone()))
        } else {
            Err(self.errors)
        }
    }

    // -----------------------------------------------------------------------
    // Token access helpers
    // -----------------------------------------------------------------------

    /// Current token (never panics – returns `Eof` sentinel past end).
    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("token slice must contain Eof"))
    }

    /// Look ahead `n` tokens from the current position.
    fn peek_ahead(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .unwrap_or_else(|| self.tokens.last().expect("token slice must contain Eof"))
    }

    /// `true` if the current token matches `kind`.
    fn check(&self, kind: &TokenKind) -> bool {
        self.peek().kind == *kind
    }

    /// `true` if we have reached the end of input.
    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    /// Advance past the current token and return it.
    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    /// If the current token is `kind`, consume it and return `true`.
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume the current token if it matches `kind`; otherwise record an
    /// error and return `None`.
    fn expect(&mut self, kind: &TokenKind) -> Option<&Token> {
        if self.check(kind) {
            // Safety: we just checked that peek() == kind, so pos is valid.
            let tok = &self.tokens[self.pos];
            self.pos += 1;
            Some(tok)
        } else {
            let tok = self.peek();
            self.error(format!("expected `{}`, found `{}`", kind, tok.kind));
            None
        }
    }

    /// Build a [`SourceLocation`] from the current token.
    fn current_loc(&self) -> SourceLocation {
        let tok = self.peek();
        SourceLocation::new(tok.line, tok.column)
    }

    /// Build a [`SourceLocation`] from an arbitrary token.
    fn loc_of(tok: &Token) -> SourceLocation {
        SourceLocation::new(tok.line, tok.column)
    }

    // -----------------------------------------------------------------------
    // Error helpers
    // -----------------------------------------------------------------------

    /// Record a parse error at the current token position.
    fn error(&mut self, msg: impl Into<String>) {
        let tok = self.peek();
        self.errors
            .push(ParseError::new(msg, tok.line, tok.column));
    }

    /// Skip tokens until we find a likely synchronization point.
    fn synchronize(&mut self) {
        while !self.at_eof() {
            // If we just consumed a semicolon, stop.
            if self.peek().kind == TokenKind::Semicolon {
                self.advance();
                return;
            }
            // Stop before statement-starting keywords.
            match self.peek().kind {
                TokenKind::RBrace
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Do
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Goto
                | TokenKind::Int
                | TokenKind::Char
                | TokenKind::Void
                | TokenKind::Long
                | TokenKind::Short
                | TokenKind::Signed
                | TokenKind::Unsigned
                | TokenKind::Static
                | TokenKind::Extern
                | TokenKind::Auto
                | TokenKind::Register => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Classification helpers
    // -----------------------------------------------------------------------

    /// `true` if `kind` can start a type specifier.
    fn is_type_keyword(kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Void
                | TokenKind::Char
                | TokenKind::Int
                | TokenKind::Long
                | TokenKind::Short
                | TokenKind::Signed
                | TokenKind::Unsigned
                | TokenKind::Const
                | TokenKind::Struct
                | TokenKind::Union
                | TokenKind::Enum
        )
    }

    /// `true` if the token at `offset` positions ahead looks like a type name
    /// (built-in type keyword, struct/union/enum, or typedef name).
    fn looks_like_type_at(&self, offset: usize) -> bool {
        let tok = self.peek_ahead(offset);
        if Self::is_type_keyword(&tok.kind) {
            return true;
        }
        if tok.kind == TokenKind::Ident && self.typedefs.contains_key(&tok.value) {
            return true;
        }
        false
    }

    /// `true` if `kind` is a storage-class specifier.
    fn is_storage_class(kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Static | TokenKind::Extern | TokenKind::Auto | TokenKind::Register
        )
    }

    /// `true` if the current position looks like the start of a declaration
    /// (storage-class, type keyword, or typedef name).
    fn at_declaration_start(&self) -> bool {
        let kind = &self.peek().kind;
        if Self::is_storage_class(kind) {
            return true;
        }
        if Self::is_type_keyword(kind) {
            return true;
        }
        if *kind == TokenKind::Typedef {
            return true;
        }
        // Check if the identifier is a typedef name.
        if *kind == TokenKind::Ident {
            let name = &self.peek().value;
            if self.typedefs.contains_key(name) {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Program (translation unit)
    // -----------------------------------------------------------------------

    fn parse_program(&mut self) -> Vec<TopLevel> {
        let mut decls = Vec::new();
        while !self.at_eof() {
            // Skip stray preprocessor directives.
            if self.check(&TokenKind::PreprocDirective) {
                self.advance();
                continue;
            }
            match self.parse_top_level() {
                Some(tl) => decls.push(tl),
                None => {
                    let pos_before = self.pos;
                    self.synchronize();
                    if self.pos == pos_before {
                        // synchronize() made no progress (e.g. stuck on a
                        // stray `}` which it stops before but doesn't consume).
                        // Forcibly consume the token to avoid an infinite loop.
                        self.advance();
                    }
                }
            }
        }
        decls
    }

    // -----------------------------------------------------------------------
    // Top-level declarations
    // -----------------------------------------------------------------------

    /// Parse one top-level declaration or definition.
    fn parse_top_level(&mut self) -> Option<TopLevel> {
        let loc = self.current_loc();

        // Handle typedef at file scope.
        if self.check(&TokenKind::Typedef) {
            return self.parse_typedef(loc);
        }

        // Optional storage class.
        let storage = self.parse_storage_class();

        // Type specifier.
        let base_type = self.parse_type_specifier()?;

        // If the type specifier consumed a struct/union/enum definition and
        // the next token is `;`, this is a bare type declaration (no variable).
        if self.check(&TokenKind::Semicolon)
            && (base_type.is_struct_or_union() || base_type.is_enum())
        {
            self.advance(); // consume `;`
            return Some(TopLevel::new(TopLevelKind::TypeDecl, loc));
        }

        // Declarator: pointer indirections + name + optional array suffix.
        let (name, full_type) = self.parse_declarator(base_type)?;

        // Function: `(` parameter-list `)` followed by `{` (definition) or `;` (declaration).
        if self.check(&TokenKind::LParen) {
            return self.parse_function(loc, storage, full_type, name);
        }

        // Global variable.
        let init = if self.eat(&TokenKind::Assign) {
            Some(self.parse_initializer()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon);

        Some(TopLevel::new(
            TopLevelKind::GlobalVar {
                name,
                ty: full_type,
                storage,
                init,
            },
            loc,
        ))
    }

    /// Parse the remainder of a function declaration or definition, starting
    /// right before the `(`.
    fn parse_function(
        &mut self,
        loc: SourceLocation,
        storage: Option<StorageClass>,
        return_type: CType,
        name: String,
    ) -> Option<TopLevel> {
        self.expect(&TokenKind::LParen);
        let (params, is_variadic) = self.parse_param_list();
        self.expect(&TokenKind::RParen);

        if self.check(&TokenKind::LBrace) {
            // Function definition.
            let body = self.parse_compound_stmt()?;
            Some(TopLevel::new(
                TopLevelKind::FuncDef {
                    name,
                    return_type,
                    params,
                    storage,
                    body,
                    is_variadic,
                },
                loc,
            ))
        } else {
            // Function declaration (prototype).
            self.expect(&TokenKind::Semicolon);
            Some(TopLevel::new(
                TopLevelKind::FuncDecl {
                    name,
                    return_type,
                    params,
                    storage,
                    is_variadic,
                },
                loc,
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Storage class
    // -----------------------------------------------------------------------

    fn parse_storage_class(&mut self) -> Option<StorageClass> {
        let sc = match self.peek().kind {
            TokenKind::Static => Some(StorageClass::Static),
            TokenKind::Extern => Some(StorageClass::Extern),
            TokenKind::Auto => Some(StorageClass::Auto),
            TokenKind::Register => Some(StorageClass::Register),
            _ => None,
        };
        if sc.is_some() {
            self.advance();
        }
        sc
    }

    // -----------------------------------------------------------------------
    // Type specifier
    // -----------------------------------------------------------------------

    /// Parse a type specifier.  Handles combinations such as `unsigned int`,
    /// `const char`, `signed long`, plain `int`, `struct tag`, `enum tag`,
    /// and typedef names.
    fn parse_type_specifier(&mut self) -> Option<CType> {
        let mut is_const = false;
        let mut is_signed: Option<bool> = None; // None = unspecified
        let mut base: Option<CType> = None;

        // Consume qualifiers / sign specifiers that can appear before the
        // base type keyword.
        loop {
            match self.peek().kind {
                TokenKind::Const => {
                    self.advance();
                    is_const = true;
                }
                TokenKind::Signed => {
                    self.advance();
                    is_signed = Some(true);
                }
                TokenKind::Unsigned => {
                    self.advance();
                    is_signed = Some(false);
                }
                TokenKind::Void => {
                    self.advance();
                    base = Some(CType::Void);
                    break;
                }
                TokenKind::Char => {
                    self.advance();
                    let signed = is_signed.unwrap_or(true);
                    base = Some(CType::Char { signed });
                    break;
                }
                TokenKind::Int => {
                    self.advance();
                    let signed = is_signed.unwrap_or(true);
                    base = Some(CType::Int { signed });
                    break;
                }
                TokenKind::Short => {
                    self.advance();
                    // `short` is treated as `int` on 8080 (both 16-bit).
                    let signed = is_signed.unwrap_or(true);
                    base = Some(CType::Int { signed });
                    // Consume optional trailing `int` keyword.
                    self.eat(&TokenKind::Int);
                    break;
                }
                TokenKind::Long => {
                    self.advance();
                    let signed = is_signed.unwrap_or(true);
                    base = Some(CType::Long { signed });
                    // Consume optional trailing `int` keyword.
                    self.eat(&TokenKind::Int);
                    break;
                }
                TokenKind::Struct | TokenKind::Union => {
                    base = Some(self.parse_struct_or_union()?);
                    break;
                }
                TokenKind::Enum => {
                    base = Some(self.parse_enum_specifier()?);
                    break;
                }
                TokenKind::Float => {
                    self.advance();
                    base = Some(CType::Float);
                    break;
                }
                TokenKind::Ident => {
                    // Check for typedef name.
                    let name = self.peek().value.clone();
                    if let Some(ty) = self.typedefs.get(&name).cloned() {
                        self.advance();
                        base = Some(ty);
                        break;
                    }
                    // Not a typedef name — fall through to error below.
                    break;
                }
                _ => break,
            }
        }

        // Handle bare `signed` / `unsigned` without base → defaults to `int`.
        if base.is_none() {
            if let Some(signed) = is_signed {
                base = Some(CType::Int { signed });
            }
        }

        // `const` without an actual type is meaningless here.
        if base.is_none() && is_const {
            self.error("expected type specifier after `const`");
            return None;
        }

        if base.is_none() {
            self.error(format!(
                "expected type specifier, found `{}`",
                self.peek().kind
            ));
            return None;
        }

        // We track `is_const` for future use (e.g., const-qualified pointers)
        // but the current CType representation does not carry qualifiers, so
        // we intentionally ignore it here.
        let _ = is_const;

        base
    }

    /// Parse a type-name (used in casts and sizeof). This is a type specifier
    /// optionally followed by pointer stars and array brackets.
    fn parse_type_name(&mut self) -> Option<CType> {
        let mut ty = self.parse_type_specifier()?;
        while self.eat(&TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }
        Some(ty)
    }

    // -----------------------------------------------------------------------
    // Declarator
    // -----------------------------------------------------------------------

    /// Parse a declarator: `*`* identifier (`[` constant `]`)*
    ///
    /// Returns `(name, fully-qualified type)`.
    fn parse_declarator(&mut self, base_type: CType) -> Option<(String, CType)> {
        // Pointer indirections.
        let mut ty = base_type;
        while self.eat(&TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }

        // Identifier.
        let name = if self.check(&TokenKind::Ident) {
            let tok = self.advance();
            tok.value.clone()
        } else {
            self.error(format!(
                "expected identifier in declarator, found `{}`",
                self.peek().kind
            ));
            return None;
        };

        // Optional array suffix(es) — supports multi-dimensional arrays.
        while self.eat(&TokenKind::LBracket) {
            let size = self.parse_array_size()?;
            self.expect(&TokenKind::RBracket);
            ty = CType::Array {
                element: Box::new(ty),
                size,
            };
        }

        Some((name, ty))
    }

    /// Parse the integer constant inside `[...]` for an array declaration.
    fn parse_array_size(&mut self) -> Option<usize> {
        if self.check(&TokenKind::IntLiteral) {
            let val_str = self.peek().value.clone();
            self.advance();
            match val_str.parse::<usize>() {
                Ok(n) => Some(n),
                Err(_) => {
                    self.error(format!("invalid array size `{}`", val_str));
                    None
                }
            }
        } else {
            self.error("expected integer constant for array size");
            None
        }
    }

    // -----------------------------------------------------------------------
    // Parameters
    // -----------------------------------------------------------------------

    fn parse_param_list(&mut self) -> (Vec<Param>, bool) {
        let mut params = Vec::new();
        let mut is_variadic = false;

        if self.check(&TokenKind::RParen) || self.at_eof() {
            return (params, false);
        }

        // `void` as the sole parameter means no parameters.
        if self.check(&TokenKind::Void) && self.peek_ahead(1).kind == TokenKind::RParen {
            self.advance(); // consume `void`
            return (params, false);
        }

        loop {
            // Accept `...` (variadic) at the end of the parameter list
            if self.eat(&TokenKind::Ellipsis) {
                is_variadic = true;
                break;
            }
            if let Some(param) = self.parse_param() {
                params.push(param);
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            // Check for `...` after the comma
            if self.check(&TokenKind::Ellipsis) {
                self.advance();
                is_variadic = true;
                break;
            }
        }
        (params, is_variadic)
    }

    fn parse_param(&mut self) -> Option<Param> {
        let base_type = self.parse_type_specifier()?;
        let mut ty = base_type;

        // Pointer indirections.
        while self.eat(&TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }

        // Name is optional in prototypes.
        let name = if self.check(&TokenKind::Ident) {
            let tok = self.advance();
            Some(tok.value.clone())
        } else {
            None
        };

        // Optional array suffix in parameter (`int arr[]` → pointer).
        if self.eat(&TokenKind::LBracket) {
            if !self.check(&TokenKind::RBracket) {
                // sized array in param — still decays to pointer
                let _ = self.parse_array_size();
            }
            self.expect(&TokenKind::RBracket);
            ty = CType::Pointer(Box::new(ty));
        }

        Some(Param { name, ty })
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    fn parse_stmt(&mut self) -> Option<Stmt> {
        while self.check(&TokenKind::PreprocDirective) {
            let text = self.advance().value.clone();
            self.handle_statement_pragma(&text);
        }

        let apply_unroll_hint = self.pending_unroll_hint;
        if apply_unroll_hint {
            self.pending_unroll_hint = false;
        }

        let mut stmt = match self.peek().kind {
            TokenKind::LBrace => self.parse_compound_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::Do => self.parse_do_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Break => self.parse_break_stmt(),
            TokenKind::Continue => self.parse_continue_stmt(),
            TokenKind::Goto => self.parse_goto_stmt(),
            TokenKind::Switch => self.parse_switch_stmt(),
            TokenKind::Case => self.parse_case_stmt(),
            TokenKind::Default => self.parse_default_stmt(),
            TokenKind::Asm => self.parse_asm_block(),
            // Label: `identifier ':'`
            TokenKind::Ident if self.peek_ahead(1).kind == TokenKind::Colon => {
                self.parse_label_stmt()
            }
            _ => self.parse_expr_stmt(),
        }?;

        if apply_unroll_hint {
            match &mut stmt.kind {
                StmtKind::While { unroll_hint, .. }
                | StmtKind::DoWhile { unroll_hint, .. }
                | StmtKind::For { unroll_hint, .. } => {
                    *unroll_hint = true;
                }
                _ => {
                    // `#pragma unroll` only applies to loop statements.
                }
            }
        }

        Some(stmt)
    }

    fn handle_statement_pragma(&mut self, directive_text: &str) {
        let trimmed = directive_text.trim();
        let Some(rest) = trimmed.strip_prefix("pragma") else {
            return;
        };
        let pragma_body = rest.trim();
        if pragma_body.eq_ignore_ascii_case("unroll") {
            self.pending_unroll_hint = true;
        }
    }

    fn parse_compound_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            if self.at_declaration_start() {
                // Handle typedef inside a compound statement.
                if self.check(&TokenKind::Typedef) {
                    let tloc = self.current_loc();
                    self.parse_typedef(tloc);
                    // typedef doesn't produce a statement node — continue.
                    continue;
                }

                match self.parse_local_var_decl() {
                    Some(s) => stmts.push(s),
                    None => self.synchronize(),
                }
            } else {
                match self.parse_stmt() {
                    Some(s) => stmts.push(s),
                    None => self.synchronize(),
                }
            }
        }

        self.expect(&TokenKind::RBrace);
        Some(Stmt::new(StmtKind::Compound(stmts), loc))
    }

    fn parse_local_var_decl(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        let storage = self.parse_storage_class();
        let base_type = self.parse_type_specifier()?;

        // Bare struct/union/enum definition followed by ';' at block scope.
        if self.check(&TokenKind::Semicolon) && base_type.is_struct_or_union() {
            self.advance();
            // No variable declared — produce a dummy empty statement.
            return Some(Stmt::new(StmtKind::Compound(vec![]), loc));
        }

        let (name, ty) = self.parse_declarator(base_type)?;

        let init = if self.eat(&TokenKind::Assign) {
            Some(self.parse_initializer()?)
        } else {
            None
        };

        self.expect(&TokenKind::Semicolon);

        Some(Stmt::new(
            StmtKind::VarDecl {
                name,
                ty,
                storage,
                init,
            },
            loc,
        ))
    }

    fn parse_if_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `if`
        self.expect(&TokenKind::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen);
        let then_body = Box::new(self.parse_stmt()?);

        let else_body = if self.eat(&TokenKind::Else) {
            Some(Box::new(self.parse_stmt()?))
        } else {
            None
        };

        Some(Stmt::new(
            StmtKind::If {
                cond,
                then_body,
                else_body,
            },
            loc,
        ))
    }

    fn parse_while_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `while`
        self.expect(&TokenKind::LParen)?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen);
        let body = Box::new(self.parse_stmt()?);

        Some(Stmt::new(
            StmtKind::While {
                cond,
                body,
                unroll_hint: false,
            },
            loc,
        ))
    }

    fn parse_do_while_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `do`
        let body = Box::new(self.parse_stmt()?);
        self.expect(&TokenKind::While);
        self.expect(&TokenKind::LParen);
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen);
        self.expect(&TokenKind::Semicolon);

        Some(Stmt::new(
            StmtKind::DoWhile {
                body,
                cond,
                unroll_hint: false,
            },
            loc,
        ))
    }

    fn parse_for_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `for`
        self.expect(&TokenKind::LParen)?;

        // init
        let init = if self.check(&TokenKind::Semicolon) {
            self.advance();
            None
        } else if self.at_declaration_start() {
            // C99-style `for (int i = 0; ...)`
            Some(Box::new(self.parse_local_var_decl()?))
        } else {
            let expr = self.parse_expr()?;
            let eloc = expr.loc;
            self.expect(&TokenKind::Semicolon);
            Some(Box::new(Stmt::new(StmtKind::Expr(expr), eloc)))
        };

        // cond
        let cond = if self.check(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&TokenKind::Semicolon);

        // step
        let step = if self.check(&TokenKind::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };

        self.expect(&TokenKind::RParen);
        let body = Box::new(self.parse_stmt()?);

        Some(Stmt::new(
            StmtKind::For {
                init,
                cond,
                step,
                body,
                unroll_hint: false,
            },
            loc,
        ))
    }

    fn parse_return_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `return`

        let value = if self.check(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };

        self.expect(&TokenKind::Semicolon);
        Some(Stmt::new(StmtKind::Return(value), loc))
    }

    fn parse_break_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `break`
        self.expect(&TokenKind::Semicolon);
        Some(Stmt::new(StmtKind::Break, loc))
    }

    fn parse_continue_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `continue`
        self.expect(&TokenKind::Semicolon);
        Some(Stmt::new(StmtKind::Continue, loc))
    }

    fn parse_goto_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `goto`
        let name = if self.check(&TokenKind::Ident) {
            self.advance().value.clone()
        } else {
            self.error("expected label name after `goto`");
            return None;
        };
        self.expect(&TokenKind::Semicolon);
        Some(Stmt::new(StmtKind::Goto(name), loc))
    }

    fn parse_label_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        let name = self.advance().value.clone(); // identifier
        self.advance(); // consume `:`
        let stmt = Box::new(self.parse_stmt()?);
        Some(Stmt::new(StmtKind::Label { name, stmt }, loc))
    }

    // -----------------------------------------------------------------------
    // Switch / Case / Default
    // -----------------------------------------------------------------------

    fn parse_switch_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `switch`
        self.expect(&TokenKind::LParen)?;
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::RParen);
        let body = Box::new(self.parse_stmt()?);
        Some(Stmt::new(StmtKind::Switch { expr, body }, loc))
    }

    fn parse_case_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `case`
        let value = self.parse_const_expr()?;
        self.expect(&TokenKind::Colon);
        let stmt = Box::new(self.parse_stmt()?);
        Some(Stmt::new(StmtKind::Case { value, stmt }, loc))
    }

    fn parse_default_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `default`
        self.expect(&TokenKind::Colon);
        let stmt = Box::new(self.parse_stmt()?);
        Some(Stmt::new(StmtKind::Default { stmt }, loc))
    }

    // -- Inline assembly ------------------------------------------------

    fn parse_asm_block(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        self.advance(); // consume `asm`

        // Parse optional parameter list: asm(int x, char y)
        let mut params = Vec::new();
        let mut return_type = None;
        let has_parens = self.check(&TokenKind::LParen);

        if has_parens {
            self.advance(); // consume '('
            while !self.check(&TokenKind::RParen) && !self.at_eof() {
                let ty = self.parse_type_name()?;
                if !self.check(&TokenKind::Ident) {
                    self.error(format!(
                        "expected variable name in asm parameter, found `{}`",
                        self.peek().kind
                    ));
                    return None;
                }
                let name = self.advance().value.clone();
                params.push((name, ty));
                if !self.check(&TokenKind::RParen) {
                    self.expect(&TokenKind::Comma)?;
                }
            }
            self.expect(&TokenKind::RParen)?;

            // Parse optional return type: -> int
            if self.check(&TokenKind::Arrow) {
                self.advance(); // consume '->'
                return_type = Some(self.parse_type_name()?);
            }
        }

        self.expect(&TokenKind::LBrace)?;
        let code = self.collect_raw_until_matching_brace();

        // Consume optional trailing `;` (asm blocks are compound-like
        // statements but the grammar allows a trailing semicolon).
        self.eat(&TokenKind::Semicolon);

        Some(Stmt::new(StmtKind::AsmBlock { code, params, return_type }, loc))
    }

    /// Collect raw source text from the current position until the matching
    /// closing brace, counting brace depth.  Uses the byte offsets stored in
    /// tokens and the original source text to preserve assembly formatting,
    /// labels, comments, and special characters.
    fn collect_raw_until_matching_brace(&mut self) -> String {
        // The `{` has already been consumed.  The byte offset of the next
        // token (or the current lexer position) tells us where the asm body
        // starts in the source.
        let body_start = self.peek().byte_offset;

        // Walk tokens counting brace depth to find the matching `}`.
        let mut depth: u32 = 1;
        while !self.at_eof() {
            match self.peek().kind {
                TokenKind::LBrace => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        let body_end = self.peek().byte_offset;
                        self.advance(); // consume the closing '}'
                        // Extract from source if available.
                        if !self.source.is_empty() && body_end <= self.source.len() {
                            let raw = &self.source[body_start..body_end];
                            return raw.trim_matches('\n').to_string();
                        }
                        // Fallback: empty.
                        return String::new();
                    }
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
        self.error("unterminated asm block".to_string());
        String::new()
    }

    /// Parse a constant expression (integer literal, enum constant, or simple
    /// arithmetic on constants).  For Phase 3 we support integer literals,
    /// negative literals, and enum constants.
    fn parse_const_expr(&mut self) -> Option<i64> {
        let negate = self.eat(&TokenKind::Minus);
        let val = if self.check(&TokenKind::IntLiteral) {
            self.parse_int_literal()?
        } else if self.check(&TokenKind::CharLiteral) {
            let tok = self.advance();
            if tok.value.is_empty() { 0i64 } else { tok.value.as_bytes()[0] as i64 }
        } else if self.check(&TokenKind::Ident) {
            let name = self.peek().value.clone();
            if let Some(&v) = self.enum_constants.get(&name) {
                self.advance();
                v
            } else {
                self.error(format!("expected constant expression, found `{}`", name));
                return None;
            }
        } else {
            self.error(format!(
                "expected constant expression, found `{}`",
                self.peek().kind
            ));
            return None;
        };
        Some(if negate { -val } else { val })
    }

    // -----------------------------------------------------------------------
    // Struct / Union
    // -----------------------------------------------------------------------

    /// Parse `struct tag { ... }` or `struct tag` or `union tag { ... }` etc.
    fn parse_struct_or_union(&mut self) -> Option<CType> {
        let is_struct = self.check(&TokenKind::Struct);
        self.advance(); // consume `struct` or `union`

        // Optional tag name.
        let tag = if self.check(&TokenKind::Ident) {
            let name = self.advance().value.clone();
            Some(name)
        } else {
            None
        };

        // If `{` follows, parse the member list (definition).
        if self.check(&TokenKind::LBrace) {
            self.advance(); // consume `{`
            let mut members = Vec::new();

            while !self.check(&TokenKind::RBrace) && !self.at_eof() {
                let field_type = self.parse_type_specifier()?;
                let (field_name, field_full_type) = self.parse_declarator(field_type)?;
                self.expect(&TokenKind::Semicolon);
                members.push((field_name, field_full_type));
            }
            self.expect(&TokenKind::RBrace);

            let tag_str = tag.clone().unwrap_or_default();
            let ty = if is_struct {
                CType::Struct {
                    tag: tag_str,
                    members,
                }
            } else {
                CType::Union {
                    tag: tag_str,
                    members,
                }
            };

            // Register the tag if it has a name.
            if let Some(ref t) = tag {
                if is_struct {
                    self.struct_tags.insert(t.clone(), ty.clone());
                } else {
                    self.union_tags.insert(t.clone(), ty.clone());
                }
            }

            Some(ty)
        } else {
            // No `{` — must be a reference to a previously defined tag.
            let tag_name = tag.unwrap_or_else(|| {
                self.error("expected struct/union tag name or `{`");
                String::new()
            });
            let table = if is_struct {
                &self.struct_tags
            } else {
                &self.union_tags
            };
            if let Some(ty) = table.get(&tag_name).cloned() {
                Some(ty)
            } else {
                // Forward reference — create an incomplete struct/union.
                let ty = if is_struct {
                    CType::Struct {
                        tag: tag_name.clone(),
                        members: vec![],
                    }
                } else {
                    CType::Union {
                        tag: tag_name.clone(),
                        members: vec![],
                    }
                };
                Some(ty)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Enum
    // -----------------------------------------------------------------------

    /// Parse `enum tag { ... }` or `enum tag`.
    fn parse_enum_specifier(&mut self) -> Option<CType> {
        self.advance(); // consume `enum`

        // Optional tag name.
        let tag = if self.check(&TokenKind::Ident) {
            let name = self.advance().value.clone();
            Some(name)
        } else {
            None
        };

        // If `{` follows, parse the enumerator list.
        if self.check(&TokenKind::LBrace) {
            self.advance(); // consume `{`
            let mut next_val: i64 = 0;

            while !self.check(&TokenKind::RBrace) && !self.at_eof() {
                if !self.check(&TokenKind::Ident) {
                    self.error("expected enumerator name");
                    break;
                }
                let name = self.advance().value.clone();

                if self.eat(&TokenKind::Assign) {
                    next_val = self.parse_const_expr()?;
                }

                self.enum_constants.insert(name, next_val);
                next_val += 1;

                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace);
        }

        let tag_str = tag.clone().unwrap_or_default();
        let ty = CType::Enum { tag: tag_str };
        if let Some(ref t) = tag {
            self.enum_tags.insert(t.clone(), ty.clone());
        }
        Some(ty)
    }

    // -----------------------------------------------------------------------
    // Typedef
    // -----------------------------------------------------------------------

    /// Parse `typedef type alias ;`.
    fn parse_typedef(&mut self, loc: SourceLocation) -> Option<TopLevel> {
        self.advance(); // consume `typedef`
        let base_type = self.parse_type_specifier()?;

        // Parse pointer indirections + name.
        let mut ty = base_type;
        while self.eat(&TokenKind::Star) {
            ty = CType::Pointer(Box::new(ty));
        }

        let name = if self.check(&TokenKind::Ident) {
            self.advance().value.clone()
        } else {
            self.error("expected typedef name");
            return None;
        };

        // Handle array suffix for typedefs like `typedef int arr_t[10];`
        while self.eat(&TokenKind::LBracket) {
            let size = self.parse_array_size()?;
            self.expect(&TokenKind::RBracket);
            ty = CType::Array {
                element: Box::new(ty),
                size,
            };
        }

        self.expect(&TokenKind::Semicolon);

        // Register the typedef.
        self.typedefs.insert(name.clone(), ty.clone());

        Some(TopLevel::new(TopLevelKind::Typedef { ty, name }, loc))
    }

    // -----------------------------------------------------------------------
    // Initializer (handles both scalar and braced initializer lists)
    // -----------------------------------------------------------------------

    fn parse_initializer(&mut self) -> Option<Expr> {
        if self.check(&TokenKind::LBrace) {
            let loc = self.current_loc();
            self.advance(); // consume `{`
            let mut elements = Vec::new();

            while !self.check(&TokenKind::RBrace) && !self.at_eof() {
                elements.push(self.parse_assignment_expr()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                // Allow trailing comma: `{1, 2, 3,}`
            }
            self.expect(&TokenKind::RBrace);
            Some(Expr::new(ExprKind::InitList(elements), loc))
        } else {
            self.parse_assignment_expr()
        }
    }

    fn parse_expr_stmt(&mut self) -> Option<Stmt> {
        let loc = self.current_loc();
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon);
        Some(Stmt::new(StmtKind::Expr(expr), loc))
    }

    // -----------------------------------------------------------------------
    // Expressions – each precedence level is one method
    // -----------------------------------------------------------------------

    /// Top-level expression: comma operator.
    fn parse_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_assignment_expr()?;

        while self.check(&TokenKind::Comma) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_assignment_expr()?;
            left = Expr::new(
                ExprKind::Comma {
                    left: Box::new(left),
                    right: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Assignment expression (right-associative).
    fn parse_assignment_expr(&mut self) -> Option<Expr> {
        let left = self.parse_conditional_expr()?;

        if let Some(op) = self.match_assign_op() {
            let loc = left.loc;
            self.advance();
            let right = self.parse_assignment_expr()?; // right-associative
            return Some(Expr::new(
                ExprKind::Assign {
                    op,
                    target: Box::new(left),
                    value: Box::new(right),
                },
                loc,
            ));
        }
        Some(left)
    }

    fn match_assign_op(&self) -> Option<AssignOp> {
        match self.peek().kind {
            TokenKind::Assign => Some(AssignOp::Assign),
            TokenKind::PlusEq => Some(AssignOp::AddAssign),
            TokenKind::MinusEq => Some(AssignOp::SubAssign),
            TokenKind::StarEq => Some(AssignOp::MulAssign),
            TokenKind::SlashEq => Some(AssignOp::DivAssign),
            TokenKind::PercentEq => Some(AssignOp::ModAssign),
            TokenKind::AmpEq => Some(AssignOp::BitAndAssign),
            TokenKind::PipeEq => Some(AssignOp::BitOrAssign),
            TokenKind::CaretEq => Some(AssignOp::BitXorAssign),
            TokenKind::LtLtEq => Some(AssignOp::ShlAssign),
            TokenKind::GtGtEq => Some(AssignOp::ShrAssign),
            _ => None,
        }
    }

    /// Conditional / ternary: `cond ? then : else`
    fn parse_conditional_expr(&mut self) -> Option<Expr> {
        let mut expr = self.parse_logical_or_expr()?;

        if self.eat(&TokenKind::Question) {
            let loc = expr.loc;
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon);
            let else_expr = self.parse_conditional_expr()?;
            expr = Expr::new(
                ExprKind::Conditional {
                    cond: Box::new(expr),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
                loc,
            );
        }
        Some(expr)
    }

    /// Logical OR: `||`
    fn parse_logical_or_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_logical_and_expr()?;
        while self.check(&TokenKind::PipePipe) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_logical_and_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::LogOr,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Logical AND: `&&`
    fn parse_logical_and_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_bitwise_or_expr()?;
        while self.check(&TokenKind::AmpAmp) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_bitwise_or_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::LogAnd,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Bitwise OR: `|`
    fn parse_bitwise_or_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_bitwise_xor_expr()?;
        while self.check(&TokenKind::Pipe) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_bitwise_xor_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::BitOr,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Bitwise XOR: `^`
    fn parse_bitwise_xor_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_bitwise_and_expr()?;
        while self.check(&TokenKind::Caret) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_bitwise_and_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::BitXor,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Bitwise AND: `&`
    fn parse_bitwise_and_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_equality_expr()?;
        while self.check(&TokenKind::Amp) {
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_equality_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op: BinOp::BitAnd,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Equality: `==` `!=`
    fn parse_equality_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_relational_expr()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::BangEq => BinOp::Ne,
                _ => break,
            };
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_relational_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Relational: `<` `>` `<=` `>=`
    fn parse_relational_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_shift_expr()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_shift_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Shift: `<<` `>>`
    fn parse_shift_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_additive_expr()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::LtLt => BinOp::Shl,
                TokenKind::GtGt => BinOp::Shr,
                _ => break,
            };
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_additive_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Additive: `+` `-`
    fn parse_additive_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_multiplicative_expr()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_multiplicative_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Multiplicative: `*` `/` `%`
    fn parse_multiplicative_expr(&mut self) -> Option<Expr> {
        let mut left = self.parse_cast_expr()?;
        loop {
            let op = match self.peek().kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            let loc = self.current_loc();
            self.advance();
            let right = self.parse_cast_expr()?;
            left = Expr::new(
                ExprKind::BinOp {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                },
                loc,
            );
        }
        Some(left)
    }

    /// Cast expression: `(type_name) cast_expr` or fall through to unary.
    ///
    /// Ambiguity resolution: if we see `(` followed by a type keyword or
    /// typedef name, treat it as a cast; otherwise it's a parenthesized
    /// expression (handled in `parse_unary_expr` → `parse_primary_expr`).
    fn parse_cast_expr(&mut self) -> Option<Expr> {
        if self.check(&TokenKind::LParen) && self.looks_like_type_at(1) {
            let loc = self.current_loc();
            self.advance(); // consume `(`
            let ty = self.parse_type_name()?;
            self.expect(&TokenKind::RParen);
            let expr = self.parse_cast_expr()?; // right-recursive
            return Some(Expr::new(
                ExprKind::Cast {
                    ty,
                    expr: Box::new(expr),
                },
                loc,
            ));
        }
        self.parse_unary_expr()
    }

    /// Unary expression: prefix operators, sizeof, or postfix expression.
    fn parse_unary_expr(&mut self) -> Option<Expr> {
        let loc = self.current_loc();

        match self.peek().kind {
            TokenKind::PlusPlus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::PreInc,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::MinusMinus => {
                self.advance();
                let operand = self.parse_unary_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::PreDec,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Amp => {
                self.advance();
                let operand = self.parse_cast_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::AddrOf,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_cast_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Deref,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_cast_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Negate,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Tilde => {
                self.advance();
                let operand = self.parse_cast_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::BitNot,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_cast_expr()?;
                Some(Expr::new(
                    ExprKind::UnaryOp {
                        op: UnaryOp::LogNot,
                        operand: Box::new(operand),
                    },
                    loc,
                ))
            }
            TokenKind::Sizeof => {
                self.advance();
                self.parse_sizeof(loc)
            }
            _ => self.parse_postfix_expr(),
        }
    }

    /// Parse `sizeof(type)` or `sizeof expr`.
    fn parse_sizeof(&mut self, loc: SourceLocation) -> Option<Expr> {
        if self.check(&TokenKind::LParen) && self.looks_like_type_at(1) {
            self.advance(); // consume `(`
            let ty = self.parse_type_name()?;
            self.expect(&TokenKind::RParen);
            Some(Expr::new(ExprKind::SizeOf(SizeOfArg::Type(ty)), loc))
        } else {
            let operand = self.parse_unary_expr()?;
            Some(Expr::new(
                ExprKind::SizeOf(SizeOfArg::Expr(Box::new(operand))),
                loc,
            ))
        }
    }

    /// Postfix expression: primary followed by `(args)`, `[index]`, `++`, `--`,
    /// `.member`, `->member`.
    fn parse_postfix_expr(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            match self.peek().kind {
                TokenKind::LParen => {
                    // Function call.
                    let loc = expr.loc;
                    self.advance();
                    let args = self.parse_arg_list()?;
                    self.expect(&TokenKind::RParen);
                    expr = Expr::new(
                        ExprKind::FuncCall {
                            callee: Box::new(expr),
                            args,
                        },
                        loc,
                    );
                }
                TokenKind::LBracket => {
                    // Array subscript.
                    let loc = expr.loc;
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&TokenKind::RBracket);
                    expr = Expr::new(
                        ExprKind::Subscript {
                            array: Box::new(expr),
                            index: Box::new(index),
                        },
                        loc,
                    );
                }
                TokenKind::PlusPlus => {
                    let loc = expr.loc;
                    self.advance();
                    expr = Expr::new(
                        ExprKind::UnaryOp {
                            op: UnaryOp::PostInc,
                            operand: Box::new(expr),
                        },
                        loc,
                    );
                }
                TokenKind::MinusMinus => {
                    let loc = expr.loc;
                    self.advance();
                    expr = Expr::new(
                        ExprKind::UnaryOp {
                            op: UnaryOp::PostDec,
                            operand: Box::new(expr),
                        },
                        loc,
                    );
                }
                TokenKind::Dot => {
                    // Direct member access: `expr.member`
                    let loc = expr.loc;
                    self.advance(); // consume `.`
                    let member = if self.check(&TokenKind::Ident) {
                        self.advance().value.clone()
                    } else {
                        self.error("expected member name after `.`");
                        return None;
                    };
                    expr = Expr::new(
                        ExprKind::MemberAccess {
                            object: Box::new(expr),
                            member,
                        },
                        loc,
                    );
                }
                TokenKind::Arrow => {
                    // Pointer member access: `expr->member`
                    let loc = expr.loc;
                    self.advance(); // consume `->`
                    let member = if self.check(&TokenKind::Ident) {
                        self.advance().value.clone()
                    } else {
                        self.error("expected member name after `->``");
                        return None;
                    };
                    expr = Expr::new(
                        ExprKind::PtrMemberAccess {
                            ptr: Box::new(expr),
                            member,
                        },
                        loc,
                    );
                }
                _ => break,
            }
        }
        Some(expr)
    }

    /// Parse a comma-separated argument list (may be empty).
    fn parse_arg_list(&mut self) -> Option<Vec<Expr>> {
        let mut args = Vec::new();
        if self.check(&TokenKind::RParen) {
            return Some(args);
        }
        loop {
            args.push(self.parse_assignment_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Some(args)
    }

    /// Primary expression: literal, identifier, or parenthesized expression.
    fn parse_primary_expr(&mut self) -> Option<Expr> {
        let tok = self.peek();
        let loc = Self::loc_of(tok);

        match tok.kind {
            TokenKind::IntLiteral => {
                let val = self.parse_int_literal()?;
                Some(Expr::new(ExprKind::IntLiteral(val), loc))
            }
            TokenKind::FloatLiteral => {
                let tok = self.advance();
                // Strip optional 'f'/'F' suffix before parsing.
                let text = tok.value.trim_end_matches(|c| c == 'f' || c == 'F');
                let val: f64 = text.parse().unwrap_or(0.0);
                Some(Expr::new(ExprKind::FloatLiteral(val), loc))
            }
            TokenKind::CharLiteral => {
                let tok = self.advance();
                let ch = if tok.value.is_empty() {
                    0u8
                } else {
                    tok.value.as_bytes()[0]
                };
                Some(Expr::new(ExprKind::CharLiteral(ch), loc))
            }
            TokenKind::StringLiteral => {
                let tok = self.advance();
                Some(Expr::new(
                    ExprKind::StringLiteral(tok.value.as_bytes().to_vec()),
                    loc,
                ))
            }
            TokenKind::Ident => {
                let tok = self.advance();
                Some(Expr::new(ExprKind::Ident(tok.value.clone()), loc))
            }
            TokenKind::LParen => {
                self.advance(); // consume `(`
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen);
                Some(inner)
            }
            _ => {
                self.error(format!("expected expression, found `{}`", tok.kind));
                None
            }
        }
    }

    /// Parse an integer literal value, handling decimal, hex, and octal.
    fn parse_int_literal(&mut self) -> Option<i64> {
        let val_str = self.peek().value.clone();
        self.advance();
        let text = val_str.trim_end_matches(|c: char| c == 'u' || c == 'U' || c == 'l' || c == 'L');

        let val = if text.starts_with("0x") || text.starts_with("0X") {
            i64::from_str_radix(&text[2..], 16)
        } else if text.starts_with('0') && text.len() > 1 {
            i64::from_str_radix(&text[1..], 8)
        } else {
            text.parse::<i64>()
        };

        match val {
            Ok(v) => Some(v),
            Err(_) => {
                self.error(format!("invalid integer literal `{}`", val_str));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{Token, TokenKind};

    /// Helper: build a token.
    fn tok(kind: TokenKind, value: &str, line: u32, col: u32) -> Token {
        Token::new(kind, value, line, col)
    }

    /// Helper: build an Eof sentinel.
    fn eof(line: u32) -> Token {
        Token::new(TokenKind::Eof, "", line, 1)
    }

    /// Convenience: parse tokens and unwrap.
    fn parse_ok(tokens: Vec<Token>) -> Program {
        Parser::new(&tokens, "").parse().expect("parse should succeed")
    }

    /// Convenience: parse tokens and expect errors.
    fn parse_err(tokens: Vec<Token>) -> Vec<ParseError> {
        Parser::new(&tokens, "").parse().expect_err("parse should fail")
    }

    // ===================================================================
    // Function definitions
    // ===================================================================

    #[test]
    fn parse_empty_void_function() {
        // void main() { }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "main", 1, 6),
            tok(TokenKind::LParen, "(", 1, 10),
            tok(TokenKind::RParen, ")", 1, 11),
            tok(TokenKind::LBrace, "{", 1, 13),
            tok(TokenKind::RBrace, "}", 1, 15),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        assert_eq!(prog.decls.len(), 1);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef {
                name,
                return_type,
                params,
                body,
                ..
            } => {
                assert_eq!(name, "main");
                assert_eq!(*return_type, CType::Void);
                assert!(params.is_empty());
                assert!(matches!(body.kind, StmtKind::Compound(ref stmts) if stmts.is_empty()));
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_function_with_params() {
        // int add(int a, int b) { return a + b; }
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "add", 1, 5),
            tok(TokenKind::LParen, "(", 1, 8),
            tok(TokenKind::Int, "int", 1, 9),
            tok(TokenKind::Ident, "a", 1, 13),
            tok(TokenKind::Comma, ",", 1, 14),
            tok(TokenKind::Int, "int", 1, 16),
            tok(TokenKind::Ident, "b", 1, 20),
            tok(TokenKind::RParen, ")", 1, 21),
            tok(TokenKind::LBrace, "{", 1, 23),
            tok(TokenKind::Return, "return", 2, 5),
            tok(TokenKind::Ident, "a", 2, 12),
            tok(TokenKind::Plus, "+", 2, 14),
            tok(TokenKind::Ident, "b", 2, 16),
            tok(TokenKind::Semicolon, ";", 2, 17),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        assert_eq!(prog.decls.len(), 1);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef {
                name,
                return_type,
                params,
                ..
            } => {
                assert_eq!(name, "add");
                assert_eq!(*return_type, CType::int_signed());
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, Some("a".into()));
                assert_eq!(params[0].ty, CType::int_signed());
                assert_eq!(params[1].name, Some("b".into()));
                assert_eq!(params[1].ty, CType::int_signed());
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_function_pointer_param() {
        // void puts(char *s) { }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "puts", 1, 6),
            tok(TokenKind::LParen, "(", 1, 10),
            tok(TokenKind::Char, "char", 1, 11),
            tok(TokenKind::Star, "*", 1, 16),
            tok(TokenKind::Ident, "s", 1, 17),
            tok(TokenKind::RParen, ")", 1, 18),
            tok(TokenKind::LBrace, "{", 1, 20),
            tok(TokenKind::RBrace, "}", 1, 22),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].ty, CType::ptr(CType::char_signed()));
                assert_eq!(params[0].name, Some("s".into()));
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_function_declaration() {
        // int puts(char *s);
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "puts", 1, 5),
            tok(TokenKind::LParen, "(", 1, 9),
            tok(TokenKind::Char, "char", 1, 10),
            tok(TokenKind::Star, "*", 1, 15),
            tok(TokenKind::Ident, "s", 1, 16),
            tok(TokenKind::RParen, ")", 1, 17),
            tok(TokenKind::Semicolon, ";", 1, 18),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        assert_eq!(prog.decls.len(), 1);
        assert!(matches!(prog.decls[0].kind, TopLevelKind::FuncDecl { .. }));
    }

    #[test]
    fn parse_function_void_param() {
        // int getchar(void);
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "getchar", 1, 5),
            tok(TokenKind::LParen, "(", 1, 12),
            tok(TokenKind::Void, "void", 1, 13),
            tok(TokenKind::RParen, ")", 1, 17),
            tok(TokenKind::Semicolon, ";", 1, 18),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDecl { params, .. } => {
                assert!(params.is_empty());
            }
            _ => panic!("expected FuncDecl"),
        }
    }

    // ===================================================================
    // Expression precedence
    // ===================================================================

    /// Helper to parse a single expression from tokens (wraps in `expr ;` context).
    fn parse_single_expr(mut expr_tokens: Vec<Token>) -> Expr {
        let last_line = expr_tokens.last().map(|t| t.line).unwrap_or(1);
        expr_tokens.push(tok(TokenKind::Semicolon, ";", last_line, 99));
        expr_tokens.push(eof(last_line));
        let mut parser = Parser::new(&expr_tokens, "");
        let e = parser.parse_expr().expect("expr parse failed");
        assert!(
            parser.errors.is_empty(),
            "unexpected parse errors: {:?}",
            parser.errors
        );
        e
    }

    #[test]
    fn precedence_add_mul() {
        // 1 + 2 * 3  →  1 + (2 * 3)
        let e = parse_single_expr(vec![
            tok(TokenKind::IntLiteral, "1", 1, 1),
            tok(TokenKind::Plus, "+", 1, 3),
            tok(TokenKind::IntLiteral, "2", 1, 5),
            tok(TokenKind::Star, "*", 1, 7),
            tok(TokenKind::IntLiteral, "3", 1, 9),
        ]);
        // Top node should be Add
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::Add,
                lhs,
                rhs,
            } => {
                assert!(matches!(lhs.kind, ExprKind::IntLiteral(1)));
                assert!(matches!(
                    rhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::Mul,
                        ..
                    }
                ));
            }
            _ => panic!("expected Add at top, got {:?}", e.kind),
        }
    }

    #[test]
    fn precedence_comparison_and_shift() {
        // a << 1 < b  →  (a << 1) < b
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::LtLt, "<<", 1, 3),
            tok(TokenKind::IntLiteral, "1", 1, 6),
            tok(TokenKind::Lt, "<", 1, 8),
            tok(TokenKind::Ident, "b", 1, 10),
        ]);
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::Lt,
                lhs,
                ..
            } => {
                assert!(matches!(
                    lhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::Shl,
                        ..
                    }
                ));
            }
            _ => panic!("expected Lt at top, got {:?}", e.kind),
        }
    }

    #[test]
    fn precedence_logical_and_or() {
        // a && b || c  →  (a && b) || c
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::AmpAmp, "&&", 1, 3),
            tok(TokenKind::Ident, "b", 1, 6),
            tok(TokenKind::PipePipe, "||", 1, 8),
            tok(TokenKind::Ident, "c", 1, 11),
        ]);
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::LogOr,
                lhs,
                rhs,
            } => {
                assert!(matches!(
                    lhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::LogAnd,
                        ..
                    }
                ));
                assert!(matches!(rhs.kind, ExprKind::Ident(ref s) if s == "c"));
            }
            _ => panic!("expected LogOr at top"),
        }
    }

    #[test]
    fn precedence_ternary() {
        // a ? b : c
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::Question, "?", 1, 3),
            tok(TokenKind::Ident, "b", 1, 5),
            tok(TokenKind::Colon, ":", 1, 7),
            tok(TokenKind::Ident, "c", 1, 9),
        ]);
        assert!(matches!(e.kind, ExprKind::Conditional { .. }));
    }

    #[test]
    fn precedence_assignment_right_assoc() {
        // a = b = 1 →  a = (b = 1)
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::Assign, "=", 1, 3),
            tok(TokenKind::Ident, "b", 1, 5),
            tok(TokenKind::Assign, "=", 1, 7),
            tok(TokenKind::IntLiteral, "1", 1, 9),
        ]);
        match &e.kind {
            ExprKind::Assign {
                op: AssignOp::Assign,
                target,
                value,
            } => {
                assert!(matches!(target.kind, ExprKind::Ident(ref s) if s == "a"));
                assert!(matches!(
                    value.kind,
                    ExprKind::Assign {
                        op: AssignOp::Assign,
                        ..
                    }
                ));
            }
            _ => panic!("expected Assign at top"),
        }
    }

    #[test]
    fn parse_comma_expr() {
        // a, b, c  →  (a, b), c  (left-associative)
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::Comma, ",", 1, 2),
            tok(TokenKind::Ident, "b", 1, 4),
            tok(TokenKind::Comma, ",", 1, 5),
            tok(TokenKind::Ident, "c", 1, 7),
        ]);
        match &e.kind {
            ExprKind::Comma { left, right } => {
                assert!(matches!(right.kind, ExprKind::Ident(ref s) if s == "c"));
                assert!(matches!(left.kind, ExprKind::Comma { .. }));
            }
            _ => panic!("expected Comma at top"),
        }
    }

    // ===================================================================
    // Unary expressions
    // ===================================================================

    #[test]
    fn parse_unary_negate() {
        let e = parse_single_expr(vec![
            tok(TokenKind::Minus, "-", 1, 1),
            tok(TokenKind::IntLiteral, "42", 1, 2),
        ]);
        match &e.kind {
            ExprKind::UnaryOp {
                op: UnaryOp::Negate,
                operand,
            } => {
                assert!(matches!(operand.kind, ExprKind::IntLiteral(42)));
            }
            _ => panic!("expected Negate"),
        }
    }

    #[test]
    fn parse_deref_and_addr() {
        // &*p
        let e = parse_single_expr(vec![
            tok(TokenKind::Amp, "&", 1, 1),
            tok(TokenKind::Star, "*", 1, 2),
            tok(TokenKind::Ident, "p", 1, 3),
        ]);
        match &e.kind {
            ExprKind::UnaryOp {
                op: UnaryOp::AddrOf,
                operand,
            } => {
                assert!(matches!(
                    operand.kind,
                    ExprKind::UnaryOp {
                        op: UnaryOp::Deref,
                        ..
                    }
                ));
            }
            _ => panic!("expected AddrOf"),
        }
    }

    #[test]
    fn parse_pre_and_post_increment() {
        // ++x
        let e = parse_single_expr(vec![
            tok(TokenKind::PlusPlus, "++", 1, 1),
            tok(TokenKind::Ident, "x", 1, 3),
        ]);
        assert!(matches!(
            e.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::PreInc,
                ..
            }
        ));

        // x++
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "x", 1, 1),
            tok(TokenKind::PlusPlus, "++", 1, 2),
        ]);
        assert!(matches!(
            e.kind,
            ExprKind::UnaryOp {
                op: UnaryOp::PostInc,
                ..
            }
        ));
    }

    // ===================================================================
    // Cast and sizeof
    // ===================================================================

    #[test]
    fn parse_cast_expr() {
        // (int)x
        let e = parse_single_expr(vec![
            tok(TokenKind::LParen, "(", 1, 1),
            tok(TokenKind::Int, "int", 1, 2),
            tok(TokenKind::RParen, ")", 1, 5),
            tok(TokenKind::Ident, "x", 1, 6),
        ]);
        match &e.kind {
            ExprKind::Cast { ty, expr } => {
                assert_eq!(*ty, CType::int_signed());
                assert!(matches!(expr.kind, ExprKind::Ident(ref s) if s == "x"));
            }
            _ => panic!("expected Cast, got {:?}", e.kind),
        }
    }

    #[test]
    fn parse_cast_pointer() {
        // (char *)p
        let e = parse_single_expr(vec![
            tok(TokenKind::LParen, "(", 1, 1),
            tok(TokenKind::Char, "char", 1, 2),
            tok(TokenKind::Star, "*", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::Ident, "p", 1, 9),
        ]);
        match &e.kind {
            ExprKind::Cast { ty, .. } => {
                assert_eq!(*ty, CType::ptr(CType::char_signed()));
            }
            _ => panic!("expected Cast"),
        }
    }

    #[test]
    fn parse_sizeof_type() {
        // sizeof(int)
        let e = parse_single_expr(vec![
            tok(TokenKind::Sizeof, "sizeof", 1, 1),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::Int, "int", 1, 8),
            tok(TokenKind::RParen, ")", 1, 11),
        ]);
        match &e.kind {
            ExprKind::SizeOf(SizeOfArg::Type(ty)) => {
                assert_eq!(*ty, CType::int_signed());
            }
            _ => panic!("expected SizeOf(Type)"),
        }
    }

    #[test]
    fn parse_sizeof_expr() {
        // sizeof x
        let e = parse_single_expr(vec![
            tok(TokenKind::Sizeof, "sizeof", 1, 1),
            tok(TokenKind::Ident, "x", 1, 8),
        ]);
        match &e.kind {
            ExprKind::SizeOf(SizeOfArg::Expr(inner)) => {
                assert!(matches!(inner.kind, ExprKind::Ident(ref s) if s == "x"));
            }
            _ => panic!("expected SizeOf(Expr)"),
        }
    }

    // ===================================================================
    // Postfix expressions
    // ===================================================================

    #[test]
    fn parse_function_call() {
        // foo(1, 2)
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "foo", 1, 1),
            tok(TokenKind::LParen, "(", 1, 4),
            tok(TokenKind::IntLiteral, "1", 1, 5),
            tok(TokenKind::Comma, ",", 1, 6),
            tok(TokenKind::IntLiteral, "2", 1, 8),
            tok(TokenKind::RParen, ")", 1, 9),
        ]);
        match &e.kind {
            ExprKind::FuncCall { callee, args } => {
                assert!(matches!(callee.kind, ExprKind::Ident(ref s) if s == "foo"));
                assert_eq!(args.len(), 2);
            }
            _ => panic!("expected FuncCall"),
        }
    }

    #[test]
    fn parse_subscript() {
        // arr[i]
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "arr", 1, 1),
            tok(TokenKind::LBracket, "[", 1, 4),
            tok(TokenKind::Ident, "i", 1, 5),
            tok(TokenKind::RBracket, "]", 1, 6),
        ]);
        assert!(matches!(e.kind, ExprKind::Subscript { .. }));
    }

    // ===================================================================
    // Control flow statements
    // ===================================================================

    #[test]
    fn parse_if_else() {
        // void f() { if (x) return 1; else return 0; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::If, "if", 2, 5),
            tok(TokenKind::LParen, "(", 2, 8),
            tok(TokenKind::Ident, "x", 2, 9),
            tok(TokenKind::RParen, ")", 2, 10),
            tok(TokenKind::Return, "return", 2, 12),
            tok(TokenKind::IntLiteral, "1", 2, 19),
            tok(TokenKind::Semicolon, ";", 2, 20),
            tok(TokenKind::Else, "else", 3, 5),
            tok(TokenKind::Return, "return", 3, 10),
            tok(TokenKind::IntLiteral, "0", 3, 17),
            tok(TokenKind::Semicolon, ";", 3, 18),
            tok(TokenKind::RBrace, "}", 4, 1),
            eof(4),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert_eq!(stmts.len(), 1);
                    match &stmts[0].kind {
                        StmtKind::If { else_body, .. } => {
                            assert!(else_body.is_some());
                        }
                        _ => panic!("expected If"),
                    }
                } else {
                    panic!("expected Compound body");
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_while_loop() {
        // void f() { while (i) i--; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::While, "while", 2, 5),
            tok(TokenKind::LParen, "(", 2, 11),
            tok(TokenKind::Ident, "i", 2, 12),
            tok(TokenKind::RParen, ")", 2, 13),
            tok(TokenKind::Ident, "i", 2, 15),
            tok(TokenKind::MinusMinus, "--", 2, 16),
            tok(TokenKind::Semicolon, ";", 2, 18),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert!(matches!(stmts[0].kind, StmtKind::While { .. }));
                } else {
                    panic!("expected Compound");
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_pragma_unroll_before_for() {
        // void f() { #pragma unroll for (;;) { } }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::PreprocDirective, "pragma unroll", 2, 5),
            tok(TokenKind::For, "for", 3, 5),
            tok(TokenKind::LParen, "(", 3, 9),
            tok(TokenKind::Semicolon, ";", 3, 10),
            tok(TokenKind::Semicolon, ";", 3, 11),
            tok(TokenKind::RParen, ")", 3, 12),
            tok(TokenKind::LBrace, "{", 3, 14),
            tok(TokenKind::RBrace, "}", 3, 16),
            tok(TokenKind::RBrace, "}", 4, 1),
            eof(4),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    match &stmts[0].kind {
                        StmtKind::For { unroll_hint, .. } => assert!(*unroll_hint),
                        _ => panic!("expected For"),
                    }
                } else {
                    panic!("expected Compound");
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_do_while() {
        // void f() { do x++; while (x < 10); }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::Do, "do", 2, 5),
            tok(TokenKind::Ident, "x", 2, 8),
            tok(TokenKind::PlusPlus, "++", 2, 9),
            tok(TokenKind::Semicolon, ";", 2, 11),
            tok(TokenKind::While, "while", 2, 13),
            tok(TokenKind::LParen, "(", 2, 19),
            tok(TokenKind::Ident, "x", 2, 20),
            tok(TokenKind::Lt, "<", 2, 22),
            tok(TokenKind::IntLiteral, "10", 2, 24),
            tok(TokenKind::RParen, ")", 2, 26),
            tok(TokenKind::Semicolon, ";", 2, 27),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert!(matches!(stmts[0].kind, StmtKind::DoWhile { .. }));
                } else {
                    panic!("expected Compound");
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_for_loop() {
        // void f() { for (int i = 0; i < 10; i++) { } }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::For, "for", 2, 5),
            tok(TokenKind::LParen, "(", 2, 9),
            tok(TokenKind::Int, "int", 2, 10),
            tok(TokenKind::Ident, "i", 2, 14),
            tok(TokenKind::Assign, "=", 2, 16),
            tok(TokenKind::IntLiteral, "0", 2, 18),
            tok(TokenKind::Semicolon, ";", 2, 19),
            tok(TokenKind::Ident, "i", 2, 21),
            tok(TokenKind::Lt, "<", 2, 23),
            tok(TokenKind::IntLiteral, "10", 2, 25),
            tok(TokenKind::Semicolon, ";", 2, 27),
            tok(TokenKind::Ident, "i", 2, 29),
            tok(TokenKind::PlusPlus, "++", 2, 30),
            tok(TokenKind::RParen, ")", 2, 32),
            tok(TokenKind::LBrace, "{", 2, 34),
            tok(TokenKind::RBrace, "}", 2, 36),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    match &stmts[0].kind {
                        StmtKind::For {
                            init,
                            cond,
                            step,
                            ..
                        } => {
                            assert!(init.is_some());
                            assert!(cond.is_some());
                            assert!(step.is_some());
                        }
                        _ => panic!("expected For"),
                    }
                } else {
                    panic!("expected Compound");
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_for_empty_clauses() {
        // void f() { for (;;) { } }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::For, "for", 2, 5),
            tok(TokenKind::LParen, "(", 2, 9),
            tok(TokenKind::Semicolon, ";", 2, 10),
            tok(TokenKind::Semicolon, ";", 2, 11),
            tok(TokenKind::RParen, ")", 2, 12),
            tok(TokenKind::LBrace, "{", 2, 14),
            tok(TokenKind::RBrace, "}", 2, 16),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    match &stmts[0].kind {
                        StmtKind::For {
                            init, cond, step, ..
                        } => {
                            assert!(init.is_none());
                            assert!(cond.is_none());
                            assert!(step.is_none());
                        }
                        _ => panic!("expected For"),
                    }
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_goto_and_label() {
        // void f() { goto end; end: return; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::Goto, "goto", 2, 5),
            tok(TokenKind::Ident, "end", 2, 10),
            tok(TokenKind::Semicolon, ";", 2, 13),
            tok(TokenKind::Ident, "end", 3, 5),
            tok(TokenKind::Colon, ":", 3, 8),
            tok(TokenKind::Return, "return", 3, 10),
            tok(TokenKind::Semicolon, ";", 3, 16),
            tok(TokenKind::RBrace, "}", 4, 1),
            eof(4),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert_eq!(stmts.len(), 2);
                    assert!(matches!(stmts[0].kind, StmtKind::Goto(ref s) if s == "end"));
                    assert!(matches!(
                        stmts[1].kind,
                        StmtKind::Label {
                            ref name,
                            ..
                        } if name == "end"
                    ));
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_break_continue() {
        // void f() { break; continue; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::Break, "break", 2, 5),
            tok(TokenKind::Semicolon, ";", 2, 10),
            tok(TokenKind::Continue, "continue", 3, 5),
            tok(TokenKind::Semicolon, ";", 3, 13),
            tok(TokenKind::RBrace, "}", 4, 1),
            eof(4),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert_eq!(stmts.len(), 2);
                    assert!(matches!(stmts[0].kind, StmtKind::Break));
                    assert!(matches!(stmts[1].kind, StmtKind::Continue));
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    // ===================================================================
    // Variable declarations
    // ===================================================================

    #[test]
    fn parse_global_var() {
        // int counter;
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "counter", 1, 5),
            tok(TokenKind::Semicolon, ";", 1, 12),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar {
                name,
                ty,
                storage,
                init,
            } => {
                assert_eq!(name, "counter");
                assert_eq!(*ty, CType::int_signed());
                assert!(storage.is_none());
                assert!(init.is_none());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_global_var_with_init() {
        // static int x = 42;
        let tokens = vec![
            tok(TokenKind::Static, "static", 1, 1),
            tok(TokenKind::Int, "int", 1, 8),
            tok(TokenKind::Ident, "x", 1, 12),
            tok(TokenKind::Assign, "=", 1, 14),
            tok(TokenKind::IntLiteral, "42", 1, 16),
            tok(TokenKind::Semicolon, ";", 1, 18),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar {
                name,
                storage,
                init,
                ..
            } => {
                assert_eq!(name, "x");
                assert_eq!(*storage, Some(StorageClass::Static));
                assert!(init.is_some());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_global_array() {
        // char buf[16];
        let tokens = vec![
            tok(TokenKind::Char, "char", 1, 1),
            tok(TokenKind::Ident, "buf", 1, 6),
            tok(TokenKind::LBracket, "[", 1, 9),
            tok(TokenKind::IntLiteral, "16", 1, 10),
            tok(TokenKind::RBracket, "]", 1, 12),
            tok(TokenKind::Semicolon, ";", 1, 13),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { name, ty, .. } => {
                assert_eq!(name, "buf");
                assert_eq!(*ty, CType::array(CType::char_signed(), 16));
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_local_var_decl() {
        // void f() { int x = 5; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::Int, "int", 2, 5),
            tok(TokenKind::Ident, "x", 2, 9),
            tok(TokenKind::Assign, "=", 2, 11),
            tok(TokenKind::IntLiteral, "5", 2, 13),
            tok(TokenKind::Semicolon, ";", 2, 14),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef { body, .. } => {
                if let StmtKind::Compound(ref stmts) = body.kind {
                    assert_eq!(stmts.len(), 1);
                    match &stmts[0].kind {
                        StmtKind::VarDecl {
                            name, ty, init, ..
                        } => {
                            assert_eq!(name, "x");
                            assert_eq!(*ty, CType::int_signed());
                            assert!(init.is_some());
                        }
                        _ => panic!("expected VarDecl"),
                    }
                }
            }
            _ => panic!("expected FuncDef"),
        }
    }

    // ===================================================================
    // Type parsing
    // ===================================================================

    #[test]
    fn parse_unsigned_int_type() {
        // unsigned int x;
        let tokens = vec![
            tok(TokenKind::Unsigned, "unsigned", 1, 1),
            tok(TokenKind::Int, "int", 1, 10),
            tok(TokenKind::Ident, "x", 1, 14),
            tok(TokenKind::Semicolon, ";", 1, 15),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::int_unsigned());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_pointer_type() {
        // int *p;
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Star, "*", 1, 5),
            tok(TokenKind::Ident, "p", 1, 6),
            tok(TokenKind::Semicolon, ";", 1, 7),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::ptr(CType::int_signed()));
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_const_char_pointer() {
        // const char *msg;
        let tokens = vec![
            tok(TokenKind::Const, "const", 1, 1),
            tok(TokenKind::Char, "char", 1, 7),
            tok(TokenKind::Star, "*", 1, 12),
            tok(TokenKind::Ident, "msg", 1, 13),
            tok(TokenKind::Semicolon, ";", 1, 16),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::ptr(CType::char_signed()));
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_signed_long_type() {
        // signed long val;
        let tokens = vec![
            tok(TokenKind::Signed, "signed", 1, 1),
            tok(TokenKind::Long, "long", 1, 8),
            tok(TokenKind::Ident, "val", 1, 13),
            tok(TokenKind::Semicolon, ";", 1, 16),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::long_signed());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_bare_unsigned() {
        // unsigned x;  → unsigned int
        let tokens = vec![
            tok(TokenKind::Unsigned, "unsigned", 1, 1),
            tok(TokenKind::Ident, "x", 1, 10),
            tok(TokenKind::Semicolon, ";", 1, 11),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::int_unsigned());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    #[test]
    fn parse_short_int_type() {
        // short int n;  → int (16-bit on 8080)
        let tokens = vec![
            tok(TokenKind::Short, "short", 1, 1),
            tok(TokenKind::Int, "int", 1, 7),
            tok(TokenKind::Ident, "n", 1, 11),
            tok(TokenKind::Semicolon, ";", 1, 12),
            eof(1),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::GlobalVar { ty, .. } => {
                assert_eq!(*ty, CType::int_signed());
            }
            _ => panic!("expected GlobalVar"),
        }
    }

    // ===================================================================
    // Integer literal formats
    // ===================================================================

    #[test]
    fn parse_hex_literal() {
        let e = parse_single_expr(vec![tok(TokenKind::IntLiteral, "0xFF", 1, 1)]);
        assert!(matches!(e.kind, ExprKind::IntLiteral(255)));
    }

    #[test]
    fn parse_octal_literal() {
        let e = parse_single_expr(vec![tok(TokenKind::IntLiteral, "010", 1, 1)]);
        assert!(matches!(e.kind, ExprKind::IntLiteral(8)));
    }

    // ===================================================================
    // Error recovery
    // ===================================================================

    #[test]
    fn error_recovery_missing_semicolon() {
        // int x     ← missing semicolon
        // int y;    ← should still parse
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "x", 1, 5),
            // no semicolon → error, synchronize
            tok(TokenKind::Int, "int", 2, 1),
            tok(TokenKind::Ident, "y", 2, 5),
            tok(TokenKind::Semicolon, ";", 2, 6),
            eof(2),
        ];
        let errs = parse_err(tokens);
        // There should be at least one error about the missing `;`.
        assert!(!errs.is_empty());
    }

    #[test]
    fn error_recovery_bad_expression() {
        // void f() { + ; return 0; }
        let tokens = vec![
            tok(TokenKind::Void, "void", 1, 1),
            tok(TokenKind::Ident, "f", 1, 6),
            tok(TokenKind::LParen, "(", 1, 7),
            tok(TokenKind::RParen, ")", 1, 8),
            tok(TokenKind::LBrace, "{", 1, 10),
            tok(TokenKind::Plus, "+", 2, 5),
            tok(TokenKind::Semicolon, ";", 2, 7),
            tok(TokenKind::Return, "return", 3, 5),
            tok(TokenKind::IntLiteral, "0", 3, 12),
            tok(TokenKind::Semicolon, ";", 3, 13),
            tok(TokenKind::RBrace, "}", 4, 1),
            eof(4),
        ];
        let errs = parse_err(tokens);
        assert!(!errs.is_empty());
        // The error should mention something about an unexpected token.
        assert!(errs[0].message.contains("expected expression"));
    }

    #[test]
    fn error_includes_location() {
        let tokens = vec![
            tok(TokenKind::Plus, "+", 7, 15),
            eof(7),
        ];
        let errs = parse_err(tokens);
        assert!(!errs.is_empty());
        assert_eq!(errs[0].line, 7);
        assert_eq!(errs[0].column, 15);
    }

    // ===================================================================
    // Compound / mixed programs
    // ===================================================================

    #[test]
    fn parse_multiple_top_level() {
        // int x;
        // void f() { }
        let tokens = vec![
            tok(TokenKind::Int, "int", 1, 1),
            tok(TokenKind::Ident, "x", 1, 5),
            tok(TokenKind::Semicolon, ";", 1, 6),
            tok(TokenKind::Void, "void", 2, 1),
            tok(TokenKind::Ident, "f", 2, 6),
            tok(TokenKind::LParen, "(", 2, 7),
            tok(TokenKind::RParen, ")", 2, 8),
            tok(TokenKind::LBrace, "{", 2, 10),
            tok(TokenKind::RBrace, "}", 2, 12),
            eof(2),
        ];
        let prog = parse_ok(tokens);
        assert_eq!(prog.decls.len(), 2);
        assert!(matches!(prog.decls[0].kind, TopLevelKind::GlobalVar { .. }));
        assert!(matches!(prog.decls[1].kind, TopLevelKind::FuncDef { .. }));
    }

    #[test]
    fn parse_empty_program() {
        let tokens = vec![eof(1)];
        let prog = parse_ok(tokens);
        assert!(prog.decls.is_empty());
    }

    #[test]
    fn parse_static_function() {
        // static int helper() { return 0; }
        let tokens = vec![
            tok(TokenKind::Static, "static", 1, 1),
            tok(TokenKind::Int, "int", 1, 8),
            tok(TokenKind::Ident, "helper", 1, 12),
            tok(TokenKind::LParen, "(", 1, 18),
            tok(TokenKind::RParen, ")", 1, 19),
            tok(TokenKind::LBrace, "{", 1, 21),
            tok(TokenKind::Return, "return", 2, 5),
            tok(TokenKind::IntLiteral, "0", 2, 12),
            tok(TokenKind::Semicolon, ";", 2, 13),
            tok(TokenKind::RBrace, "}", 3, 1),
            eof(3),
        ];
        let prog = parse_ok(tokens);
        match &prog.decls[0].kind {
            TopLevelKind::FuncDef {
                name, storage, ..
            } => {
                assert_eq!(name, "helper");
                assert_eq!(*storage, Some(StorageClass::Static));
            }
            _ => panic!("expected FuncDef"),
        }
    }

    #[test]
    fn parse_compound_assignment_ops() {
        // x += 1
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "x", 1, 1),
            tok(TokenKind::PlusEq, "+=", 1, 3),
            tok(TokenKind::IntLiteral, "1", 1, 6),
        ]);
        match &e.kind {
            ExprKind::Assign {
                op: AssignOp::AddAssign,
                ..
            } => {}
            _ => panic!("expected AddAssign"),
        }
    }

    #[test]
    fn parse_bitwise_operators() {
        // a & b | c ^ d
        // Precedence: & > ^ > |
        // So: ((a & b) | (c ^ d))
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::Amp, "&", 1, 3),
            tok(TokenKind::Ident, "b", 1, 5),
            tok(TokenKind::Pipe, "|", 1, 7),
            tok(TokenKind::Ident, "c", 1, 9),
            tok(TokenKind::Caret, "^", 1, 11),
            tok(TokenKind::Ident, "d", 1, 13),
        ]);
        // Top level should be BitOr
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::BitOr,
                lhs,
                rhs,
            } => {
                assert!(matches!(
                    lhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::BitAnd,
                        ..
                    }
                ));
                assert!(matches!(
                    rhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::BitXor,
                        ..
                    }
                ));
            }
            _ => panic!("expected BitOr at top, got {:?}", e.kind),
        }
    }

    #[test]
    fn parse_string_literal_expr() {
        let e = parse_single_expr(vec![tok(
            TokenKind::StringLiteral,
            "hello",
            1,
            1,
        )]);
        match &e.kind {
            ExprKind::StringLiteral(bytes) => {
                assert_eq!(bytes, b"hello");
            }
            _ => panic!("expected StringLiteral"),
        }
    }

    #[test]
    fn parse_char_literal_expr() {
        let e = parse_single_expr(vec![tok(TokenKind::CharLiteral, "A", 1, 1)]);
        match &e.kind {
            ExprKind::CharLiteral(ch) => {
                assert_eq!(*ch, b'A');
            }
            _ => panic!("expected CharLiteral"),
        }
    }

    #[test]
    fn parse_parenthesized_expr() {
        // (a + b) * c
        let e = parse_single_expr(vec![
            tok(TokenKind::LParen, "(", 1, 1),
            tok(TokenKind::Ident, "a", 1, 2),
            tok(TokenKind::Plus, "+", 1, 4),
            tok(TokenKind::Ident, "b", 1, 6),
            tok(TokenKind::RParen, ")", 1, 7),
            tok(TokenKind::Star, "*", 1, 9),
            tok(TokenKind::Ident, "c", 1, 11),
        ]);
        // Top node should be Mul (parentheses force Add to be lhs)
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::Mul,
                lhs,
                ..
            } => {
                assert!(matches!(
                    lhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::Add,
                        ..
                    }
                ));
            }
            _ => panic!("expected Mul at top"),
        }
    }

    #[test]
    fn parse_nested_function_calls() {
        // f(g(x))
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "f", 1, 1),
            tok(TokenKind::LParen, "(", 1, 2),
            tok(TokenKind::Ident, "g", 1, 3),
            tok(TokenKind::LParen, "(", 1, 4),
            tok(TokenKind::Ident, "x", 1, 5),
            tok(TokenKind::RParen, ")", 1, 6),
            tok(TokenKind::RParen, ")", 1, 7),
        ]);
        match &e.kind {
            ExprKind::FuncCall { callee, args } => {
                assert!(matches!(callee.kind, ExprKind::Ident(ref s) if s == "f"));
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].kind, ExprKind::FuncCall { .. }));
            }
            _ => panic!("expected FuncCall"),
        }
    }

    #[test]
    fn parse_logical_not_and_bitwise_not() {
        // !~x
        let e = parse_single_expr(vec![
            tok(TokenKind::Bang, "!", 1, 1),
            tok(TokenKind::Tilde, "~", 1, 2),
            tok(TokenKind::Ident, "x", 1, 3),
        ]);
        match &e.kind {
            ExprKind::UnaryOp {
                op: UnaryOp::LogNot,
                operand,
            } => {
                assert!(matches!(
                    operand.kind,
                    ExprKind::UnaryOp {
                        op: UnaryOp::BitNot,
                        ..
                    }
                ));
            }
            _ => panic!("expected LogNot(BitNot(..))"),
        }
    }

    #[test]
    fn parse_equality_and_relational() {
        // a == b < c  →  a == (b < c)  (relational binds tighter)
        let e = parse_single_expr(vec![
            tok(TokenKind::Ident, "a", 1, 1),
            tok(TokenKind::EqEq, "==", 1, 3),
            tok(TokenKind::Ident, "b", 1, 6),
            tok(TokenKind::Lt, "<", 1, 8),
            tok(TokenKind::Ident, "c", 1, 10),
        ]);
        match &e.kind {
            ExprKind::BinOp {
                op: BinOp::Eq,
                lhs,
                rhs,
            } => {
                assert!(matches!(lhs.kind, ExprKind::Ident(ref s) if s == "a"));
                assert!(matches!(
                    rhs.kind,
                    ExprKind::BinOp {
                        op: BinOp::Lt,
                        ..
                    }
                ));
            }
            _ => panic!("expected Eq at top"),
        }
    }
}
