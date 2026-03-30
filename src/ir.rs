//! Three-address code (TAC) Intermediate Representation for the v6c compiler.
//!
//! The IR uses virtual registers (unlimited supply) with explicit loads and
//! stores.  Every binary/unary operation carries a [`Width`] tag so the code
//! generator can emit the correct 8-bit, 16-bit, or 32-bit instruction
//! sequence for the Intel 8080.
//!
//! ```text
//! t1 = LOAD_GLOBAL  addr        // load from named address
//! t2 = ADD.w16  t1, t3          // 16-bit add
//!      STORE_GLOBAL addr, t2    // store to named address
//!      IF_FALSE t2, label       // conditional branch
//!      CALL func, t1, t2        // function call
//! t3 = CALL_RESULT              // retrieve return value
//! ```

use std::fmt;

use crate::types::CType;

// ---------------------------------------------------------------------------
// Width — operation width tag
// ---------------------------------------------------------------------------

/// Width of an IR operation, matching the 8080 data sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Width {
    /// 8-bit  (`char`)
    W8,
    /// 16-bit (`int`, pointer)
    W16,
    /// 32-bit (`long`) — synthesised from register pairs on the 8080
    W32,
}

impl Width {
    /// Number of bytes for this width.
    pub fn bytes(self) -> usize {
        match self {
            Width::W8 => 1,
            Width::W16 => 2,
            Width::W32 => 4,
        }
    }

    /// Derive the appropriate width from a [`CType`].
    ///
    /// Pointers are 16-bit on the 8080.  Returns `None` for types with no
    /// meaningful scalar width (`Void`, `Array`, `Function`).
    pub fn from_ctype(ty: &CType) -> Option<Width> {
        match ty {
            CType::Char { .. } => Some(Width::W8),
            CType::Int { .. } | CType::Pointer(_) | CType::Enum { .. } => Some(Width::W16),
            CType::Long { .. } | CType::Float => Some(Width::W32),
            _ => None,
        }
    }
}

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Width::W8 => write!(f, "w8"),
            Width::W16 => write!(f, "w16"),
            Width::W32 => write!(f, "w32"),
        }
    }
}

// ---------------------------------------------------------------------------
// VReg — virtual register
// ---------------------------------------------------------------------------

/// A virtual register identified by a unique index.
///
/// Each `VReg` also carries the [`Width`] of the value it holds so that
/// downstream passes know the data size without extra lookups.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct VReg {
    pub id: u32,
    pub width: Width,
}

impl VReg {
    pub fn new(id: u32, width: Width) -> Self {
        Self { id, width }
    }
}

impl fmt::Debug for VReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}.{}", self.id, self.width)
    }
}

impl fmt::Display for VReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}.{}", self.id, self.width)
    }
}

// ---------------------------------------------------------------------------
// Label — branch target
// ---------------------------------------------------------------------------

/// An IR label used as a branch target.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Label(pub u32);

impl Label {
    pub fn new(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Debug for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// IrOp — every IR operation
// ---------------------------------------------------------------------------

/// A single IR operation (three-address code style).
#[derive(Debug, Clone, PartialEq)]
pub enum IrOp {
    // -- loads / stores ---------------------------------------------------

    /// Load an immediate constant into `dst`.
    LoadImm {
        dst: VReg,
        value: i64,
    },

    /// Load from a named global symbol.
    LoadGlobal {
        dst: VReg,
        addr_label: String,
    },

    /// Store to a named global symbol.
    StoreGlobal {
        addr_label: String,
        src: VReg,
    },

    /// Load from a stack-frame local at the given byte offset from the frame
    /// pointer.
    LoadLocal {
        dst: VReg,
        offset: i32,
    },

    /// Store to a stack-frame local.
    StoreLocal {
        offset: i32,
        src: VReg,
    },

    /// Dereference a pointer: `dst = *ptr`.
    LoadPtr {
        dst: VReg,
        ptr: VReg,
    },

    /// Store through a pointer: `*ptr = src`.
    StorePtr {
        ptr: VReg,
        src: VReg,
    },

    // -- binary arithmetic ------------------------------------------------

    Add { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Sub { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Mul { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },
    Div { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },
    Mod { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },

    // -- bitwise ----------------------------------------------------------

    And { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Or  { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Xor { dst: VReg, lhs: VReg, rhs: VReg, width: Width },

    /// Logical (unsigned) shift left.
    Shl { dst: VReg, lhs: VReg, rhs: VReg, width: Width },

    /// Shift right — `arithmetic` = true for sign-extending (signed) shifts.
    Shr { dst: VReg, lhs: VReg, rhs: VReg, width: Width, arithmetic: bool },

    // -- comparison (result is a W8 boolean: 0 or 1) ----------------------

    Eq { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Ne { dst: VReg, lhs: VReg, rhs: VReg, width: Width },
    Lt { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },
    Le { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },
    Gt { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },
    Ge { dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool },

    // -- unary ------------------------------------------------------------

    /// Two's-complement negation: `dst = -src`.
    Neg { dst: VReg, src: VReg, width: Width },

    /// Bitwise NOT: `dst = ~src`.
    Not { dst: VReg, src: VReg, width: Width },

    /// Logical NOT: `dst = !src` (result is W8 boolean).
    LogicalNot { dst: VReg, src: VReg, width: Width },

    // -- moves / conversions ----------------------------------------------

    /// Register-to-register copy (same width).
    Copy { dst: VReg, src: VReg },

    /// Type conversion (widen, narrow, or sign-change).
    Cast {
        dst: VReg,
        src: VReg,
        to_type: CType,
    },

    // -- control flow -----------------------------------------------------

    /// Unconditional jump.
    Jump { target: Label },

    /// Branch if `cond` is non-zero.
    JumpIfTrue { cond: VReg, target: Label },

    /// Branch if `cond` is zero.
    JumpIfFalse { cond: VReg, target: Label },

    /// Function call.  `dst` is `None` for void calls.
    Call {
        func_name: String,
        args: Vec<VReg>,
        dst: Option<VReg>,
    },

    /// Return from the current function.  `value` is `None` for void return.
    Return { value: Option<VReg> },

    /// Label definition (a pseudo-instruction marking a branch target).
    Label { label: Label },

    // -- address-of -------------------------------------------------------

    /// Load the address of a named global variable or function into `dst`.
    AddrOfGlobal {
        dst: VReg,
        name: String,
    },

    // -- pointer arithmetic -----------------------------------------------

    /// `dst = ptr + offset * element_size`.
    ///
    /// `element_size` is the byte size of the pointed-to type so the code
    /// generator can scale `offset` appropriately.
    PtrAdd {
        dst: VReg,
        ptr: VReg,
        offset: VReg,
        element_size: u16,
    },

    /// Inline assembly block.  The raw assembly text is emitted verbatim.
    InlineAsm {
        /// Raw assembly text.
        code: String,
        /// Typed input vregs from the parameter list.
        /// Empty for raw `asm { }` blocks (clobber-all).
        inputs: Vec<(VReg, CType)>,
        /// Return type if `-> type` was specified.
        return_type: Option<CType>,
        /// True for raw `asm { }` (no parens) — spill/clobber all registers.
        /// False for `asm(...) { }` — selective spill based on params.
        clobber_all: bool,
    },
}

// ---------------------------------------------------------------------------
// IrInstr — instruction with source location
// ---------------------------------------------------------------------------

/// An IR instruction paired with source-location information for diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct IrInstr {
    pub op: IrOp,
    /// Source line number (0 if unknown).
    pub line: u32,
}

impl IrInstr {
    pub fn new(op: IrOp, line: u32) -> Self {
        Self { op, line }
    }

    /// Build an instruction with no source location.
    pub fn bare(op: IrOp) -> Self {
        Self { op, line: 0 }
    }
}

// ---------------------------------------------------------------------------
// IrFunction
// ---------------------------------------------------------------------------

/// Parameter descriptor used in [`IrFunction`].
#[derive(Debug, Clone, PartialEq)]
pub struct IrParam {
    pub name: String,
    pub ty: CType,
    pub vreg: VReg,
}

/// A single function's IR.
#[derive(Debug, Clone, PartialEq)]
pub struct IrFunction {
    /// The function name (e.g. `"main"`).
    pub name: String,

    /// Formal parameters.
    pub params: Vec<IrParam>,

    /// Local variable descriptors: `(name, type, stack_offset)`.
    ///
    /// Only meaningful when `is_stack_mode` is `true`.
    pub locals: Vec<(String, CType, i32)>,

    /// The linear sequence of IR instructions comprising the function body.
    pub body: Vec<IrInstr>,

    /// Loop header labels that are explicitly marked with `#pragma unroll`.
    pub unroll_loop_headers: Vec<Label>,

    /// Return type.
    pub return_type: CType,

    /// When `true`, locals live on the stack and are accessed via
    /// `LoadLocal` / `StoreLocal`.  When `false`, all values live in virtual
    /// registers (register-promotion mode).
    pub is_stack_mode: bool,

    /// When `true`, this function accepts a variable number of arguments.
    pub is_variadic: bool,

    /// When `true`, the entire function body is a single `asm { }` block.
    /// The code generator skips the standard prologue/epilogue.
    pub is_asm_body: bool,
}

impl IrFunction {
    pub fn new(name: impl Into<String>, return_type: CType) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            locals: Vec::new(),
            body: Vec::new(),
            unroll_loop_headers: Vec::new(),
            return_type,
            is_stack_mode: false,
            is_variadic: false,
            is_asm_body: false,
        }
    }

    /// Append an instruction to the function body.
    pub fn push(&mut self, instr: IrInstr) {
        self.body.push(instr);
    }

    /// Append an [`IrOp`] with no source location.
    pub fn push_op(&mut self, op: IrOp) {
        self.body.push(IrInstr::bare(op));
    }

    /// Total byte size of all locals (for stack frame allocation).
    pub fn locals_size(&self) -> usize {
        self.locals
            .iter()
            .filter_map(|(_, ty, _)| ty.size_of())
            .sum()
    }
}

// ---------------------------------------------------------------------------
// GlobalVar — top-level variable
// ---------------------------------------------------------------------------

/// Describes a global variable in the IR.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalVar {
    pub name: String,
    pub ty: CType,
    /// Optional initial value (for initialized globals).  `None` means BSS.
    pub init: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// StringLiteral
// ---------------------------------------------------------------------------

/// An interned string literal with a compiler-generated label.
#[derive(Debug, Clone, PartialEq)]
pub struct StringLiteral {
    /// The label emitted in the assembly (e.g. `_S0`).
    pub label: String,
    /// The raw bytes of the string (including the NUL terminator).
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// IrProgram — the whole translation unit
// ---------------------------------------------------------------------------

/// The complete IR for one translation unit.
#[derive(Debug, Clone, PartialEq)]
pub struct IrProgram {
    /// Global variables.
    pub globals: Vec<GlobalVar>,
    /// Function definitions.
    pub functions: Vec<IrFunction>,
    /// String literal table (referenced by `AddrOfGlobal`).
    pub strings: Vec<StringLiteral>,
}

impl IrProgram {
    pub fn new() -> Self {
        Self {
            globals: Vec::new(),
            functions: Vec::new(),
            strings: Vec::new(),
        }
    }
}

impl Default for IrProgram {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VRegAllocator — helper for generating fresh virtual registers
// ---------------------------------------------------------------------------

/// A monotonically-incrementing allocator for [`VReg`] identifiers.
#[derive(Debug, Clone)]
pub struct VRegAllocator {
    next_id: u32,
}

impl VRegAllocator {
    pub fn new() -> Self {
        Self { next_id: 0 }
    }

    /// Allocate a fresh virtual register of the given width.
    pub fn alloc(&mut self, width: Width) -> VReg {
        let id = self.next_id;
        self.next_id += 1;
        VReg::new(id, width)
    }

    /// Allocate a fresh virtual register whose width is derived from a
    /// [`CType`].
    ///
    /// # Panics
    ///
    /// Panics if the type has no scalar width (e.g. `Void`, `Array`,
    /// `Function`).
    pub fn alloc_for_type(&mut self, ty: &CType) -> VReg {
        let width = Width::from_ctype(ty)
            .expect("cannot allocate vreg for non-scalar type");
        self.alloc(width)
    }
}

impl Default for VRegAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LabelAllocator — helper for generating fresh labels
// ---------------------------------------------------------------------------

/// A monotonically-incrementing allocator for [`Label`] identifiers.
#[derive(Debug, Clone)]
pub struct LabelAllocator {
    next_id: u32,
}

impl LabelAllocator {
    pub fn new() -> Self {
        Self { next_id: 0 }
    }

    /// Allocate a fresh label.
    pub fn alloc(&mut self) -> Label {
        let id = self.next_id;
        self.next_id += 1;
        Label::new(id)
    }
}

impl Default for LabelAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors on IrOp
// ---------------------------------------------------------------------------

/// Helpers for constructing common IR operations concisely.
impl IrOp {
    pub fn load_imm(dst: VReg, value: i64) -> Self {
        IrOp::LoadImm { dst, value }
    }

    pub fn load_global(dst: VReg, addr_label: impl Into<String>) -> Self {
        IrOp::LoadGlobal {
            dst,
            addr_label: addr_label.into(),
        }
    }

    pub fn store_global(addr_label: impl Into<String>, src: VReg) -> Self {
        IrOp::StoreGlobal {
            addr_label: addr_label.into(),
            src,
        }
    }

    pub fn load_local(dst: VReg, offset: i32) -> Self {
        IrOp::LoadLocal { dst, offset }
    }

    pub fn store_local(offset: i32, src: VReg) -> Self {
        IrOp::StoreLocal { offset, src }
    }

    pub fn load_ptr(dst: VReg, ptr: VReg) -> Self {
        IrOp::LoadPtr { dst, ptr }
    }

    pub fn store_ptr(ptr: VReg, src: VReg) -> Self {
        IrOp::StorePtr { ptr, src }
    }

    pub fn add(dst: VReg, lhs: VReg, rhs: VReg, width: Width) -> Self {
        IrOp::Add { dst, lhs, rhs, width }
    }

    pub fn sub(dst: VReg, lhs: VReg, rhs: VReg, width: Width) -> Self {
        IrOp::Sub { dst, lhs, rhs, width }
    }

    pub fn copy(dst: VReg, src: VReg) -> Self {
        IrOp::Copy { dst, src }
    }

    pub fn cast(dst: VReg, src: VReg, to_type: CType) -> Self {
        IrOp::Cast { dst, src, to_type }
    }

    pub fn jump(target: Label) -> Self {
        IrOp::Jump { target }
    }

    pub fn jump_if_true(cond: VReg, target: Label) -> Self {
        IrOp::JumpIfTrue { cond, target }
    }

    pub fn jump_if_false(cond: VReg, target: Label) -> Self {
        IrOp::JumpIfFalse { cond, target }
    }

    pub fn call(
        func_name: impl Into<String>,
        args: Vec<VReg>,
        dst: Option<VReg>,
    ) -> Self {
        IrOp::Call {
            func_name: func_name.into(),
            args,
            dst,
        }
    }

    pub fn ret(value: Option<VReg>) -> Self {
        IrOp::Return { value }
    }

    pub fn label(label: Label) -> Self {
        IrOp::Label { label }
    }

    pub fn addr_of_global(dst: VReg, name: impl Into<String>) -> Self {
        IrOp::AddrOfGlobal {
            dst,
            name: name.into(),
        }
    }

    pub fn ptr_add(dst: VReg, ptr: VReg, offset: VReg, element_size: u16) -> Self {
        IrOp::PtrAdd {
            dst,
            ptr,
            offset,
            element_size,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Width ------------------------------------------------------------

    #[test]
    fn width_bytes() {
        assert_eq!(Width::W8.bytes(), 1);
        assert_eq!(Width::W16.bytes(), 2);
        assert_eq!(Width::W32.bytes(), 4);
    }

    #[test]
    fn width_from_ctype() {
        assert_eq!(Width::from_ctype(&CType::char_signed()), Some(Width::W8));
        assert_eq!(Width::from_ctype(&CType::int_signed()), Some(Width::W16));
        assert_eq!(Width::from_ctype(&CType::long_unsigned()), Some(Width::W32));
        assert_eq!(
            Width::from_ctype(&CType::ptr(CType::char_signed())),
            Some(Width::W16)
        );
        assert_eq!(Width::from_ctype(&CType::Void), None);
    }

    // -- VReg / Label display --------------------------------------------

    #[test]
    fn vreg_display() {
        let r = VReg::new(42, Width::W16);
        assert_eq!(format!("{}", r), "t42.w16");
        assert_eq!(format!("{:?}", r), "t42.w16");
    }

    #[test]
    fn label_display() {
        let l = Label::new(7);
        assert_eq!(format!("{}", l), "L7");
    }

    // -- Allocators -------------------------------------------------------

    #[test]
    fn vreg_allocator_increments() {
        let mut alloc = VRegAllocator::new();
        let r0 = alloc.alloc(Width::W8);
        let r1 = alloc.alloc(Width::W16);
        let r2 = alloc.alloc(Width::W32);
        assert_eq!(r0.id, 0);
        assert_eq!(r1.id, 1);
        assert_eq!(r2.id, 2);
        assert_eq!(r0.width, Width::W8);
        assert_eq!(r1.width, Width::W16);
        assert_eq!(r2.width, Width::W32);
    }

    #[test]
    fn vreg_alloc_for_type() {
        let mut alloc = VRegAllocator::new();
        let r = alloc.alloc_for_type(&CType::int_signed());
        assert_eq!(r.width, Width::W16);
    }

    #[test]
    fn label_allocator_increments() {
        let mut alloc = LabelAllocator::new();
        let l0 = alloc.alloc();
        let l1 = alloc.alloc();
        assert_eq!(l0.0, 0);
        assert_eq!(l1.0, 1);
    }

    // -- IrInstr construction ---------------------------------------------

    #[test]
    fn instr_with_line() {
        let r = VReg::new(0, Width::W16);
        let instr = IrInstr::new(IrOp::load_imm(r, 42), 10);
        assert_eq!(instr.line, 10);
        assert_eq!(
            instr.op,
            IrOp::LoadImm {
                dst: r,
                value: 42
            }
        );
    }

    #[test]
    fn instr_bare() {
        let r = VReg::new(0, Width::W8);
        let instr = IrInstr::bare(IrOp::ret(None));
        assert_eq!(instr.line, 0);
        assert_eq!(instr.op, IrOp::Return { value: None });
        let _ = r; // suppress unused
    }

    // -- IrFunction -------------------------------------------------------

    #[test]
    fn function_push_op() {
        let mut func = IrFunction::new("main", CType::int_signed());
        let r = VReg::new(0, Width::W16);
        func.push_op(IrOp::load_imm(r, 0));
        func.push_op(IrOp::ret(Some(r)));
        assert_eq!(func.body.len(), 2);
    }

    #[test]
    fn function_locals_size() {
        let mut func = IrFunction::new("f", CType::Void);
        func.is_stack_mode = true;
        func.locals.push(("x".into(), CType::int_signed(), 0));
        func.locals.push(("y".into(), CType::long_signed(), 2));
        assert_eq!(func.locals_size(), 6); // 2 + 4
    }

    // -- IrProgram --------------------------------------------------------

    #[test]
    fn program_default_is_empty() {
        let prog = IrProgram::default();
        assert!(prog.globals.is_empty());
        assert!(prog.functions.is_empty());
        assert!(prog.strings.is_empty());
    }

    // -- Convenience constructors -----------------------------------------

    #[test]
    fn op_constructors() {
        let mut va = VRegAllocator::new();
        let r0 = va.alloc(Width::W16);
        let r1 = va.alloc(Width::W16);
        let r2 = va.alloc(Width::W16);
        let lbl = Label::new(0);

        // Spot-check a few constructors to make sure they produce the right
        // variant and don't panic.
        let _ = IrOp::load_imm(r0, 100);
        let _ = IrOp::load_global(r0, "_x");
        let _ = IrOp::store_global("_x", r0);
        let _ = IrOp::add(r2, r0, r1, Width::W16);
        let _ = IrOp::sub(r2, r0, r1, Width::W16);
        let _ = IrOp::copy(r1, r0);
        let _ = IrOp::cast(r1, r0, CType::long_signed());
        let _ = IrOp::jump(lbl);
        let _ = IrOp::jump_if_true(r0, lbl);
        let _ = IrOp::jump_if_false(r0, lbl);
        let _ = IrOp::call("puts", vec![r0], Some(r1));
        let _ = IrOp::ret(Some(r0));
        let _ = IrOp::label(lbl);
        let _ = IrOp::addr_of_global(r0, "_msg");
        let _ = IrOp::ptr_add(r2, r0, r1, 2);
    }

    // -- Binary op width --------------------------------------------------

    #[test]
    fn binary_ops_carry_width() {
        let r = VReg::new(0, Width::W32);
        let s = VReg::new(1, Width::W32);
        let d = VReg::new(2, Width::W32);
        let op = IrOp::Add {
            dst: d,
            lhs: r,
            rhs: s,
            width: Width::W32,
        };
        if let IrOp::Add { width, .. } = op {
            assert_eq!(width, Width::W32);
        } else {
            panic!("expected Add");
        }
    }
}
