//! AST → IR lowering for the v6c compiler targeting the Intel 8080.
//!
//! Translates the AST ([`Program`]) into the three-address code IR
//! ([`IrProgram`]).  In the default "global mode" every local variable is
//! given a unique static-storage label (e.g. `_l_main_x`) rather than a
//! stack-frame slot, because the 8080's stack operations are expensive.

use std::collections::HashMap;
use std::fmt;

use crate::ast::*;
use crate::ir::*;
use crate::types::{common_type, CType};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// An error emitted during IR generation.
#[derive(Debug, Clone, PartialEq)]
pub struct IrGenError {
    pub message: String,
    pub line: u32,
}

impl fmt::Display for IrGenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

// ---------------------------------------------------------------------------
// LValue representation
// ---------------------------------------------------------------------------

/// The result of evaluating an expression as an lvalue.
#[derive(Debug, Clone)]
enum LValueResult {
    /// The lvalue is a named global / static label.
    Global { label: String, ty: CType },
    /// The lvalue is behind a pointer held in a virtual register.
    Ptr { reg: VReg, pointee_ty: CType },
}

// ---------------------------------------------------------------------------
// SwitchContext
// ---------------------------------------------------------------------------

/// Tracks case/default labels during switch statement codegen.
struct SwitchContext {
    /// Label for the break target (end of switch).
    break_label: Label,
    /// (case_value, label) pairs collected from Case statements.
    cases: Vec<(i64, Label)>,
    /// Label for the default branch, if present.
    default_label: Option<Label>,
}

// ---------------------------------------------------------------------------
// IrGenerator
// ---------------------------------------------------------------------------

/// Translates AST nodes into three-address-code IR.
pub struct IrGenerator {
    vreg_alloc: VRegAllocator,
    label_alloc: LabelAllocator,

    // ---- output accumulators ----
    globals: Vec<GlobalVar>,
    functions: Vec<IrFunction>,
    strings: Vec<StringLiteral>,

    // ---- symbol tables ----
    /// C name → (IR label, type) for global variables.
    global_syms: HashMap<String, (String, CType)>,
    /// Known function return types.
    func_return_types: HashMap<String, CType>,

    // ---- per-function state ----
    current_func_name: String,
    current_return_type: CType,
    /// C name → (IR label, type) for locals / parameters of the current fn.
    local_syms: HashMap<String, (String, CType)>,
    /// Instructions accumulated for the current function body.
    body: Vec<IrInstr>,
    /// Loop header labels marked with `#pragma unroll` in the source.
    unroll_loop_headers: Vec<Label>,
    current_line: u32,

    // ---- loop stacks ----
    break_stack: Vec<Label>,
    continue_stack: Vec<Label>,

    // ---- goto / user labels ----
    user_labels: HashMap<String, Label>,

    // ---- switch ----
    /// Stack of (break_label, Vec<(case_value, Label)>, Option<default_label>)
    /// for the innermost switch statement being compiled.
    switch_stack: Vec<SwitchContext>,

    // ---- enum constants ----
    enum_constants: HashMap<String, i64>,

    // ---- misc ----
    string_count: u32,
    errors: Vec<IrGenError>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Translate a parsed [`Program`] into an [`IrProgram`].
///
/// Returns `Err` with accumulated errors if any problems were detected.
pub fn generate(program: &Program) -> Result<IrProgram, Vec<IrGenError>> {
    let mut gen = IrGenerator::new();
    gen.enum_constants = program.enum_constants.clone();
    for decl in &program.decls {
        gen.gen_top_level(decl);
    }
    if gen.errors.is_empty() {
        Ok(IrProgram {
            globals: gen.globals,
            functions: gen.functions,
            strings: gen.strings,
        })
    } else {
        Err(gen.errors)
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl IrGenerator {
    fn new() -> Self {
        Self {
            vreg_alloc: VRegAllocator::new(),
            label_alloc: LabelAllocator::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            strings: Vec::new(),
            global_syms: HashMap::new(),
            func_return_types: HashMap::new(),
            current_func_name: String::new(),
            current_return_type: CType::Void,
            local_syms: HashMap::new(),
            body: Vec::new(),
            unroll_loop_headers: Vec::new(),
            current_line: 0,
            break_stack: Vec::new(),
            continue_stack: Vec::new(),
            user_labels: HashMap::new(),
            switch_stack: Vec::new(),
            enum_constants: HashMap::new(),
            string_count: 0,
            errors: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn emit(&mut self, op: IrOp) {
        self.body.push(IrInstr::new(op, self.current_line));
    }

    fn error(&mut self, msg: &str) {
        self.errors.push(IrGenError {
            message: msg.to_string(),
            line: self.current_line,
        });
    }

    /// Emit an error if the integer literal `val` cannot be represented in `ty`.
    fn check_literal_fits_type(&mut self, val: i64, ty: &CType) {
        let fits = match ty {
            CType::Char { signed: true }  => val >= -128  && val <= 127,
            CType::Char { signed: false } => val >= 0     && val <= 255,
            CType::Int  { signed: true }  => val >= -32768 && val <= 32767,
            CType::Int  { signed: false } => val >= 0     && val <= 65535,
            CType::Long { signed: true }  => val >= i32::MIN as i64 && val <= i32::MAX as i64,
            CType::Long { signed: false } => val >= 0     && val <= u32::MAX as i64,
            _ => return,
        };
        if !fits {
            self.error(&format!(
                "constant value {} overflows type '{}'",
                val, ty
            ));
        }
    }

    fn body_ends_with_return(&self) -> bool {
        self.body
            .last()
            .map_or(false, |i| matches!(i.op, IrOp::Return { .. }))
    }

    fn get_or_create_user_label(&mut self, name: &str) -> Label {
        if let Some(&lbl) = self.user_labels.get(name) {
            lbl
        } else {
            let lbl = self.label_alloc.alloc();
            self.user_labels.insert(name.to_string(), lbl);
            lbl
        }
    }

    /// Return a VReg whose width matches `to`, inserting a [`IrOp::Cast`] if
    /// the source width differs.
    fn maybe_cast(&mut self, reg: VReg, from: &CType, to: &CType) -> VReg {
        if from == to {
            return reg;
        }

        // int → float conversion via runtime call.
        if from.is_integer() && to.is_float() {
            let dst = self.vreg_alloc.alloc(Width::W32);
            self.emit(IrOp::Call {
                func_name: "__itof".to_string(),
                args: vec![reg],
                dst: Some(dst),
            });
            return dst;
        }

        // float → int conversion via runtime call.
        if from.is_float() && to.is_integer() {
            let to_w = Width::from_ctype(to).unwrap_or(Width::W16);
            let dst = self.vreg_alloc.alloc(to_w);
            self.emit(IrOp::Call {
                func_name: "__ftoi".to_string(),
                args: vec![reg],
                dst: Some(dst),
            });
            return dst;
        }

        let from_w = Width::from_ctype(from);
        let to_w = Width::from_ctype(to);
        if from_w == to_w {
            return reg;
        }
        let dst = self.vreg_alloc.alloc(to_w.unwrap_or(Width::W16));
        self.emit(IrOp::cast(dst, reg, to.clone()));
        dst
    }

    /// Determine the C type of an expression *without* generating code.
    /// Used for `sizeof(expr)`.
    fn expr_type(&self, expr: &Expr) -> CType {
        match &expr.kind {
            ExprKind::IntLiteral(_) => CType::int_signed(),
            ExprKind::FloatLiteral(_) => CType::Float,
            ExprKind::CharLiteral(_) => CType::char_signed(),
            ExprKind::StringLiteral(_) => CType::ptr(CType::char_signed()),
            ExprKind::Ident(name) => {
                if let Some((_, ty)) = self.local_syms.get(name) {
                    ty.decay()
                } else if let Some((_, ty)) = self.global_syms.get(name) {
                    ty.decay()
                } else {
                    CType::int_signed()
                }
            }
            ExprKind::UnaryOp {
                op: UnaryOp::AddrOf,
                operand,
            } => CType::ptr(self.expr_type(operand)),
            ExprKind::UnaryOp {
                op: UnaryOp::Deref,
                operand,
            } => {
                let ty = self.expr_type(operand);
                ty.pointee().cloned().unwrap_or(CType::int_signed())
            }
            ExprKind::UnaryOp {
                op: UnaryOp::LogNot,
                ..
            } => CType::int_signed(),
            ExprKind::UnaryOp { operand, .. } => self.expr_type(operand),
            ExprKind::BinOp { op, lhs, rhs } => match op {
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::LogAnd
                | BinOp::LogOr => CType::int_signed(),
                _ => {
                    let lt = self.expr_type(lhs);
                    let rt = self.expr_type(rhs);
                    if lt.is_pointer() {
                        lt
                    } else if rt.is_pointer() {
                        rt
                    } else {
                        common_type(&lt, &rt).unwrap_or(CType::int_signed())
                    }
                }
            },
            ExprKind::Assign { target, .. } => self.expr_type(target),
            ExprKind::FuncCall { callee, .. } => {
                if let ExprKind::Ident(name) = &callee.kind {
                    self.func_return_types
                        .get(name)
                        .cloned()
                        .unwrap_or(CType::int_signed())
                } else {
                    CType::int_signed()
                }
            }
            ExprKind::Subscript { array, .. } => {
                let ty = self.expr_type(array);
                if let Some(p) = ty.pointee() {
                    p.clone()
                } else if let Some(e) = ty.element_type() {
                    e.clone()
                } else {
                    CType::int_signed()
                }
            }
            ExprKind::Cast { ty, .. } => ty.clone(),
            ExprKind::SizeOf(_) => CType::int_unsigned(),
            ExprKind::Conditional { then_expr, .. } => self.expr_type(then_expr),
            ExprKind::Comma { right, .. } => self.expr_type(right),
            ExprKind::MemberAccess { object, member } => {
                let obj_ty = self.expr_type(object);
                obj_ty
                    .field_offset(member)
                    .map(|(_, ty)| ty)
                    .unwrap_or(CType::int_signed())
            }
            ExprKind::PtrMemberAccess { ptr, member } => {
                let ptr_ty = self.expr_type(ptr);
                let struct_ty = ptr_ty.pointee().cloned().unwrap_or(CType::int_signed());
                struct_ty
                    .field_offset(member)
                    .map(|(_, ty)| ty)
                    .unwrap_or(CType::int_signed())
            }
            ExprKind::InitList(elems) => {
                elems.last().map(|e| self.expr_type(e)).unwrap_or(CType::int_signed())
            }
        }
    }

    // -----------------------------------------------------------------------
    // Top-level declarations
    // -----------------------------------------------------------------------

    fn gen_top_level(&mut self, decl: &TopLevel) {
        self.current_line = decl.loc.line;
        match &decl.kind {
            TopLevelKind::GlobalVar {
                name,
                ty,
                storage: _,
                init,
            } => self.gen_global_var(name, ty, init),

            TopLevelKind::FuncDef {
                name,
                return_type,
                params,
                storage: _,
                body,
                is_variadic,
            } => self.gen_func_def(name, return_type, params, body, *is_variadic, decl.loc),

            TopLevelKind::FuncDecl {
                name,
                return_type,
                params: _,
                storage: _,
                is_variadic: _,
            } => {
                self.func_return_types
                    .insert(name.clone(), return_type.clone());
            }

            // Type declarations and typedefs are resolved at parse time;
            // nothing to emit for them.
            TopLevelKind::TypeDecl | TopLevelKind::Typedef { .. } => {}
        }
    }

    // ---- global variables ------------------------------------------------

    fn gen_global_var(&mut self, name: &str, ty: &CType, init: &Option<Expr>) {
        let label = format!("_g_{}", name);

        let init_bytes = init.as_ref().and_then(|e| self.const_init_bytes(e, ty));

        self.globals.push(GlobalVar {
            name: label.clone(),
            ty: ty.clone(),
            init: init_bytes,
        });
        self.global_syms
            .insert(name.to_string(), (label, ty.clone()));
    }

    /// Try to fold a constant initializer into a byte vector.
    fn const_init_bytes(&self, expr: &Expr, ty: &CType) -> Option<Vec<u8>> {
        let size = ty.size_of()?;
        match &expr.kind {
            ExprKind::IntLiteral(val) => {
                let mut bytes = vec![0u8; size];
                let v = *val;
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = ((v >> (i as u64 * 8)) & 0xff) as u8;
                }
                Some(bytes)
            }
            ExprKind::CharLiteral(ch) => {
                let mut bytes = vec![0u8; size];
                bytes[0] = *ch;
                Some(bytes)
            }
            ExprKind::InitList(elems) => {
                self.const_init_list_bytes(elems, ty)
            }
            _ => None,
        }
    }

    /// Try to fold an initializer list into a byte vector for global data.
    fn const_init_list_bytes(&self, elems: &[Expr], ty: &CType) -> Option<Vec<u8>> {
        let total_size = ty.size_of()?;
        let mut bytes = vec![0u8; total_size];
        match ty {
            CType::Array { element: ref elem_ty, .. } => {
                let elem_size = elem_ty.size_of()?;
                for (i, elem) in elems.iter().enumerate() {
                    let offset = i * elem_size;
                    if offset >= total_size {
                        break;
                    }
                    let elem_bytes = self.const_init_bytes(elem, elem_ty)?;
                    for (j, b) in elem_bytes.iter().enumerate() {
                        if offset + j < total_size {
                            bytes[offset + j] = *b;
                        }
                    }
                }
                Some(bytes)
            }
            CType::Struct { members, .. } => {
                let mut offset = 0usize;
                for (i, (_mname, mty)) in members.iter().enumerate() {
                    if i >= elems.len() {
                        break;
                    }
                    let elem_bytes = self.const_init_bytes(&elems[i], mty)?;
                    for (j, b) in elem_bytes.iter().enumerate() {
                        if offset + j < total_size {
                            bytes[offset + j] = *b;
                        }
                    }
                    offset += mty.size_of().unwrap_or(1);
                }
                Some(bytes)
            }
            _ => {
                // Scalar – use first element.
                if let Some(first) = elems.first() {
                    self.const_init_bytes(first, ty)
                } else {
                    Some(bytes)
                }
            }
        }
    }

    // ---- function definitions --------------------------------------------

    fn gen_func_def(
        &mut self,
        name: &str,
        return_type: &CType,
        params: &[Param],
        body_stmt: &Stmt,
        is_variadic: bool,
        loc: SourceLocation,
    ) {
        // Reset per-function state.
        self.current_func_name = name.to_string();
        self.current_return_type = return_type.clone();
        self.local_syms.clear();
        self.body.clear();
        self.unroll_loop_headers.clear();
        self.user_labels.clear();
        self.break_stack.clear();
        self.continue_stack.clear();
        self.current_line = loc.line;

        self.func_return_types
            .insert(name.to_string(), return_type.clone());

        // Build IR params: each parameter gets a static label and a store at
        // function entry.
        let mut ir_params = Vec::new();
        for param in params {
            let param_name = param
                .name
                .clone()
                .unwrap_or_else(|| "_unnamed".to_string());
            let vreg = self.vreg_alloc.alloc_for_type(&param.ty);
            ir_params.push(IrParam {
                name: param_name.clone(),
                ty: param.ty.clone(),
                vreg,
            });

            let label = format!("_l_{}_{}", name, param_name);
            self.globals.push(GlobalVar {
                name: label.clone(),
                ty: param.ty.clone(),
                init: None,
            });
            self.emit(IrOp::store_global(&label, vreg));
            self.local_syms
                .insert(param_name, (label, param.ty.clone()));
        }

        // Generate the function body.
        self.gen_stmt(body_stmt);

        // Append an implicit return if the body doesn't already end with one.
        if !self.body_ends_with_return() {
            if return_type.is_void() {
                self.emit(IrOp::ret(None));
            } else {
                let zero = self.vreg_alloc.alloc_for_type(return_type);
                self.emit(IrOp::load_imm(zero, 0));
                self.emit(IrOp::ret(Some(zero)));
            }
        }

        self.functions.push(IrFunction {
            name: name.to_string(),
            params: ir_params,
            locals: Vec::new(),
            body: std::mem::take(&mut self.body),
            unroll_loop_headers: std::mem::take(&mut self.unroll_loop_headers),
            return_type: return_type.clone(),
            is_stack_mode: false,
            is_variadic,
        });
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    fn gen_stmt(&mut self, stmt: &Stmt) {
        self.current_line = stmt.loc.line;
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.gen_expr(expr);
            }

            StmtKind::Compound(stmts) => {
                for s in stmts {
                    self.gen_stmt(s);
                }
            }

            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => self.gen_if(cond, then_body, else_body.as_deref()),

            StmtKind::While {
                cond,
                body,
                unroll_hint,
            } => self.gen_while(cond, body, *unroll_hint),

            StmtKind::DoWhile {
                body,
                cond,
                unroll_hint,
            } => self.gen_do_while(body, cond, *unroll_hint),

            StmtKind::For {
                init,
                cond,
                step,
                body,
                unroll_hint,
            } => self.gen_for(
                init.as_deref(),
                cond.as_ref(),
                step.as_ref(),
                body,
                *unroll_hint,
            ),

            StmtKind::Return(val) => {
                if let Some(expr) = val {
                    let (reg, val_ty) = self.gen_expr(expr);
                    let reg = self.maybe_cast(reg, &val_ty, &self.current_return_type.clone());
                    self.emit(IrOp::ret(Some(reg)));
                } else {
                    self.emit(IrOp::ret(None));
                }
            }

            StmtKind::Break => {
                if let Some(&target) = self.break_stack.last() {
                    self.emit(IrOp::jump(target));
                } else {
                    self.error("break outside of loop");
                }
            }

            StmtKind::Continue => {
                if let Some(&target) = self.continue_stack.last() {
                    self.emit(IrOp::jump(target));
                } else {
                    self.error("continue outside of loop");
                }
            }

            StmtKind::Goto(name) => {
                let lbl = self.get_or_create_user_label(name);
                self.emit(IrOp::jump(lbl));
            }

            StmtKind::Label { name, stmt } => {
                let lbl = self.get_or_create_user_label(name);
                self.emit(IrOp::label(lbl));
                self.gen_stmt(stmt);
            }

            StmtKind::VarDecl {
                name,
                ty,
                storage: _,
                init,
            } => self.gen_var_decl(name, ty, init),

            StmtKind::Switch { expr, body } => self.gen_switch(expr, body),

            StmtKind::Case { value, stmt } => {
                // Emit the label for this case value.
                let lbl = self.label_alloc.alloc();
                if let Some(ctx) = self.switch_stack.last_mut() {
                    ctx.cases.push((*value, lbl));
                }
                self.emit(IrOp::label(lbl));
                self.gen_stmt(stmt);
            }

            StmtKind::Default { stmt } => {
                let lbl = self.label_alloc.alloc();
                if let Some(ctx) = self.switch_stack.last_mut() {
                    ctx.default_label = Some(lbl);
                }
                self.emit(IrOp::label(lbl));
                self.gen_stmt(stmt);
            }

            StmtKind::AsmBlock { .. } => {
                // TODO: Phase 4 — inline asm IR generation
                unimplemented!("inline asm not yet implemented");
            }
        }
    }

    // ---- statement helpers ------------------------------------------------

    fn gen_if(&mut self, cond: &Expr, then_body: &Stmt, else_body: Option<&Stmt>) {
        let (cond_reg, _) = self.gen_expr(cond);
        if let Some(else_body) = else_body {
            let else_lbl = self.label_alloc.alloc();
            let end_lbl = self.label_alloc.alloc();
            self.emit(IrOp::jump_if_false(cond_reg, else_lbl));
            self.gen_stmt(then_body);
            self.emit(IrOp::jump(end_lbl));
            self.emit(IrOp::label(else_lbl));
            self.gen_stmt(else_body);
            self.emit(IrOp::label(end_lbl));
        } else {
            let end_lbl = self.label_alloc.alloc();
            self.emit(IrOp::jump_if_false(cond_reg, end_lbl));
            self.gen_stmt(then_body);
            self.emit(IrOp::label(end_lbl));
        }
    }

    fn gen_while(&mut self, cond: &Expr, body: &Stmt, unroll_hint: bool) {
        let start = self.label_alloc.alloc();
        let end = self.label_alloc.alloc();
        self.break_stack.push(end);
        self.continue_stack.push(start);
        if unroll_hint {
            self.unroll_loop_headers.push(start);
        }

        self.emit(IrOp::label(start));
        let (cond_reg, _) = self.gen_expr(cond);
        self.emit(IrOp::jump_if_false(cond_reg, end));
        self.gen_stmt(body);
        self.emit(IrOp::jump(start));
        self.emit(IrOp::label(end));

        self.break_stack.pop();
        self.continue_stack.pop();
    }

    fn gen_do_while(&mut self, body: &Stmt, cond: &Expr, unroll_hint: bool) {
        let start = self.label_alloc.alloc();
        let cont = self.label_alloc.alloc();
        let end = self.label_alloc.alloc();
        self.break_stack.push(end);
        self.continue_stack.push(cont);
        if unroll_hint {
            self.unroll_loop_headers.push(start);
        }

        self.emit(IrOp::label(start));
        self.gen_stmt(body);
        self.emit(IrOp::label(cont));
        let (cond_reg, _) = self.gen_expr(cond);
        self.emit(IrOp::jump_if_true(cond_reg, start));
        self.emit(IrOp::label(end));

        self.break_stack.pop();
        self.continue_stack.pop();
    }

    fn gen_for(
        &mut self,
        init: Option<&Stmt>,
        cond: Option<&Expr>,
        step: Option<&Expr>,
        body: &Stmt,
        unroll_hint: bool,
    ) {
        if let Some(init) = init {
            self.gen_stmt(init);
        }

        let start = self.label_alloc.alloc();
        let cont = self.label_alloc.alloc();
        let end = self.label_alloc.alloc();
        self.break_stack.push(end);
        self.continue_stack.push(cont);
        if unroll_hint {
            self.unroll_loop_headers.push(start);
        }

        self.emit(IrOp::label(start));
        if let Some(cond) = cond {
            let (cond_reg, _) = self.gen_expr(cond);
            self.emit(IrOp::jump_if_false(cond_reg, end));
        }
        self.gen_stmt(body);
        self.emit(IrOp::label(cont));
        if let Some(step) = step {
            self.gen_expr(step);
        }
        self.emit(IrOp::jump(start));
        self.emit(IrOp::label(end));

        self.break_stack.pop();
        self.continue_stack.pop();
    }

    fn gen_switch(&mut self, expr: &Expr, body: &Stmt) {
        let (val, _val_ty) = self.gen_expr(expr);

        let dispatch_lbl = self.label_alloc.alloc();
        let end_lbl = self.label_alloc.alloc();

        // Jump to the dispatch block (emitted after the body).
        self.emit(IrOp::jump(dispatch_lbl));

        // Push switch context so Case/Default statements can register labels.
        self.switch_stack.push(SwitchContext {
            break_label: end_lbl,
            cases: Vec::new(),
            default_label: None,
        });
        self.break_stack.push(end_lbl);

        // Generate the switch body (case/default labels are registered).
        self.gen_stmt(body);
        // Fall through to end after the last case.
        self.emit(IrOp::jump(end_lbl));

        // Emit the dispatch block: compare val against each case value.
        self.emit(IrOp::label(dispatch_lbl));
        let ctx = self.switch_stack.pop().unwrap();
        self.break_stack.pop();

        for (case_val, case_lbl) in &ctx.cases {
            let imm = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::load_imm(imm, *case_val));
            let cmp = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::Eq {
                dst: cmp,
                lhs: val,
                rhs: imm,
                width: Width::W16,
            });
            self.emit(IrOp::jump_if_true(cmp, *case_lbl));
        }

        // Jump to default or end.
        if let Some(default_lbl) = ctx.default_label {
            self.emit(IrOp::jump(default_lbl));
        } else {
            self.emit(IrOp::jump(end_lbl));
        }

        self.emit(IrOp::label(end_lbl));
    }

    fn gen_var_decl(&mut self, name: &str, ty: &CType, init: &Option<Expr>) {
        let label = format!("_l_{}_{}", self.current_func_name, name);
        self.globals.push(GlobalVar {
            name: label.clone(),
            ty: ty.clone(),
            init: None,
        });
        self.local_syms
            .insert(name.to_string(), (label.clone(), ty.clone()));

        if let Some(init_expr) = init {
            match &init_expr.kind {
                ExprKind::InitList(elems) => {
                    self.gen_init_list_store(&label, ty, elems);
                }
                _ => {
                    let (val, _) = self.gen_expr(init_expr);
                    self.emit(IrOp::store_global(&label, val));
                }
            }
        }
    }

    /// Generate stores for an initializer list into a named variable.
    fn gen_init_list_store(&mut self, label: &str, ty: &CType, elems: &[Expr]) {
        match ty {
            CType::Array { element: ref elem_ty, .. } => {
                let elem_size = elem_ty.size_of().unwrap_or(1);
                let base = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::addr_of_global(base, label));
                for (i, elem_expr) in elems.iter().enumerate() {
                    let (val, _) = self.gen_expr(elem_expr);
                    if i == 0 {
                        self.emit(IrOp::store_ptr(base, val));
                    } else {
                        let offset = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::load_imm(offset, (i * elem_size) as i64));
                        let ptr = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::Add {
                            dst: ptr,
                            lhs: base,
                            rhs: offset,
                            width: Width::W16,
                        });
                        self.emit(IrOp::store_ptr(ptr, val));
                    }
                }
            }
            CType::Struct { members, .. } => {
                let base = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::addr_of_global(base, label));
                let mut offset_bytes = 0usize;
                for (i, (_mname, mty)) in members.iter().enumerate() {
                    if i >= elems.len() {
                        break;
                    }
                    let (val, _) = self.gen_expr(&elems[i]);
                    if offset_bytes == 0 {
                        self.emit(IrOp::store_ptr(base, val));
                    } else {
                        let off_reg = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::load_imm(off_reg, offset_bytes as i64));
                        let ptr = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::Add {
                            dst: ptr,
                            lhs: base,
                            rhs: off_reg,
                            width: Width::W16,
                        });
                        self.emit(IrOp::store_ptr(ptr, val));
                    }
                    offset_bytes += mty.size_of().unwrap_or(1);
                }
            }
            _ => {
                // Scalar with brace initializer – use first element.
                if let Some(first) = elems.first() {
                    let (val, _) = self.gen_expr(first);
                    self.emit(IrOp::store_global(label, val));
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expressions  (returns (result_vreg, result_ctype))
    // -----------------------------------------------------------------------

    fn gen_expr(&mut self, expr: &Expr) -> (VReg, CType) {
        self.current_line = expr.loc.line;
        match &expr.kind {
            ExprKind::IntLiteral(val) => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::load_imm(dst, *val));
                (dst, CType::int_signed())
            }

            ExprKind::FloatLiteral(val) => {
                // Store IEEE 754 bits as a 32-bit integer in a W32 vreg.
                let bits = (*val as f32).to_bits() as i64;
                let dst = self.vreg_alloc.alloc(Width::W32);
                self.emit(IrOp::load_imm(dst, bits));
                (dst, CType::Float)
            }

            ExprKind::CharLiteral(val) => {
                let dst = self.vreg_alloc.alloc(Width::W8);
                self.emit(IrOp::load_imm(dst, *val as i64));
                (dst, CType::char_signed())
            }

            ExprKind::StringLiteral(bytes) => {
                let label = format!("_S{}", self.string_count);
                self.string_count += 1;
                let mut data = bytes.clone();
                data.push(0); // NUL terminator
                self.strings.push(StringLiteral {
                    label: label.clone(),
                    data,
                });
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::addr_of_global(dst, &label));
                (dst, CType::ptr(CType::char_signed()))
            }

            ExprKind::Ident(name) => self.gen_ident_expr(name),

            ExprKind::BinOp { op, lhs, rhs } => self.gen_binop(*op, lhs, rhs),

            ExprKind::UnaryOp { op, operand } => self.gen_unaryop(*op, operand),

            ExprKind::Assign { op, target, value } => self.gen_assign(*op, target, value),

            ExprKind::FuncCall { callee, args } => self.gen_func_call(callee, args),

            ExprKind::Subscript { array, index } => self.gen_subscript(array, index),

            ExprKind::Cast { ty, expr: inner } => {
                let (src, _) = self.gen_expr(inner);
                let dst_w = Width::from_ctype(ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(dst_w);
                self.emit(IrOp::cast(dst, src, ty.clone()));
                (dst, ty.clone())
            }

            ExprKind::SizeOf(arg) => {
                let size = match arg {
                    SizeOfArg::Type(ty) => ty.size_of().unwrap_or(0),
                    SizeOfArg::Expr(inner) => {
                        let ty = self.expr_type(inner);
                        ty.size_of().unwrap_or(0)
                    }
                };
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::load_imm(dst, size as i64));
                (dst, CType::int_unsigned())
            }

            ExprKind::Conditional {
                cond,
                then_expr,
                else_expr,
            } => self.gen_conditional(cond, then_expr, else_expr),

            ExprKind::Comma { left, right } => {
                self.gen_expr(left);
                self.gen_expr(right)
            }

            ExprKind::MemberAccess { object, member } => {
                self.gen_member_access(object, member)
            }

            ExprKind::PtrMemberAccess { ptr, member } => {
                self.gen_ptr_member_access(ptr, member)
            }

            ExprKind::InitList(elems) => {
                // InitList in expression context: evaluate all elements,
                // return the last (GCC extension behavior).
                let mut result = (self.vreg_alloc.alloc(Width::W16), CType::int_signed());
                for e in elems {
                    result = self.gen_expr(e);
                }
                result
            }
        }
    }

    // ---- identifier -------------------------------------------------------

    fn gen_ident_expr(&mut self, name: &str) -> (VReg, CType) {
        // Local variables / parameters first.
        if let Some((label, ty)) = self.local_syms.get(name).cloned() {
            return self.load_named_var(&label, &ty);
        }
        // Global variables.
        if let Some((label, ty)) = self.global_syms.get(name).cloned() {
            return self.load_named_var(&label, &ty);
        }
        // Enum constants.
        if let Some(&val) = self.enum_constants.get(name) {
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::load_imm(dst, val));
            return (dst, CType::int_signed());
        }
        // Known function name → address (function pointer decay).
        if self.func_return_types.contains_key(name) {
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::addr_of_global(dst, name));
            return (dst, CType::ptr(CType::Void));
        }
        // Unknown identifier.
        self.error(&format!("undefined variable: {}", name));
        let label = format!("_g_{}", name);
        let dst = self.vreg_alloc.alloc(Width::W16);
        self.emit(IrOp::load_global(dst, &label));
        (dst, CType::int_signed())
    }

    /// Load a named variable, decaying arrays to pointers.
    fn load_named_var(&mut self, label: &str, ty: &CType) -> (VReg, CType) {
        if ty.is_array() {
            let elem = ty.element_type().unwrap().clone();
            let ptr_ty = CType::ptr(elem);
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::addr_of_global(dst, label));
            (dst, ptr_ty)
        } else if ty.is_struct_or_union() {
            // Structs/unions don't fit in a register; return their address.
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::addr_of_global(dst, label));
            (dst, CType::ptr(ty.clone()))
        } else {
            let w = Width::from_ctype(ty).unwrap_or(Width::W16);
            let dst = self.vreg_alloc.alloc(w);
            self.emit(IrOp::load_global(dst, label));
            (dst, ty.clone())
        }
    }

    // ---- binary operations ------------------------------------------------

    fn gen_binop(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> (VReg, CType) {
        // Short-circuit logical operators.
        match op {
            BinOp::LogAnd => return self.gen_log_and(lhs, rhs),
            BinOp::LogOr => return self.gen_log_or(lhs, rhs),
            _ => {}
        }

        let (lhs_reg, lhs_ty) = self.gen_expr(lhs);
        let (rhs_reg, rhs_ty) = self.gen_expr(rhs);

        // Pointer arithmetic: ptr + int / int + ptr / ptr - int.
        if (lhs_ty.is_pointer() || lhs_ty.is_array()) && rhs_ty.is_integer() && op == BinOp::Add {
            return self.gen_ptr_arith(lhs_reg, &lhs_ty, rhs_reg, true);
        }
        if lhs_ty.is_integer() && (rhs_ty.is_pointer() || rhs_ty.is_array()) && op == BinOp::Add {
            return self.gen_ptr_arith(rhs_reg, &rhs_ty, lhs_reg, true);
        }
        if (lhs_ty.is_pointer() || lhs_ty.is_array()) && rhs_ty.is_integer() && op == BinOp::Sub {
            return self.gen_ptr_arith(lhs_reg, &lhs_ty, rhs_reg, false);
        }

        // Usual arithmetic conversions.
        let result_ty = common_type(&lhs_ty, &rhs_ty).unwrap_or(CType::int_signed());
        let width = Width::from_ctype(&result_ty).unwrap_or(Width::W16);
        let signed = result_ty.is_signed();

        let l = self.maybe_cast(lhs_reg, &lhs_ty, &result_ty);
        let r = self.maybe_cast(rhs_reg, &rhs_ty, &result_ty);

        // Float operations → emit calls to soft-float runtime library.
        if result_ty.is_float() {
            return self.gen_float_binop(op, l, r, &result_ty);
        }

        match op {
            // --- arithmetic ---
            BinOp::Add => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Add { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::Sub => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Sub { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::Mul => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Mul { dst, lhs: l, rhs: r, width, signed });
                (dst, result_ty)
            }
            BinOp::Div => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Div { dst, lhs: l, rhs: r, width, signed });
                (dst, result_ty)
            }
            BinOp::Mod => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Mod { dst, lhs: l, rhs: r, width, signed });
                (dst, result_ty)
            }
            // --- bitwise ---
            BinOp::BitAnd => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::And { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::BitOr => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Or { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::BitXor => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Xor { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::Shl => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Shl { dst, lhs: l, rhs: r, width });
                (dst, result_ty)
            }
            BinOp::Shr => {
                let dst = self.vreg_alloc.alloc(width);
                self.emit(IrOp::Shr { dst, lhs: l, rhs: r, width, arithmetic: signed });
                (dst, result_ty)
            }
            // --- comparison (result is C `int`) ---
            BinOp::Eq => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Eq { dst, lhs: l, rhs: r, width });
                (dst, CType::int_signed())
            }
            BinOp::Ne => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Ne { dst, lhs: l, rhs: r, width });
                (dst, CType::int_signed())
            }
            BinOp::Lt => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Lt { dst, lhs: l, rhs: r, width, signed });
                (dst, CType::int_signed())
            }
            BinOp::Le => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Le { dst, lhs: l, rhs: r, width, signed });
                (dst, CType::int_signed())
            }
            BinOp::Gt => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Gt { dst, lhs: l, rhs: r, width, signed });
                (dst, CType::int_signed())
            }
            BinOp::Ge => {
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Ge { dst, lhs: l, rhs: r, width, signed });
                (dst, CType::int_signed())
            }
            BinOp::LogAnd | BinOp::LogOr => unreachable!("handled above"),
        }
    }

    // ---- short-circuit logical operators ----------------------------------

    fn gen_log_and(&mut self, lhs: &Expr, rhs: &Expr) -> (VReg, CType) {
        let result = self.vreg_alloc.alloc(Width::W16);
        let false_lbl = self.label_alloc.alloc();
        let end_lbl = self.label_alloc.alloc();

        let (l, _) = self.gen_expr(lhs);
        self.emit(IrOp::jump_if_false(l, false_lbl));
        let (r, _) = self.gen_expr(rhs);
        self.emit(IrOp::jump_if_false(r, false_lbl));

        self.emit(IrOp::load_imm(result, 1));
        self.emit(IrOp::jump(end_lbl));

        self.emit(IrOp::label(false_lbl));
        self.emit(IrOp::load_imm(result, 0));

        self.emit(IrOp::label(end_lbl));
        (result, CType::int_signed())
    }

    fn gen_log_or(&mut self, lhs: &Expr, rhs: &Expr) -> (VReg, CType) {
        let result = self.vreg_alloc.alloc(Width::W16);
        let true_lbl = self.label_alloc.alloc();
        let end_lbl = self.label_alloc.alloc();

        let (l, _) = self.gen_expr(lhs);
        self.emit(IrOp::jump_if_true(l, true_lbl));
        let (r, _) = self.gen_expr(rhs);
        self.emit(IrOp::jump_if_true(r, true_lbl));

        self.emit(IrOp::load_imm(result, 0));
        self.emit(IrOp::jump(end_lbl));

        self.emit(IrOp::label(true_lbl));
        self.emit(IrOp::load_imm(result, 1));

        self.emit(IrOp::label(end_lbl));
        (result, CType::int_signed())
    }

    // ---- unary operations -------------------------------------------------

    fn gen_unaryop(&mut self, op: UnaryOp, operand: &Expr) -> (VReg, CType) {
        match op {
            UnaryOp::Negate => {
                let (src, ty) = self.gen_expr(operand);
                let w = Width::from_ctype(&ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(w);
                self.emit(IrOp::Neg { dst, src, width: w });
                (dst, ty)
            }
            UnaryOp::BitNot => {
                let (src, ty) = self.gen_expr(operand);
                let w = Width::from_ctype(&ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(w);
                self.emit(IrOp::Not { dst, src, width: w });
                (dst, ty)
            }
            UnaryOp::LogNot => {
                let (src, ty) = self.gen_expr(operand);
                let w = Width::from_ctype(&ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::LogicalNot { dst, src, width: w });
                (dst, CType::int_signed())
            }
            UnaryOp::PreInc | UnaryOp::PreDec => self.gen_pre_inc_dec(op, operand),
            UnaryOp::PostInc | UnaryOp::PostDec => self.gen_post_inc_dec(op, operand),
            UnaryOp::AddrOf => {
                let lv = self.gen_lvalue(operand);
                match lv {
                    LValueResult::Global { label, ty } => {
                        let dst = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::addr_of_global(dst, &label));
                        (dst, CType::ptr(ty))
                    }
                    LValueResult::Ptr { reg, pointee_ty } => (reg, CType::ptr(pointee_ty)),
                }
            }
            UnaryOp::Deref => {
                let (ptr, ptr_ty) = self.gen_expr(operand);
                let pointee = ptr_ty.pointee().cloned().unwrap_or(CType::int_signed());
                let w = Width::from_ctype(&pointee).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(w);
                self.emit(IrOp::load_ptr(dst, ptr));
                (dst, pointee)
            }
        }
    }

    fn gen_pre_inc_dec(&mut self, op: UnaryOp, operand: &Expr) -> (VReg, CType) {
        let lv = self.gen_lvalue(operand);
        let (old, ty) = self.load_lvalue(&lv);
        let w = Width::from_ctype(&ty).unwrap_or(Width::W16);

        let step_val = self.inc_step(&ty);
        let one = self.vreg_alloc.alloc(w);
        self.emit(IrOp::load_imm(one, step_val));

        let new = self.vreg_alloc.alloc(w);
        if matches!(op, UnaryOp::PreInc) {
            self.emit(IrOp::Add { dst: new, lhs: old, rhs: one, width: w });
        } else {
            self.emit(IrOp::Sub { dst: new, lhs: old, rhs: one, width: w });
        }
        self.store_lvalue(&lv, new);
        (new, ty)
    }

    fn gen_post_inc_dec(&mut self, op: UnaryOp, operand: &Expr) -> (VReg, CType) {
        let lv = self.gen_lvalue(operand);
        let (old, ty) = self.load_lvalue(&lv);
        let w = Width::from_ctype(&ty).unwrap_or(Width::W16);

        let step_val = self.inc_step(&ty);
        let one = self.vreg_alloc.alloc(w);
        self.emit(IrOp::load_imm(one, step_val));

        let new = self.vreg_alloc.alloc(w);
        if matches!(op, UnaryOp::PostInc) {
            self.emit(IrOp::Add { dst: new, lhs: old, rhs: one, width: w });
        } else {
            self.emit(IrOp::Sub { dst: new, lhs: old, rhs: one, width: w });
        }
        self.store_lvalue(&lv, new);
        (old, ty) // return the old value
    }

    /// Step value for increment/decrement: 1 for scalars, element size for
    /// pointers.
    fn inc_step(&self, ty: &CType) -> i64 {
        if ty.is_pointer() {
            ty.pointee()
                .and_then(|p| p.size_of())
                .unwrap_or(1) as i64
        } else {
            1
        }
    }

    // ---- assignment -------------------------------------------------------

    fn gen_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) -> (VReg, CType) {
        if matches!(op, AssignOp::Assign) {
            // Simple assignment.
            let lv = self.gen_lvalue(target);
            let ty = self.lvalue_type(&lv);
            // Check that integer literals fit the target type before narrowing.
            if let ExprKind::IntLiteral(lit_val) = &value.kind {
                self.check_literal_fits_type(*lit_val, &ty);
            }
            let (val, val_ty) = self.gen_expr(value);
            let val = self.maybe_cast(val, &val_ty, &ty);
            self.store_lvalue(&lv, val);
            return (val, ty);
        }

        // Compound assignment: load, compute, store.
        // Evaluate the RHS expression *before* loading the current LHS value.
        // This reduces register pressure on the 8080 where W8 values live in A:
        // if we loaded the LHS first, computing a non-trivial RHS would spill
        // the LHS (since both need A).  By computing RHS first, the codegen can
        // often use the deferred-M mechanism (ADD M / SUB M) for the LHS load,
        // avoiding the spill entirely.  C permits this: the relative evaluation
        // order of the two operands is unspecified.
        let lv = self.gen_lvalue(target);
        let ty = self.lvalue_type(&lv);
        let (rhs, _) = self.gen_expr(value);
        let (cur, _) = self.load_lvalue(&lv);
        let w = Width::from_ctype(&ty).unwrap_or(Width::W16);
        let signed = ty.is_signed();
        let result = self.vreg_alloc.alloc(w);

        match op {
            AssignOp::AddAssign => self.emit(IrOp::Add { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::SubAssign => self.emit(IrOp::Sub { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::MulAssign => self.emit(IrOp::Mul { dst: result, lhs: cur, rhs, width: w, signed }),
            AssignOp::DivAssign => self.emit(IrOp::Div { dst: result, lhs: cur, rhs, width: w, signed }),
            AssignOp::ModAssign => self.emit(IrOp::Mod { dst: result, lhs: cur, rhs, width: w, signed }),
            AssignOp::BitAndAssign => self.emit(IrOp::And { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::BitOrAssign => self.emit(IrOp::Or { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::BitXorAssign => self.emit(IrOp::Xor { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::ShlAssign => self.emit(IrOp::Shl { dst: result, lhs: cur, rhs, width: w }),
            AssignOp::ShrAssign => self.emit(IrOp::Shr { dst: result, lhs: cur, rhs, width: w, arithmetic: signed }),
            AssignOp::Assign => unreachable!(),
        }

        self.store_lvalue(&lv, result);
        (result, ty)
    }

    // ---- function call ----------------------------------------------------

    fn gen_func_call(&mut self, callee: &Expr, args: &[Expr]) -> (VReg, CType) {
        let arg_regs: Vec<VReg> = args.iter().map(|a| self.gen_expr(a).0).collect();

        let (func_name, ret_ty) = match &callee.kind {
            ExprKind::Ident(name) => {
                let ret = self
                    .func_return_types
                    .get(name)
                    .cloned()
                    .unwrap_or(CType::int_signed());
                (name.clone(), ret)
            }
            _ => {
                self.error("indirect function calls not yet supported");
                ("_unknown".to_string(), CType::int_signed())
            }
        };

        if ret_ty.is_void() {
            self.emit(IrOp::call(&func_name, arg_regs, None));
            // Return a dummy value – the caller should not use it.
            let d = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::load_imm(d, 0));
            (d, CType::Void)
        } else {
            let d = self.vreg_alloc.alloc_for_type(&ret_ty);
            self.emit(IrOp::call(&func_name, arg_regs, Some(d)));
            (d, ret_ty)
        }
    }

    // ---- subscript --------------------------------------------------------

    fn gen_subscript(&mut self, array: &Expr, index: &Expr) -> (VReg, CType) {
        let (arr_reg, arr_ty) = self.gen_expr(array);
        let (idx_reg, _) = self.gen_expr(index);

        let (elem_ty, elem_size) = self.element_info(&arr_ty);

        let ptr = self.vreg_alloc.alloc(Width::W16);
        self.emit(IrOp::ptr_add(ptr, arr_reg, idx_reg, elem_size));

        let w = Width::from_ctype(&elem_ty).unwrap_or(Width::W16);
        let dst = self.vreg_alloc.alloc(w);
        self.emit(IrOp::load_ptr(dst, ptr));
        (dst, elem_ty)
    }

    /// Determine element type and byte size from a pointer or array type.
    fn element_info(&self, ty: &CType) -> (CType, u16) {
        if let Some(p) = ty.pointee() {
            (p.clone(), p.size_of().unwrap_or(1) as u16)
        } else if let Some(e) = ty.element_type() {
            (e.clone(), e.size_of().unwrap_or(1) as u16)
        } else {
            (CType::int_signed(), 2)
        }
    }

    // ---- member access (`.` and `->`) ------------------------------------

    fn gen_member_access(&mut self, object: &Expr, member: &str) -> (VReg, CType) {
        // Get the address of the struct object.
        let lv = self.gen_lvalue(object);
        let base_ty = self.lvalue_type(&lv);
        let base_ptr = match &lv {
            LValueResult::Global { label, .. } => {
                let p = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::addr_of_global(p, label));
                p
            }
            LValueResult::Ptr { reg, .. } => *reg,
        };
        self.access_field(base_ptr, &base_ty, member)
    }

    fn gen_ptr_member_access(&mut self, ptr_expr: &Expr, member: &str) -> (VReg, CType) {
        let (base_ptr, ptr_ty) = self.gen_expr(ptr_expr);
        let struct_ty = ptr_ty.pointee().cloned().unwrap_or(CType::int_signed());
        self.access_field(base_ptr, &struct_ty, member)
    }

    /// Given a pointer to a struct/union and a field name, load the field.
    fn access_field(&mut self, base_ptr: VReg, struct_ty: &CType, member: &str) -> (VReg, CType) {
        if let Some((offset, field_ty)) = struct_ty.field_offset(member) {
            let field_ptr = if offset == 0 {
                base_ptr
            } else {
                let off_reg = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::load_imm(off_reg, offset as i64));
                let p = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::Add {
                    dst: p,
                    lhs: base_ptr,
                    rhs: off_reg,
                    width: Width::W16,
                });
                p
            };
            // If the field is an array, decay to pointer (don't load).
            if field_ty.is_array() {
                let elem = field_ty.element_type().unwrap().clone();
                return (field_ptr, CType::ptr(elem));
            }
            // If the field is a struct/union, return its address as a pointer.
            if field_ty.is_struct_or_union() {
                return (field_ptr, field_ty);
            }
            let w = Width::from_ctype(&field_ty).unwrap_or(Width::W16);
            let dst = self.vreg_alloc.alloc(w);
            self.emit(IrOp::load_ptr(dst, field_ptr));
            (dst, field_ty)
        } else {
            self.error(&format!("no member '{}' in type", member));
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::load_imm(dst, 0));
            (dst, CType::int_signed())
        }
    }

    // ---- ternary conditional ----------------------------------------------

    fn gen_conditional(
        &mut self,
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> (VReg, CType) {
        let (cond_reg, _) = self.gen_expr(cond);
        let else_lbl = self.label_alloc.alloc();
        let end_lbl = self.label_alloc.alloc();

        self.emit(IrOp::jump_if_false(cond_reg, else_lbl));

        let (then_reg, then_ty) = self.gen_expr(then_expr);
        let result = self.vreg_alloc.alloc(then_reg.width);
        self.emit(IrOp::copy(result, then_reg));
        self.emit(IrOp::jump(end_lbl));

        self.emit(IrOp::label(else_lbl));
        let (else_reg, _) = self.gen_expr(else_expr);
        self.emit(IrOp::copy(result, else_reg));

        self.emit(IrOp::label(end_lbl));
        (result, then_ty)
    }

    // ---- pointer arithmetic -----------------------------------------------

    fn gen_ptr_arith(
        &mut self,
        ptr_reg: VReg,
        ptr_ty: &CType,
        offset_reg: VReg,
        is_add: bool,
    ) -> (VReg, CType) {
        let pointee = ptr_ty.pointee().cloned().unwrap_or(CType::Void);
        let elem_size = pointee.size_of().unwrap_or(1) as u16;

        if is_add {
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::ptr_add(dst, ptr_reg, offset_reg, elem_size));
            (dst, ptr_ty.clone())
        } else {
            // ptr - int ⇒ negate the offset then PtrAdd
            let neg = self.vreg_alloc.alloc(offset_reg.width);
            self.emit(IrOp::Neg {
                dst: neg,
                src: offset_reg,
                width: offset_reg.width,
            });
            let dst = self.vreg_alloc.alloc(Width::W16);
            self.emit(IrOp::ptr_add(dst, ptr_reg, neg, elem_size));
            (dst, ptr_ty.clone())
        }
    }

    // -----------------------------------------------------------------------
    // Float binary operations (via software library calls)
    // -----------------------------------------------------------------------

    fn gen_float_binop(
        &mut self,
        op: BinOp,
        lhs: VReg,
        rhs: VReg,
        _result_ty: &CType,
    ) -> (VReg, CType) {
        let func_name = match op {
            BinOp::Add => "__fadd",
            BinOp::Sub => "__fsub",
            BinOp::Mul => "__fmul",
            BinOp::Div => "__fdiv",
            BinOp::Eq => "__feq",
            BinOp::Ne => "__fne",
            BinOp::Lt => "__flt",
            BinOp::Le => "__fle",
            BinOp::Gt => "__fgt",
            BinOp::Ge => "__fge",
            _ => {
                self.error(&format!("unsupported float operation: {:?}", op));
                "__fadd"
            }
        };

        // Comparison operations return an int (0 or 1).
        let is_cmp = matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge);
        let ret_width = if is_cmp { Width::W16 } else { Width::W32 };
        let ret_type = if is_cmp { CType::int_signed() } else { CType::Float };

        let dst = self.vreg_alloc.alloc(ret_width);
        self.emit(IrOp::Call {
            func_name: func_name.to_string(),
            args: vec![lhs, rhs],
            dst: Some(dst),
        });
        (dst, ret_type)
    }

    // -----------------------------------------------------------------------
    // LValue helpers
    // -----------------------------------------------------------------------

    fn gen_lvalue(&mut self, expr: &Expr) -> LValueResult {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if let Some((label, ty)) = self.local_syms.get(name).cloned() {
                    LValueResult::Global { label, ty }
                } else if let Some((label, ty)) = self.global_syms.get(name).cloned() {
                    LValueResult::Global { label, ty }
                } else {
                    self.error(&format!("undefined variable: {}", name));
                    LValueResult::Global {
                        label: format!("_g_{}", name),
                        ty: CType::int_signed(),
                    }
                }
            }
            ExprKind::UnaryOp {
                op: UnaryOp::Deref,
                operand,
            } => {
                let (ptr, ptr_ty) = self.gen_expr(operand);
                let pointee = ptr_ty.pointee().cloned().unwrap_or(CType::int_signed());
                LValueResult::Ptr {
                    reg: ptr,
                    pointee_ty: pointee,
                }
            }
            ExprKind::Subscript { array, index } => {
                let (arr_reg, arr_ty) = self.gen_expr(array);
                let (idx_reg, _) = self.gen_expr(index);
                let (elem_ty, elem_size) = self.element_info(&arr_ty);
                let ptr = self.vreg_alloc.alloc(Width::W16);
                self.emit(IrOp::ptr_add(ptr, arr_reg, idx_reg, elem_size));
                LValueResult::Ptr {
                    reg: ptr,
                    pointee_ty: elem_ty,
                }
            }
            ExprKind::MemberAccess { object, member } => {
                let lv = self.gen_lvalue(object);
                let base_ty = self.lvalue_type(&lv);
                let base_ptr = match &lv {
                    LValueResult::Global { label, .. } => {
                        let p = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::addr_of_global(p, label));
                        p
                    }
                    LValueResult::Ptr { reg, .. } => *reg,
                };
                if let Some((offset, field_ty)) = base_ty.field_offset(member) {
                    let field_ptr = if offset == 0 {
                        base_ptr
                    } else {
                        let off_reg = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::load_imm(off_reg, offset as i64));
                        let p = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::Add {
                            dst: p,
                            lhs: base_ptr,
                            rhs: off_reg,
                            width: Width::W16,
                        });
                        p
                    };
                    LValueResult::Ptr {
                        reg: field_ptr,
                        pointee_ty: field_ty,
                    }
                } else {
                    self.error(&format!("no member '{}' in type", member));
                    LValueResult::Global {
                        label: "_error".to_string(),
                        ty: CType::int_signed(),
                    }
                }
            }
            ExprKind::PtrMemberAccess { ptr, member } => {
                let (base_ptr, ptr_ty) = self.gen_expr(ptr);
                let struct_ty = ptr_ty.pointee().cloned().unwrap_or(CType::int_signed());
                if let Some((offset, field_ty)) = struct_ty.field_offset(member) {
                    let field_ptr = if offset == 0 {
                        base_ptr
                    } else {
                        let off_reg = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::load_imm(off_reg, offset as i64));
                        let p = self.vreg_alloc.alloc(Width::W16);
                        self.emit(IrOp::Add {
                            dst: p,
                            lhs: base_ptr,
                            rhs: off_reg,
                            width: Width::W16,
                        });
                        p
                    };
                    LValueResult::Ptr {
                        reg: field_ptr,
                        pointee_ty: field_ty,
                    }
                } else {
                    self.error(&format!("no member '{}' in type", member));
                    LValueResult::Global {
                        label: "_error".to_string(),
                        ty: CType::int_signed(),
                    }
                }
            }
            _ => {
                self.error("expression is not an lvalue");
                LValueResult::Global {
                    label: "_error".to_string(),
                    ty: CType::int_signed(),
                }
            }
        }
    }

    fn load_lvalue(&mut self, lv: &LValueResult) -> (VReg, CType) {
        match lv {
            LValueResult::Global { label, ty } => {
                let w = Width::from_ctype(ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(w);
                self.emit(IrOp::load_global(dst, label));
                (dst, ty.clone())
            }
            LValueResult::Ptr { reg, pointee_ty } => {
                let w = Width::from_ctype(pointee_ty).unwrap_or(Width::W16);
                let dst = self.vreg_alloc.alloc(w);
                self.emit(IrOp::load_ptr(dst, *reg));
                (dst, pointee_ty.clone())
            }
        }
    }

    fn store_lvalue(&mut self, lv: &LValueResult, src: VReg) {
        match lv {
            LValueResult::Global { label, .. } => {
                self.emit(IrOp::store_global(label, src));
            }
            LValueResult::Ptr { reg, .. } => {
                self.emit(IrOp::store_ptr(*reg, src));
            }
        }
    }

    fn lvalue_type(&self, lv: &LValueResult) -> CType {
        match lv {
            LValueResult::Global { ty, .. } => ty.clone(),
            LValueResult::Ptr { pointee_ty, .. } => pointee_ty.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        AssignOp, BinOp, Expr, ExprKind, Param, Program, SizeOfArg, SourceLocation, Stmt,
        StmtKind, TopLevel, TopLevelKind, UnaryOp,
    };
    use crate::ir::IrOp;
    use crate::types::CType;

    // -- helpers ----------------------------------------------------------

    fn loc(line: u32) -> SourceLocation {
        SourceLocation::new(line, 1)
    }

    fn int_lit(val: i64) -> Expr {
        Expr::new(ExprKind::IntLiteral(val), loc(1))
    }

    fn char_lit(val: u8) -> Expr {
        Expr::new(ExprKind::CharLiteral(val), loc(1))
    }

    fn ident(name: &str) -> Expr {
        Expr::new(ExprKind::Ident(name.into()), loc(1))
    }

    fn string_lit(s: &[u8]) -> Expr {
        Expr::new(ExprKind::StringLiteral(s.to_vec()), loc(1))
    }

    fn binop(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::new(
            ExprKind::BinOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            loc(1),
        )
    }

    fn unaryop(op: UnaryOp, operand: Expr) -> Expr {
        Expr::new(
            ExprKind::UnaryOp {
                op,
                operand: Box::new(operand),
            },
            loc(1),
        )
    }

    fn assign(op: AssignOp, target: Expr, value: Expr) -> Expr {
        Expr::new(
            ExprKind::Assign {
                op,
                target: Box::new(target),
                value: Box::new(value),
            },
            loc(1),
        )
    }

    fn call(name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            ExprKind::FuncCall {
                callee: Box::new(ident(name)),
                args,
            },
            loc(1),
        )
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt::new(StmtKind::Return(val), loc(1))
    }

    fn expr_stmt(expr: Expr) -> Stmt {
        Stmt::new(StmtKind::Expr(expr), loc(1))
    }

    fn compound(stmts: Vec<Stmt>) -> Stmt {
        Stmt::new(StmtKind::Compound(stmts), loc(1))
    }

    /// Build a minimal `Program` with one function definition.
    fn one_func(
        name: &str,
        ret: CType,
        params: Vec<Param>,
        body: Stmt,
    ) -> Program {
        Program::from_decls(vec![TopLevel::new(
            TopLevelKind::FuncDef {
                name: name.into(),
                return_type: ret,
                params,
                storage: None,
                is_variadic: false,
                body,
            },
            loc(1),
        )])
    }

    /// Count occurrences of a particular IrOp variant in a function body.
    fn count_ops(func: &IrFunction, pred: fn(&IrOp) -> bool) -> usize {
        func.body.iter().filter(|i| pred(&i.op)).count()
    }

    // =====================================================================
    // Global variables
    // =====================================================================

    #[test]
    fn global_var_uninitialized() {
        let prog = Program::from_decls(vec![TopLevel::new(
            TopLevelKind::GlobalVar {
                name: "x".into(),
                ty: CType::int_signed(),
                storage: None,
                init: None,
            },
            loc(1),
        )]);
        let ir = generate(&prog).unwrap();
        assert_eq!(ir.globals.len(), 1);
        assert_eq!(ir.globals[0].name, "_g_x");
        assert!(ir.globals[0].init.is_none());
    }

    #[test]
    fn global_var_with_init() {
        let prog = Program::from_decls(vec![TopLevel::new(
            TopLevelKind::GlobalVar {
                name: "y".into(),
                ty: CType::int_signed(),
                storage: None,
                init: Some(int_lit(42)),
            },
            loc(1),
        )]);
        let ir = generate(&prog).unwrap();
        assert_eq!(ir.globals[0].name, "_g_y");
        // 42 in little-endian 16-bit = [42, 0]
        assert_eq!(ir.globals[0].init, Some(vec![42, 0]));
    }

    // =====================================================================
    // Simple function – return constant
    // =====================================================================

    #[test]
    fn func_return_constant() {
        let prog = one_func(
            "main",
            CType::int_signed(),
            vec![],
            compound(vec![return_stmt(Some(int_lit(0)))]),
        );
        let ir = generate(&prog).unwrap();
        assert_eq!(ir.functions.len(), 1);
        let f = &ir.functions[0];
        assert_eq!(f.name, "main");
        assert!(!f.is_stack_mode);

        // Expect: LoadImm 0, Return
        assert!(f.body.iter().any(|i| matches!(i.op, IrOp::LoadImm { value: 0, .. })));
        assert!(f.body.iter().any(|i| matches!(i.op, IrOp::Return { value: Some(_) })));
    }

    // =====================================================================
    // Implicit return for void function
    // =====================================================================

    #[test]
    fn void_func_implicit_return() {
        let prog = one_func("noop", CType::Void, vec![], compound(vec![]));
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // The last instruction must be a void return.
        let last = f.body.last().unwrap();
        assert_eq!(last.op, IrOp::Return { value: None });
    }

    // =====================================================================
    // Function parameters become static locals
    // =====================================================================

    #[test]
    fn func_params_stored_as_globals() {
        let prog = one_func(
            "add",
            CType::int_signed(),
            vec![
                Param { name: Some("a".into()), ty: CType::int_signed() },
                Param { name: Some("b".into()), ty: CType::int_signed() },
            ],
            compound(vec![return_stmt(Some(binop(
                BinOp::Add,
                ident("a"),
                ident("b"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        // Parameters create global storage labels.
        assert!(ir.globals.iter().any(|g| g.name == "_l_add_a"));
        assert!(ir.globals.iter().any(|g| g.name == "_l_add_b"));
        // First two body instructions should be StoreGlobal for params.
        let f = &ir.functions[0];
        assert!(matches!(&f.body[0].op, IrOp::StoreGlobal { addr_label, .. } if addr_label == "_l_add_a"));
        assert!(matches!(&f.body[1].op, IrOp::StoreGlobal { addr_label, .. } if addr_label == "_l_add_b"));
    }

    // =====================================================================
    // Binary arithmetic – a + b
    // =====================================================================

    #[test]
    fn binary_add() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![
                Param { name: Some("a".into()), ty: CType::int_signed() },
                Param { name: Some("b".into()), ty: CType::int_signed() },
            ],
            compound(vec![return_stmt(Some(binop(
                BinOp::Add,
                ident("a"),
                ident("b"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::Add { .. })) >= 1);
    }

    // =====================================================================
    // Comparison
    // =====================================================================

    #[test]
    fn comparison_lt() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![
                Param { name: Some("a".into()), ty: CType::int_signed() },
                Param { name: Some("b".into()), ty: CType::int_signed() },
            ],
            compound(vec![return_stmt(Some(binop(
                BinOp::Lt,
                ident("a"),
                ident("b"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::Lt { .. })) >= 1);
    }

    // =====================================================================
    // If / else
    // =====================================================================

    #[test]
    fn if_else_lowering() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![Stmt::new(
                StmtKind::If {
                    cond: ident("x"),
                    then_body: Box::new(return_stmt(Some(int_lit(1)))),
                    else_body: Some(Box::new(return_stmt(Some(int_lit(2))))),
                },
                loc(1),
            )]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Expect JumpIfFalse, Jump (unconditional), two Label pseudo-ops.
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfFalse { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::Label { .. })) >= 2);
    }

    // =====================================================================
    // While loop
    // =====================================================================

    #[test]
    fn while_loop() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![
                Stmt::new(
                    StmtKind::VarDecl {
                        name: "i".into(),
                        ty: CType::int_signed(),
                        storage: None,
                        init: Some(int_lit(0)),
                    },
                    loc(1),
                ),
                Stmt::new(
                    StmtKind::While {
                        cond: binop(BinOp::Lt, ident("i"), int_lit(10)),
                        body: Box::new(expr_stmt(assign(
                            AssignOp::AddAssign,
                            ident("i"),
                            int_lit(1),
                        ))),
                        unroll_hint: false,
                    },
                    loc(2),
                ),
            ]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Two labels (loop start, loop end), one JumpIfFalse, one Jump back.
        assert!(count_ops(f, |op| matches!(op, IrOp::Label { .. })) >= 2);
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfFalse { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::Jump { .. })) >= 1);
    }

    // =====================================================================
    // For loop
    // =====================================================================

    #[test]
    fn for_loop() {
        let init = Stmt::new(
            StmtKind::VarDecl {
                name: "i".into(),
                ty: CType::int_signed(),
                storage: None,
                init: Some(int_lit(0)),
            },
            loc(1),
        );
        let cond = binop(BinOp::Lt, ident("i"), int_lit(10));
        let step = unaryop(UnaryOp::PostInc, ident("i"));
        let body = compound(vec![]);
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![Stmt::new(
                StmtKind::For {
                    init: Some(Box::new(init)),
                    cond: Some(cond),
                    step: Some(step),
                    body: Box::new(body),
                    unroll_hint: false,
                },
                loc(1),
            )]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Three labels: start, continue, end.
        assert!(count_ops(f, |op| matches!(op, IrOp::Label { .. })) >= 3);
    }

    #[test]
    fn for_loop_unroll_hint_recorded() {
        let body = compound(vec![]);
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![Stmt::new(
                StmtKind::For {
                    init: None,
                    cond: None,
                    step: None,
                    body: Box::new(body),
                    unroll_hint: true,
                },
                loc(1),
            )]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(!f.unroll_loop_headers.is_empty());
    }

    // =====================================================================
    // Do-while loop
    // =====================================================================

    #[test]
    fn do_while_loop() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![
                Stmt::new(
                    StmtKind::VarDecl {
                        name: "i".into(),
                        ty: CType::int_signed(),
                        storage: None,
                        init: Some(int_lit(0)),
                    },
                    loc(1),
                ),
                Stmt::new(
                    StmtKind::DoWhile {
                        body: Box::new(expr_stmt(assign(
                            AssignOp::AddAssign,
                            ident("i"),
                            int_lit(1),
                        ))),
                        cond: binop(BinOp::Lt, ident("i"), int_lit(10)),
                        unroll_hint: false,
                    },
                    loc(2),
                ),
            ]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfTrue { .. })) >= 1);
    }

    // =====================================================================
    // Break and continue
    // =====================================================================

    #[test]
    fn break_in_while() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![Stmt::new(
                StmtKind::While {
                    cond: int_lit(1),
                    body: Box::new(Stmt::new(StmtKind::Break, loc(1))),
                    unroll_hint: false,
                },
                loc(1),
            )]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // break emits a Jump to the loop-end label.
        let jump_count = count_ops(f, |op| matches!(op, IrOp::Jump { .. }));
        assert!(jump_count >= 2); // break jump + back-edge jump
    }

    #[test]
    fn continue_in_while() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![Stmt::new(
                StmtKind::While {
                    cond: int_lit(1),
                    body: Box::new(Stmt::new(StmtKind::Continue, loc(1))),
                    unroll_hint: false,
                },
                loc(1),
            )]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // continue emits a Jump to the loop-start label.
        let jump_count = count_ops(f, |op| matches!(op, IrOp::Jump { .. }));
        assert!(jump_count >= 2);
    }

    // =====================================================================
    // String literal
    // =====================================================================

    #[test]
    fn string_literal_stored_and_referenced() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![],
            compound(vec![return_stmt(Some(string_lit(b"hello")))]),
        );
        let ir = generate(&prog).unwrap();
        assert_eq!(ir.strings.len(), 1);
        assert_eq!(ir.strings[0].label, "_S0");
        assert_eq!(ir.strings[0].data, b"hello\0");
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::AddrOfGlobal { .. })) >= 1);
    }

    // =====================================================================
    // Function call
    // =====================================================================

    #[test]
    fn function_call_with_args() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::FuncDecl {
                    name: "add".into(),
                    return_type: CType::int_signed(),
                    params: vec![
                        Param { name: Some("a".into()), ty: CType::int_signed() },
                        Param { name: Some("b".into()), ty: CType::int_signed() },
                    ],
                    storage: None,
                    is_variadic: false,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "main".into(),
                    return_type: CType::int_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(call(
                        "add",
                        vec![int_lit(1), int_lit(2)],
                    )))]),
                },
                loc(5),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        let call_ops: Vec<_> = f
            .body
            .iter()
            .filter(|i| matches!(&i.op, IrOp::Call { .. }))
            .collect();
        assert_eq!(call_ops.len(), 1);
        if let IrOp::Call { func_name, args, dst } = &call_ops[0].op {
            assert_eq!(func_name, "add");
            assert_eq!(args.len(), 2);
            assert!(dst.is_some());
        }
    }

    // =====================================================================
    // Assignment
    // =====================================================================

    #[test]
    fn simple_assignment() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![expr_stmt(assign(
                        AssignOp::Assign,
                        ident("x"),
                        int_lit(42),
                    ))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(f.body.iter().any(|i| matches!(
            &i.op,
            IrOp::StoreGlobal { addr_label, .. } if addr_label == "_g_x"
        )));
    }

    // =====================================================================
    // Compound assignment (+=)
    // =====================================================================

    #[test]
    fn compound_add_assign() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![expr_stmt(assign(
                        AssignOp::AddAssign,
                        ident("x"),
                        int_lit(5),
                    ))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Expect: LoadGlobal _g_x, LoadImm 5, Add, StoreGlobal _g_x
        assert!(count_ops(f, |op| matches!(op, IrOp::LoadGlobal { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::Add { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::StoreGlobal { .. })) >= 1);
    }

    // =====================================================================
    // Pre-increment
    // =====================================================================

    #[test]
    fn pre_increment() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::int_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(unaryop(
                        UnaryOp::PreInc,
                        ident("x"),
                    )))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Load, LoadImm 1, Add, StoreGlobal, Return(new value)
        assert!(count_ops(f, |op| matches!(op, IrOp::Add { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::StoreGlobal { .. })) >= 1);
    }

    // =====================================================================
    // Post-increment returns old value
    // =====================================================================

    #[test]
    fn post_increment_returns_old() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::int_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(unaryop(
                        UnaryOp::PostInc,
                        ident("x"),
                    )))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // The Return instruction should reference the *old* value register,
        // not the post-incremented one.
        let ret_instr = f.body.iter().find(|i| matches!(i.op, IrOp::Return { .. })).unwrap();
        if let IrOp::Return { value: Some(ret_reg) } = &ret_instr.op {
            // The add result should be a different register.
            let add_instr = f.body.iter().find(|i| matches!(i.op, IrOp::Add { .. })).unwrap();
            if let IrOp::Add { dst: add_dst, lhs: old_reg, .. } = &add_instr.op {
                assert_eq!(*ret_reg, *old_reg); // return old value
                assert_ne!(*ret_reg, *add_dst); // not the new value
            }
        }
    }

    // =====================================================================
    // Short-circuit && and ||
    // =====================================================================

    #[test]
    fn short_circuit_and() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![
                Param { name: Some("a".into()), ty: CType::int_signed() },
                Param { name: Some("b".into()), ty: CType::int_signed() },
            ],
            compound(vec![return_stmt(Some(binop(
                BinOp::LogAnd,
                ident("a"),
                ident("b"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Should have two JumpIfFalse instructions (one per operand).
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfFalse { .. })) >= 2);
        // Two LoadImm for 0 and 1.
        let imm_count = count_ops(f, |op| matches!(op, IrOp::LoadImm { .. }));
        assert!(imm_count >= 2);
    }

    #[test]
    fn short_circuit_or() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![
                Param { name: Some("a".into()), ty: CType::int_signed() },
                Param { name: Some("b".into()), ty: CType::int_signed() },
            ],
            compound(vec![return_stmt(Some(binop(
                BinOp::LogOr,
                ident("a"),
                ident("b"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfTrue { .. })) >= 2);
    }

    // =====================================================================
    // Ternary conditional
    // =====================================================================

    #[test]
    fn ternary_conditional() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![return_stmt(Some(Expr::new(
                ExprKind::Conditional {
                    cond: Box::new(ident("x")),
                    then_expr: Box::new(int_lit(1)),
                    else_expr: Box::new(int_lit(2)),
                },
                loc(1),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::JumpIfFalse { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::Copy { .. })) >= 2);
    }

    // =====================================================================
    // Address-of and dereference
    // =====================================================================

    #[test]
    fn addr_of_global() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::ptr(CType::int_signed()),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(unaryop(
                        UnaryOp::AddrOf,
                        ident("x"),
                    )))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(f.body.iter().any(|i| matches!(
            &i.op,
            IrOp::AddrOfGlobal { name, .. } if name == "_g_x"
        )));
    }

    #[test]
    fn dereference_pointer() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param {
                name: Some("p".into()),
                ty: CType::ptr(CType::int_signed()),
            }],
            compound(vec![return_stmt(Some(unaryop(
                UnaryOp::Deref,
                ident("p"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::LoadPtr { .. })) >= 1);
    }

    // =====================================================================
    // Array subscript
    // =====================================================================

    #[test]
    fn array_subscript() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "arr".into(),
                    ty: CType::array(CType::int_signed(), 10),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::int_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(Expr::new(
                        ExprKind::Subscript {
                            array: Box::new(ident("arr")),
                            index: Box::new(int_lit(3)),
                        },
                        loc(1),
                    )))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Array ident decays: AddrOfGlobal, then PtrAdd, then LoadPtr.
        assert!(count_ops(f, |op| matches!(op, IrOp::AddrOfGlobal { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::PtrAdd { element_size: 2, .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::LoadPtr { .. })) >= 1);
    }

    // =====================================================================
    // sizeof
    // =====================================================================

    #[test]
    fn sizeof_type() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![],
            compound(vec![return_stmt(Some(Expr::new(
                ExprKind::SizeOf(SizeOfArg::Type(CType::long_signed())),
                loc(1),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(f.body.iter().any(|i| matches!(i.op, IrOp::LoadImm { value: 4, .. })));
    }

    // =====================================================================
    // Unary negate and bitwise not
    // =====================================================================

    #[test]
    fn unary_negate() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![return_stmt(Some(unaryop(
                UnaryOp::Negate,
                ident("x"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::Neg { .. })) >= 1);
    }

    #[test]
    fn unary_bitnot() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![return_stmt(Some(unaryop(
                UnaryOp::BitNot,
                ident("x"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::Not { .. })) >= 1);
    }

    #[test]
    fn unary_logical_not() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![return_stmt(Some(unaryop(
                UnaryOp::LogNot,
                ident("x"),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::LogicalNot { .. })) >= 1);
    }

    // =====================================================================
    // Local variable declaration with initializer
    // =====================================================================

    #[test]
    fn local_var_decl() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![],
            compound(vec![
                Stmt::new(
                    StmtKind::VarDecl {
                        name: "x".into(),
                        ty: CType::int_signed(),
                        storage: None,
                        init: Some(int_lit(7)),
                    },
                    loc(1),
                ),
                return_stmt(Some(ident("x"))),
            ]),
        );
        let ir = generate(&prog).unwrap();
        // The local creates a global storage slot.
        assert!(ir.globals.iter().any(|g| g.name == "_l_f_x"));
        let f = &ir.functions[0];
        // Should store the initializer.
        assert!(f.body.iter().any(|i| matches!(
            &i.op,
            IrOp::StoreGlobal { addr_label, .. } if addr_label == "_l_f_x"
        )));
    }

    // =====================================================================
    // Goto / label
    // =====================================================================

    #[test]
    fn goto_and_label() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![
                Stmt::new(StmtKind::Goto("end".into()), loc(1)),
                Stmt::new(
                    StmtKind::Label {
                        name: "end".into(),
                        stmt: Box::new(Stmt::new(StmtKind::Return(None), loc(3))),
                    },
                    loc(2),
                ),
            ]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // The goto and label should reference the same Label id.
        let jumps: Vec<_> = f.body.iter().filter_map(|i| {
            if let IrOp::Jump { target } = &i.op { Some(*target) } else { None }
        }).collect();
        let labels: Vec<_> = f.body.iter().filter_map(|i| {
            if let IrOp::Label { label } = &i.op { Some(*label) } else { None }
        }).collect();
        assert!(!jumps.is_empty());
        // The first Jump target should appear among the labels.
        assert!(labels.contains(&jumps[0]));
    }

    // =====================================================================
    // Comma expression
    // =====================================================================

    #[test]
    fn comma_expression() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![],
            compound(vec![return_stmt(Some(Expr::new(
                ExprKind::Comma {
                    left: Box::new(int_lit(1)),
                    right: Box::new(int_lit(2)),
                },
                loc(1),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Both values are loaded; the return uses the second one.
        let load_imms: Vec<_> = f.body.iter().filter_map(|i| {
            if let IrOp::LoadImm { value, dst } = &i.op { Some((*value, *dst)) } else { None }
        }).collect();
        assert!(load_imms.len() >= 2);
        // The last LoadImm before Return should be 2.
        if let IrOp::Return { value: Some(ret_reg) } = &f.body.iter().rev()
            .find(|i| matches!(i.op, IrOp::Return { .. })).unwrap().op
        {
            let second = load_imms.iter().find(|(v, _)| *v == 2).unwrap();
            assert_eq!(*ret_reg, second.1);
        }
    }

    // =====================================================================
    // Cast expression
    // =====================================================================

    #[test]
    fn cast_expression() {
        let prog = one_func(
            "f",
            CType::long_signed(),
            vec![Param { name: Some("x".into()), ty: CType::int_signed() }],
            compound(vec![return_stmt(Some(Expr::new(
                ExprKind::Cast {
                    ty: CType::long_signed(),
                    expr: Box::new(ident("x")),
                },
                loc(1),
            )))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        assert!(count_ops(f, |op| matches!(op, IrOp::Cast { .. })) >= 1);
    }

    // =====================================================================
    // Char literal
    // =====================================================================

    #[test]
    fn char_literal_width() {
        let prog = one_func(
            "f",
            CType::int_signed(),
            vec![],
            compound(vec![return_stmt(Some(char_lit(b'A')))]),
        );
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        let imm = f.body.iter().find(|i| matches!(i.op, IrOp::LoadImm { .. })).unwrap();
        if let IrOp::LoadImm { dst, value } = &imm.op {
            assert_eq!(*value, 65);
            assert_eq!(dst.width, Width::W8);
        }
    }

    // =====================================================================
    // Function declaration registers return type
    // =====================================================================

    #[test]
    fn func_decl_registers_type() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::FuncDecl {
                    name: "ext".into(),
                    return_type: CType::long_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::long_signed(),
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![return_stmt(Some(call("ext", vec![])))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        let call_instr = f.body.iter().find(|i| matches!(i.op, IrOp::Call { .. })).unwrap();
        if let IrOp::Call { dst: Some(d), .. } = &call_instr.op {
            assert_eq!(d.width, Width::W32); // long = 32-bit
        } else {
            panic!("expected call with dst");
        }
    }

    // =====================================================================
    // Error: break outside loop
    // =====================================================================

    #[test]
    fn break_outside_loop_is_error() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![Stmt::new(StmtKind::Break, loc(1))]),
        );
        let result = generate(&prog);
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("break")));
    }

    // =====================================================================
    // Multiple string literals get unique labels
    // =====================================================================

    #[test]
    fn multiple_string_literals() {
        let prog = one_func(
            "f",
            CType::Void,
            vec![],
            compound(vec![
                expr_stmt(string_lit(b"aaa")),
                expr_stmt(string_lit(b"bbb")),
            ]),
        );
        let ir = generate(&prog).unwrap();
        assert_eq!(ir.strings.len(), 2);
        assert_eq!(ir.strings[0].label, "_S0");
        assert_eq!(ir.strings[1].label, "_S1");
    }

    // =====================================================================
    // Subscript as lvalue (arr[i] = val)
    // =====================================================================

    #[test]
    fn subscript_lvalue() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "arr".into(),
                    ty: CType::array(CType::int_signed(), 10),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![expr_stmt(assign(
                        AssignOp::Assign,
                        Expr::new(
                            ExprKind::Subscript {
                                array: Box::new(ident("arr")),
                                index: Box::new(int_lit(0)),
                            },
                            loc(1),
                        ),
                        int_lit(42),
                    ))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        // Expect PtrAdd + StorePtr for the assignment.
        assert!(count_ops(f, |op| matches!(op, IrOp::PtrAdd { .. })) >= 1);
        assert!(count_ops(f, |op| matches!(op, IrOp::StorePtr { .. })) >= 1);
    }

    // =====================================================================
    // Void function call
    // =====================================================================

    #[test]
    fn void_function_call() {
        let prog = Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::FuncDecl {
                    name: "noop".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "f".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![expr_stmt(call("noop", vec![]))]),
                },
                loc(2),
            ),
        ]);
        let ir = generate(&prog).unwrap();
        let f = &ir.functions[0];
        let call_instr = f.body.iter().find(|i| matches!(i.op, IrOp::Call { .. })).unwrap();
        if let IrOp::Call { dst, .. } = &call_instr.op {
            assert!(dst.is_none());
        }
    }

    // =====================================================================
    // Literal overflow / type-range checks
    // =====================================================================

    /// Helper: build a program assigning `val` to a global of type `ty`.
    fn overflow_prog(ty: CType, val: i64) -> Program {
        Program::from_decls(vec![
            TopLevel::new(
                TopLevelKind::GlobalVar {
                    name: "x".into(),
                    ty: ty.clone(),
                    storage: None,
                    init: None,
                },
                loc(1),
            ),
            TopLevel::new(
                TopLevelKind::FuncDef {
                    name: "main".into(),
                    return_type: CType::Void,
                    params: vec![],
                    storage: None,
                    is_variadic: false,
                    body: compound(vec![expr_stmt(assign(
                        AssignOp::Assign,
                        ident("x"),
                        int_lit(val),
                    ))]),
                },
                loc(2),
            ),
        ])
    }

    #[test]
    fn overflow_char_signed_too_large() {
        let err = generate(&overflow_prog(CType::char_signed(), 128))
            .expect_err("should fail");
        assert!(
            err[0].message.contains("128") && err[0].message.contains("char"),
            "unexpected error: {}", err[0].message
        );
    }

    #[test]
    fn overflow_char_signed_too_small() {
        let err = generate(&overflow_prog(CType::char_signed(), -129))
            .expect_err("should fail");
        assert!(
            err[0].message.contains("-129") && err[0].message.contains("char"),
            "unexpected error: {}", err[0].message
        );
    }

    #[test]
    fn overflow_char_unsigned_too_large() {
        let err = generate(&overflow_prog(CType::char_unsigned(), 256))
            .expect_err("should fail");
        assert!(
            err[0].message.contains("256") && err[0].message.contains("char"),
            "unexpected error: {}", err[0].message
        );
    }

    #[test]
    fn overflow_char_unsigned_negative() {
        let err = generate(&overflow_prog(CType::char_unsigned(), -1))
            .expect_err("should fail");
        assert!(
            err[0].message.contains("-1") && err[0].message.contains("char"),
            "unexpected error: {}", err[0].message
        );
    }

    #[test]
    fn overflow_int_unsigned_negative() {
        let err = generate(&overflow_prog(CType::int_unsigned(), -1))
            .expect_err("should fail");
        assert!(
            err[0].message.contains("-1") && err[0].message.contains("int"),
            "unexpected error: {}", err[0].message
        );
    }

    #[test]
    fn no_overflow_char_signed_boundary() {
        // 127 and -128 are exactly at the boundary — must succeed.
        generate(&overflow_prog(CType::char_signed(), 127)).expect("127 fits signed char");
        generate(&overflow_prog(CType::char_signed(), -128)).expect("-128 fits signed char");
    }

    #[test]
    fn no_overflow_char_unsigned_boundary() {
        // 0 and 255 fit unsigned char.
        generate(&overflow_prog(CType::char_unsigned(), 0)).expect("0 fits unsigned char");
        generate(&overflow_prog(CType::char_unsigned(), 255)).expect("255 fits unsigned char");
    }

    #[test]
    fn no_overflow_int_signed() {
        generate(&overflow_prog(CType::int_signed(), 32767)).expect("32767 fits signed int");
        generate(&overflow_prog(CType::int_signed(), -32768)).expect("-32768 fits signed int");
    }
}
