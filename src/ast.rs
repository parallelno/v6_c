//! Abstract Syntax Tree (AST) node definitions for the v6c compiler.
//!
//! Every node carries a [`SourceLocation`] so that later compiler phases
//! (type-checking, code generation, diagnostics) can point back to the
//! original source.

use crate::types::{CType, StorageClass};

// ---------------------------------------------------------------------------
// Source location
// ---------------------------------------------------------------------------

/// Minimal source-location information attached to every AST node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    /// 1-based line number in the source file.
    pub line: u32,
    /// 1-based column number (byte offset within the line).
    pub column: u32,
}

impl SourceLocation {
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

// ---------------------------------------------------------------------------
// Operators
// ---------------------------------------------------------------------------

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,

    // Comparison
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,

    // Logical
    LogAnd,
    LogOr,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// Arithmetic negation (`-x`).
    Negate,
    /// Bitwise complement (`~x`).
    BitNot,
    /// Logical not (`!x`).
    LogNot,
    /// Pre-increment (`++x`).
    PreInc,
    /// Pre-decrement (`--x`).
    PreDec,
    /// Post-increment (`x++`).
    PostInc,
    /// Post-decrement (`x--`).
    PostDec,
    /// Address-of (`&x`).
    AddrOf,
    /// Dereference (`*x`).
    Deref,
}

/// Assignment operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignOp {
    /// `=`
    Assign,
    /// `+=`
    AddAssign,
    /// `-=`
    SubAssign,
    /// `*=`
    MulAssign,
    /// `/=`
    DivAssign,
    /// `%=`
    ModAssign,
    /// `&=`
    BitAndAssign,
    /// `|=`
    BitOrAssign,
    /// `^=`
    BitXorAssign,
    /// `<<=`
    ShlAssign,
    /// `>>=`
    ShrAssign,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression node with attached source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub loc: SourceLocation,
}

impl Expr {
    pub fn new(kind: ExprKind, loc: SourceLocation) -> Self {
        Self { kind, loc }
    }
}

/// The different kinds of expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer literal (e.g. `42`, `0xFF`).
    IntLiteral(i64),

    /// Floating-point literal (e.g. `3.14`, `1.0e-2`).
    FloatLiteral(f64),

    /// Character literal (e.g. `'a'`).
    CharLiteral(u8),

    /// String literal (e.g. `"hello"`). Stored as raw bytes (no NUL terminator
    /// appended here — code-gen handles that).
    StringLiteral(Vec<u8>),

    /// Identifier reference (variable or function name).
    Ident(String),

    /// Binary operation: `lhs op rhs`.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    /// Unary operation: `op operand`.
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },

    /// Assignment: `target op value`.
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },

    /// Function call: `callee(args...)`.
    FuncCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    /// Array subscript: `array[index]`.
    Subscript {
        array: Box<Expr>,
        index: Box<Expr>,
    },

    /// Cast expression: `(type)expr`.
    Cast {
        ty: CType,
        expr: Box<Expr>,
    },

    /// `sizeof(type)` or `sizeof expr`. Resolved to a constant by later phases.
    SizeOf(SizeOfArg),

    /// Ternary / conditional: `cond ? then_expr : else_expr`.
    Conditional {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },

    /// Comma expression: `left, right`. Evaluates both; yields `right`.
    Comma {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Member access: `expr.member`.
    MemberAccess {
        object: Box<Expr>,
        member: String,
    },

    /// Pointer member access: `expr->member`.
    PtrMemberAccess {
        ptr: Box<Expr>,
        member: String,
    },

    /// Initializer list: `{ expr1, expr2, ... }`.
    InitList(Vec<Expr>),
}

/// Argument to `sizeof`: either a type name or a sub-expression.
#[derive(Debug, Clone, PartialEq)]
pub enum SizeOfArg {
    Type(CType),
    Expr(Box<Expr>),
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// A statement node with attached source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub loc: SourceLocation,
}

impl Stmt {
    pub fn new(kind: StmtKind, loc: SourceLocation) -> Self {
        Self { kind, loc }
    }
}

/// The different kinds of statements.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// Expression statement (expression followed by `;`).
    Expr(Expr),

    /// Compound statement / block: `{ ... }`.
    Compound(Vec<Stmt>),

    /// `if (cond) then_body [else else_body]`.
    If {
        cond: Expr,
        then_body: Box<Stmt>,
        else_body: Option<Box<Stmt>>,
    },

    /// `while (cond) body`.
    While {
        cond: Expr,
        body: Box<Stmt>,
    },

    /// `do body while (cond);`.
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },

    /// `for (init; cond; step) body`.
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },

    /// `return [expr];`.
    Return(Option<Expr>),

    /// `break;`.
    Break,

    /// `continue;`.
    Continue,

    /// `goto label;`.
    Goto(String),

    /// `label:` before a statement.
    Label {
        name: String,
        stmt: Box<Stmt>,
    },

    /// Local variable declaration, possibly with an initializer.
    ///
    /// ```c
    /// int x;
    /// int x = 42;
    /// static char buf[16];
    /// ```
    VarDecl {
        name: String,
        ty: CType,
        storage: Option<StorageClass>,
        init: Option<Expr>,
    },

    /// `switch (expr) { ... }`.
    Switch {
        expr: Expr,
        body: Box<Stmt>,
    },

    /// `case const_expr:` label inside a switch body.
    Case {
        value: i64,
        stmt: Box<Stmt>,
    },

    /// `default:` label inside a switch body.
    Default {
        stmt: Box<Stmt>,
    },
}

// ---------------------------------------------------------------------------
// Top-level declarations
// ---------------------------------------------------------------------------

/// A top-level declaration with attached source location.
#[derive(Debug, Clone, PartialEq)]
pub struct TopLevel {
    pub kind: TopLevelKind,
    pub loc: SourceLocation,
}

impl TopLevel {
    pub fn new(kind: TopLevelKind, loc: SourceLocation) -> Self {
        Self { kind, loc }
    }
}

/// A function parameter: a name (may be unnamed in prototypes) and a type.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Option<String>,
    pub ty: CType,
}

/// The different kinds of top-level declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum TopLevelKind {
    /// Function definition (has a body).
    ///
    /// ```c
    /// int add(int a, int b) { return a + b; }
    /// ```
    FuncDef {
        name: String,
        return_type: CType,
        params: Vec<Param>,
        storage: Option<StorageClass>,
        body: Stmt,
        is_variadic: bool,
    },

    /// Function prototype / forward declaration (no body).
    ///
    /// ```c
    /// int puts(char *s);
    /// ```
    FuncDecl {
        name: String,
        return_type: CType,
        params: Vec<Param>,
        storage: Option<StorageClass>,
        is_variadic: bool,
    },

    /// Global variable declaration.
    ///
    /// ```c
    /// int counter;
    /// static char *msg = "hello";
    /// ```
    GlobalVar {
        name: String,
        ty: CType,
        storage: Option<StorageClass>,
        init: Option<Expr>,
    },

    /// struct/union/enum type definition at file scope (no variable declared).
    TypeDecl,

    /// `typedef old_type new_name;` at file scope.
    Typedef {
        ty: CType,
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Program (translation unit)
// ---------------------------------------------------------------------------

/// A complete translation unit — one `.c` source file.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub decls: Vec<TopLevel>,
    /// Enum constant name → integer value, collected during parsing.
    pub enum_constants: std::collections::HashMap<String, i64>,
}

impl Program {
    pub fn new(decls: Vec<TopLevel>, enum_constants: std::collections::HashMap<String, i64>) -> Self {
        Self { decls, enum_constants }
    }

    /// Convenience constructor with no enum constants (useful for tests).
    pub fn from_decls(decls: Vec<TopLevel>) -> Self {
        Self { decls, enum_constants: std::collections::HashMap::new() }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(line: u32) -> SourceLocation {
        SourceLocation::new(line, 1)
    }

    #[test]
    fn build_int_literal_expr() {
        let e = Expr::new(ExprKind::IntLiteral(42), loc(1));
        assert_eq!(e.kind, ExprKind::IntLiteral(42));
        assert_eq!(e.loc.line, 1);
    }

    #[test]
    fn build_binary_add() {
        let lhs = Expr::new(ExprKind::Ident("x".into()), loc(5));
        let rhs = Expr::new(ExprKind::IntLiteral(1), loc(5));
        let add = Expr::new(
            ExprKind::BinOp {
                op: BinOp::Add,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            loc(5),
        );
        matches!(add.kind, ExprKind::BinOp { op: BinOp::Add, .. });
    }

    #[test]
    fn build_func_call() {
        let callee = Expr::new(ExprKind::Ident("puts".into()), loc(10));
        let arg = Expr::new(ExprKind::StringLiteral(b"hello".to_vec()), loc(10));
        let call = Expr::new(
            ExprKind::FuncCall {
                callee: Box::new(callee),
                args: vec![arg],
            },
            loc(10),
        );
        if let ExprKind::FuncCall { args, .. } = &call.kind {
            assert_eq!(args.len(), 1);
        } else {
            panic!("expected FuncCall");
        }
    }

    #[test]
    fn build_if_stmt() {
        let cond = Expr::new(ExprKind::Ident("flag".into()), loc(20));
        let body = Stmt::new(
            StmtKind::Return(Some(Expr::new(ExprKind::IntLiteral(0), loc(21)))),
            loc(21),
        );
        let s = Stmt::new(
            StmtKind::If {
                cond,
                then_body: Box::new(body),
                else_body: None,
            },
            loc(20),
        );
        matches!(s.kind, StmtKind::If { .. });
    }

    #[test]
    fn build_for_loop() {
        let init = Stmt::new(
            StmtKind::VarDecl {
                name: "i".into(),
                ty: CType::int_signed(),
                storage: None,
                init: Some(Expr::new(ExprKind::IntLiteral(0), loc(30))),
            },
            loc(30),
        );
        let cond = Expr::new(
            ExprKind::BinOp {
                op: BinOp::Lt,
                lhs: Box::new(Expr::new(ExprKind::Ident("i".into()), loc(30))),
                rhs: Box::new(Expr::new(ExprKind::IntLiteral(10), loc(30))),
            },
            loc(30),
        );
        let step = Expr::new(
            ExprKind::UnaryOp {
                op: UnaryOp::PostInc,
                operand: Box::new(Expr::new(ExprKind::Ident("i".into()), loc(30))),
            },
            loc(30),
        );
        let body = Stmt::new(StmtKind::Compound(vec![]), loc(31));
        let s = Stmt::new(
            StmtKind::For {
                init: Some(Box::new(init)),
                cond: Some(cond),
                step: Some(step),
                body: Box::new(body),
            },
            loc(30),
        );
        matches!(s.kind, StmtKind::For { .. });
    }

    #[test]
    fn build_func_def() {
        let ret = Stmt::new(
            StmtKind::Return(Some(Expr::new(
                ExprKind::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::new(ExprKind::Ident("a".into()), loc(2))),
                    rhs: Box::new(Expr::new(ExprKind::Ident("b".into()), loc(2))),
                },
                loc(2),
            ))),
            loc(2),
        );
        let body = Stmt::new(StmtKind::Compound(vec![ret]), loc(1));
        let func = TopLevel::new(
            TopLevelKind::FuncDef {
                name: "add".into(),
                return_type: CType::int_signed(),
                params: vec![
                    Param { name: Some("a".into()), ty: CType::int_signed() },
                    Param { name: Some("b".into()), ty: CType::int_signed() },
                ],
                storage: None,
                is_variadic: false,
                body,
            },
            loc(1),
        );
        if let TopLevelKind::FuncDef { name, params, .. } = &func.kind {
            assert_eq!(name, "add");
            assert_eq!(params.len(), 2);
        } else {
            panic!("expected FuncDef");
        }
    }

    #[test]
    fn build_program() {
        let decl = TopLevel::new(
            TopLevelKind::GlobalVar {
                name: "count".into(),
                ty: CType::int_signed(),
                storage: None,
                init: None,
            },
            loc(1),
        );
        let prog = Program::from_decls(vec![decl]);
        assert_eq!(prog.decls.len(), 1);
    }
}
