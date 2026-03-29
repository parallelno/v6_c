//! Intel 8080 code generator.
//!
//! Translates the three-address-code IR into Intel 8080 assembly text.
//! Each IR function becomes a labelled block of 8080 instructions; globals
//! are emitted as `DB` and `.storage` directives in a data section.
//!
//! The module works hand-in-hand with [`crate::regalloc::RegAllocator`] which
//! tracks physical register assignments, and [`crate::callgraph::CallGraphAnalysis`]
//! which provides the static memory addresses for non-recursive functions.

use std::collections::{HashMap, HashSet};

use crate::callgraph::{CallGraphAnalysis, FunctionEffects};
use crate::ir::{IrFunction, IrInstr, IrOp, IrProgram, Label, VReg, Width};
use crate::regalloc::{Location, MoveOp, PhysReg, RegAllocator};
use crate::types::CType;

// ---------------------------------------------------------------------------
// CodeGenerator
// ---------------------------------------------------------------------------

/// Intel 8080 code generator.
///
/// Accumulates assembly lines in `output` while walking the IR.
pub struct CodeGenerator {
    /// Assembly lines produced so far.
    output: Vec<String>,
    /// Physical register allocator.
    regalloc: RegAllocator,
    /// Static allocation information from call-graph analysis.
    analysis: CallGraphAnalysis,
    /// Name of the function currently being generated (used for label
    /// construction).
    current_func: String,
    /// Monotonic counter for compiler-generated labels (comparison helpers,
    /// etc.).
    label_counter: u32,
    /// Whether the current function is a leaf (makes no calls).
    is_leaf_func: bool,
    /// For each vreg id, the index of its last use in the current function.
    /// Used to free registers as soon as their values become dead.
    last_use: HashMap<u32, usize>,
    /// Current instruction index within the function being generated.
    instr_index: usize,
    /// Last emitted source C line marker for current function.
    current_c_line: u32,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Generate Intel 8080 assembly for an entire [`IrProgram`].
///
/// Returns a `Vec<String>` where each element is one line of assembly.
pub fn generate(program: &IrProgram, analysis: &CallGraphAnalysis) -> Vec<String> {
    let mut cg = CodeGenerator {
        output: Vec::new(),
        regalloc: RegAllocator::new(),
        analysis: analysis.clone(),
        current_func: String::new(),
        label_counter: 0,
        is_leaf_func: false,
        last_use: HashMap::new(),
        instr_index: 0,
        current_c_line: 0,
    };

    cg.emit_comment("--- code section ---");

    for func in &program.functions {
        cg.gen_function(func);
    }

    cg.emit_comment("--- data section ---");
    cg.gen_data_section(program);

    compact_cfg_layout(cg.output)
}

fn compact_cfg_layout(lines: Vec<String>) -> Vec<String> {
    fn parse_inst(line: &str) -> Option<(String, String)> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.ends_with(':') {
            return None;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let opcode = parts.next()?.to_uppercase();
        let operands = parts.next().unwrap_or("").trim().to_string();
        Some((opcode, operands))
    }

    fn next_significant(lines: &[String], mut idx: usize) -> Option<usize> {
        while idx < lines.len() {
            let t = lines[idx].trim();
            if t.is_empty() || t.starts_with(';') {
                idx += 1;
                continue;
            }
            return Some(idx);
        }
        None
    }

    let mut out = lines;
    let mut labels: HashMap<String, usize> = HashMap::new();
    for (i, line) in out.iter().enumerate() {
        let t = line.trim();
        if t.ends_with(':') && !t.starts_with(';') {
            labels.insert(t.trim_end_matches(':').to_string(), i);
        }
    }

    let mut forward: HashMap<String, String> = HashMap::new();
    for (label, &idx) in &labels {
        if let Some(next_idx) = next_significant(&out, idx + 1) {
            if let Some((op, operands)) = parse_inst(&out[next_idx]) {
                if op == "JMP" {
                    forward.insert(label.clone(), operands);
                }
            }
        }
    }

    for _ in 0..16 {
        let snapshot: Vec<(String, String)> = forward.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut changed = false;
        for (src, dst) in snapshot {
            if let Some(next) = forward.get(&dst) {
                if next != &src {
                    forward.insert(src, next.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for line in &mut out {
        if let Some((op, operands)) = parse_inst(line) {
            let is_jump = matches!(
                op.as_str(),
                "JMP" | "JZ" | "JNZ" | "JC" | "JNC" | "JM" | "JP" | "JPE" | "JPO"
            );
            if is_jump {
                if let Some(final_target) = forward.get(operands.trim()) {
                    *line = format!("\t{} {}", op, final_target);
                }
            }
        }
    }

    let mut compact = Vec::with_capacity(out.len());
    let mut i = 0;
    while i < out.len() {
        let remove = if let Some((op, operands)) = parse_inst(&out[i]) {
            if op == "JMP" {
                if let Some(next_idx) = next_significant(&out, i + 1) {
                    let next_trim = out[next_idx].trim();
                    next_trim.ends_with(':') && next_trim.trim_end_matches(':') == operands.trim()
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !remove {
            compact.push(out[i].clone());
        }
        i += 1;
    }

    compact
}

// ---------------------------------------------------------------------------
// Internal helpers – emit primitives
// ---------------------------------------------------------------------------

impl CodeGenerator {
    /// Emit a raw assembly line (already formatted).
    fn emit(&mut self, line: impl Into<String>) {
        self.output.push(line.into());
    }

    /// Emit an instruction (indented with a tab).
    fn emit_inst(&mut self, inst: &str) {
        self.output.push(format!("\t{}", inst));
    }

    /// Emit a label (with colon suffix, no indent).
    fn emit_label(&mut self, label: &str) {
        self.output.push(format!("{}:", label));
    }

    /// Emit a comment line.
    fn emit_comment(&mut self, text: &str) {
        self.output.push(format!("; {}", text));
    }

    /// Allocate a fresh internal label name.
    fn fresh_label(&mut self) -> String {
        let n = self.label_counter;
        self.label_counter += 1;
        format!("__cg_{}", n)
    }

    /// Format an IR label for assembly output.
    ///
    /// Labels are suffixed with the current function name to avoid global
    /// symbol collisions when multiple functions define their own `L0`, etc.
    fn ir_label(&self, label: Label) -> String {
        format!("L{}__{}", label.0, self.current_func)
    }

    /// Get the memory label for a W32 vreg (spilling to memory if needed).
    /// Returns the label where the low 16 bits are stored; high 16 are at label+2.
    fn w32_mem_label(&mut self, vreg: VReg) -> String {
        // If already in memory, return that label.
        if let Some(Location::Memory(label)) = self.regalloc.get_location(vreg).cloned() {
            return label;
        }
        // If in a register, spill to get a memory label.
        // The spill only saves the low 16 bits; that's acceptable for
        // the current W32 model where we track low 16 in registers and
        // the full 32-bit value is at the global/spill label.
        let save_ops = self.regalloc.save_all();
        self.emit_moves(&save_ops);
        if let Some(Location::Memory(label)) = self.regalloc.get_location(vreg).cloned() {
            return label;
        }
        // Fallback: create a fresh spill label.
        let label = format!("__w32_{}", self.label_counter);
        self.label_counter += 1;
        label
    }

    /// Copy a W32 value from memory `src_label` to `__op1`.
    fn emit_w32_to_op1(&mut self, src_label: &str) {
        self.emit_inst(&format!("LHLD {}", src_label));
        self.emit_inst("SHLD __op1");
        self.emit_inst(&format!("LHLD {}+2", src_label));
        self.emit_inst("SHLD __op1+2");
    }

    /// Copy a W32 value from memory `src_label` to `__op2`.
    fn emit_w32_to_op2(&mut self, src_label: &str) {
        self.emit_inst(&format!("LHLD {}", src_label));
        self.emit_inst("SHLD __op2");
        self.emit_inst(&format!("LHLD {}+2", src_label));
        self.emit_inst("SHLD __op2+2");
    }

    /// Copy a W32 value from `__op1` to memory `dst_label`.
    fn emit_op1_to_w32(&mut self, dst_label: &str) {
        self.emit_inst("LHLD __op1");
        self.emit_inst(&format!("SHLD {}", dst_label));
        self.emit_inst("LHLD __op1+2");
        self.emit_inst(&format!("SHLD {}+2", dst_label));
    }

    fn call_effects(&self, func_name: &str) -> FunctionEffects {
        self.analysis.effects_for(func_name)
    }

    fn spill_live_before_call(&mut self, func_name: &str) {
        let effects = self.call_effects(func_name);
        for reg in [PhysReg::HL, PhysReg::DE, PhysReg::BC, PhysReg::A] {
            if !effects.clobbers.contains(&reg) {
                continue;
            }
            let Some(vreg_id) = self.regalloc.occupant(reg) else {
                continue;
            };
            let live_after_call = self
                .last_use
                .get(&vreg_id)
                .is_some_and(|&last_idx| last_idx > self.instr_index);
            if live_after_call {
                if let Some(op) = self.regalloc.spill(reg) {
                    self.emit_moves(&[op]);
                }
            }
        }
    }

    fn clobber_after_call(&mut self, func_name: &str) {
        let effects = self.call_effects(func_name);
        let regs: Vec<PhysReg> = effects.clobbers.iter().copied().collect();
        self.regalloc.clobber_regs(&regs);
    }

    fn emit_call_with_effects(&mut self, func_name: &str) {
        self.emit_inst(&format!("CALL {}", func_name));
        self.clobber_after_call(func_name);
    }

    fn arg_place_cost(&self, arg: VReg, target: PhysReg) -> u32 {
        match self.regalloc.get_location(arg) {
            Some(Location::Reg(r)) if *r == target => 0,
            Some(Location::Reg(_)) => 1,
            Some(Location::Memory(_)) => 2,
            Some(Location::RematImm(_)) => 1,
            None => 3,
        }
    }

    fn known_imm(&self, vreg: VReg) -> Option<i64> {
        self.regalloc.immediate_of(vreg)
    }

    fn load_stack_arg_for_push(&mut self, arg: VReg) {
        match arg.width {
            Width::W8 => {
                self.ensure_a(arg);
                self.emit_inst("MOV L,A");
                self.emit_inst("MVI H,0");
                self.emit_inst("PUSH H");
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(arg);
                self.emit_inst("PUSH H");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MoveOp emission
// ---------------------------------------------------------------------------

impl CodeGenerator {
    /// Emit assembly for a slice of [`MoveOp`]s returned by the register
    /// allocator.
    fn emit_moves(&mut self, ops: &[MoveOp]) {
        for op in ops {
            match op {
                MoveOp::Spill { src, label } => {
                    match src {
                        PhysReg::HL => self.emit_inst(&format!("SHLD {}", label)),
                        PhysReg::A => self.emit_inst(&format!("STA {}", label)),
                        PhysReg::DE => {
                            // DE has no direct store; use XCHG, SHLD, XCHG
                            self.emit_inst("XCHG");
                            self.emit_inst(&format!("SHLD {}", label));
                            self.emit_inst("XCHG");
                        }
                        PhysReg::BC => {
                            self.emit_inst("MOV H,B");
                            self.emit_inst("MOV L,C");
                            self.emit_inst(&format!("SHLD {}", label));
                        }
                    }
                }
                MoveOp::Reload { dst, label } => {
                    match dst {
                        PhysReg::HL => self.emit_inst(&format!("LHLD {}", label)),
                        PhysReg::A => self.emit_inst(&format!("LDA {}", label)),
                        PhysReg::DE => {
                            self.emit_inst(&format!("LHLD {}", label));
                            self.emit_inst("XCHG");
                        }
                        PhysReg::BC => {
                            self.emit_inst(&format!("LHLD {}", label));
                            self.emit_inst("MOV B,H");
                            self.emit_inst("MOV C,L");
                        }
                    }
                }
                MoveOp::LoadImm { dst, value, width } => {
                    let imm8 = (*value & 0xFF) as u8;
                    let imm16 = (*value & 0xFFFF) as u16;
                    match (dst, width) {
                        (PhysReg::A, _) => self.emit_inst(&format!("MVI A,{}", imm8)),
                        (PhysReg::HL, _) => self.emit_inst(&format!("LXI H,{}", imm16)),
                        (PhysReg::DE, Width::W8) => {
                            self.emit_inst(&format!("MVI E,{}", imm8));
                            self.emit_inst("MVI D,0");
                        }
                        (PhysReg::BC, Width::W8) => {
                            self.emit_inst(&format!("MVI C,{}", imm8));
                            self.emit_inst("MVI B,0");
                        }
                        (PhysReg::DE, _) => self.emit_inst(&format!("LXI D,{}", imm16)),
                        (PhysReg::BC, _) => self.emit_inst(&format!("LXI B,{}", imm16)),
                    }
                }
                MoveOp::RegToReg { src, dst } => {
                    self.emit_reg_to_reg(*src, *dst);
                }
            }
        }
    }

    /// Emit a register-to-register move.
    fn emit_reg_to_reg(&mut self, src: PhysReg, dst: PhysReg) {
        if src == dst {
            return;
        }
        match (src, dst) {
            (PhysReg::HL, PhysReg::DE) | (PhysReg::DE, PhysReg::HL) => {
                self.emit_inst("XCHG");
            }
            (PhysReg::HL, PhysReg::BC) => {
                self.emit_inst("MOV B,H");
                self.emit_inst("MOV C,L");
            }
            (PhysReg::BC, PhysReg::HL) => {
                self.emit_inst("MOV H,B");
                self.emit_inst("MOV L,C");
            }
            (PhysReg::DE, PhysReg::BC) => {
                self.emit_inst("MOV B,D");
                self.emit_inst("MOV C,E");
            }
            (PhysReg::BC, PhysReg::DE) => {
                self.emit_inst("MOV D,B");
                self.emit_inst("MOV E,C");
            }
            (PhysReg::A, PhysReg::HL) => {
                self.emit_inst("MOV L,A");
                self.emit_inst("MVI H,0");
            }
            (PhysReg::HL, PhysReg::A) => {
                self.emit_inst("MOV A,L");
            }
            (PhysReg::A, PhysReg::DE) => {
                self.emit_inst("MOV E,A");
                self.emit_inst("MVI D,0");
            }
            (PhysReg::DE, PhysReg::A) => {
                self.emit_inst("MOV A,E");
            }
            (PhysReg::A, PhysReg::BC) => {
                self.emit_inst("MOV C,A");
                self.emit_inst("MVI B,0");
            }
            (PhysReg::BC, PhysReg::A) => {
                self.emit_inst("MOV A,C");
            }
            // Same register — already handled by early return above.
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Ensure helpers – place a vreg into a specific physical register
// ---------------------------------------------------------------------------

impl CodeGenerator {
    /// Ensure `vreg` is in `target` register, emitting any necessary moves.
    fn ensure(&mut self, vreg: VReg, target: PhysReg) {
        let ops = self.regalloc.ensure_in_reg(vreg, target);
        self.emit_moves(&ops);
    }

    /// Ensure `vreg` is in HL.
    fn ensure_hl(&mut self, vreg: VReg) {
        self.ensure(vreg, PhysReg::HL);
    }

    /// Ensure `vreg` is in DE.
    fn ensure_de(&mut self, vreg: VReg) {
        self.ensure(vreg, PhysReg::DE);
    }

    /// Ensure `vreg` is in A.
    fn ensure_a(&mut self, vreg: VReg) {
        self.ensure(vreg, PhysReg::A);
    }

    /// Mark `vreg` as living in `reg` after we've emitted a load ourselves.
    fn mark(&mut self, vreg: VReg, reg: PhysReg) {
        let ops = self.regalloc.mark_allocated(vreg, reg);
        self.emit_moves(&ops);
    }
}

// ---------------------------------------------------------------------------
// Function generation
// ---------------------------------------------------------------------------

impl CodeGenerator {
    fn gen_function(&mut self, func: &IrFunction) {
        self.current_func = func.name.clone();
        self.is_leaf_func = self.analysis.is_leaf(&func.name);
        self.regalloc.reset();
        self.last_use = compute_last_use(&func.body);
        self.current_c_line = 0;
        self.emit_comment(&format!("function {}", func.name));
        self.emit_label(&func.name);

        // For variadic functions, capture the address of the first variadic
        // arg on the stack. At entry: SP → [ret_addr], stack args start at SP+2.
        if func.is_variadic {
            self.emit_inst("LXI H,2");
            self.emit_inst("DAD SP");
            self.emit_inst(&format!("SHLD __va_base_{}", func.name));
        }

        for (idx, instr) in func.body.iter().enumerate() {
            self.instr_index = idx;
            if instr.line != 0 && instr.line != self.current_c_line {
                self.emit_comment(&format!("C_LINE {}", instr.line));
                self.current_c_line = instr.line;
            }
            self.gen_op(&instr.op);
            // Free registers holding vregs that are dead after this instruction.
            self.free_dead_vregs(&instr.op);
        }

        // Safety net: if the function body doesn't end with a Return, emit one.
        if !func
            .body
            .last()
            .is_some_and(|i| matches!(i.op, IrOp::Return { .. }))
        {
            self.emit_inst("RET");
        }
    }

    /// Free any source-operand vregs whose last use is the current instruction.
    /// This makes registers available sooner, reducing unnecessary spills.
    fn free_dead_vregs(&mut self, op: &IrOp) {
        let src_ids = collect_op_src_ids(op);
        for vreg_id in src_ids {
            if let Some(&last_idx) = self.last_use.get(&vreg_id) {
                if last_idx == self.instr_index {
                    // This vreg is dead after this instruction — free it.
                    // We use a dummy width (W16) since `free` only cares about the id.
                    self.regalloc.free(VReg::new(vreg_id, Width::W16));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-instruction code generation
// ---------------------------------------------------------------------------

impl CodeGenerator {
    fn gen_op(&mut self, op: &IrOp) {
        match op {
            // -- loads / stores -------------------------------------------
            IrOp::LoadImm { dst, value } => self.gen_load_imm(*dst, *value),
            IrOp::LoadGlobal { dst, addr_label } => {
                self.gen_load_global(*dst, addr_label);
            }
            IrOp::StoreGlobal { addr_label, src } => {
                self.gen_store_global(addr_label, *src);
            }
            IrOp::LoadLocal { dst, offset } => self.gen_load_local(*dst, *offset),
            IrOp::StoreLocal { offset, src } => self.gen_store_local(*offset, *src),
            IrOp::LoadPtr { dst, ptr } => self.gen_load_ptr(*dst, *ptr),
            IrOp::StorePtr { ptr, src } => self.gen_store_ptr(*ptr, *src),

            // -- binary arithmetic ----------------------------------------
            IrOp::Add { dst, lhs, rhs, width } => {
                self.gen_add(*dst, *lhs, *rhs, *width);
            }
            IrOp::Sub { dst, lhs, rhs, width } => {
                self.gen_sub(*dst, *lhs, *rhs, *width);
            }
            IrOp::Mul { dst, lhs, rhs, width, signed } => {
                self.gen_mul(*dst, *lhs, *rhs, *width, *signed);
            }
            IrOp::Div { dst, lhs, rhs, width, signed } => {
                self.gen_div(*dst, *lhs, *rhs, *width, *signed);
            }
            IrOp::Mod { dst, lhs, rhs, width, signed } => {
                self.gen_mod(*dst, *lhs, *rhs, *width, *signed);
            }

            // -- bitwise --------------------------------------------------
            IrOp::And { dst, lhs, rhs, width } => {
                self.gen_bitwise(*dst, *lhs, *rhs, *width, "ANA", "ANI");
            }
            IrOp::Or { dst, lhs, rhs, width } => {
                self.gen_bitwise(*dst, *lhs, *rhs, *width, "ORA", "ORI");
            }
            IrOp::Xor { dst, lhs, rhs, width } => {
                self.gen_bitwise(*dst, *lhs, *rhs, *width, "XRA", "XRI");
            }

            // -- shifts ---------------------------------------------------
            IrOp::Shl { dst, lhs, rhs, width } => {
                self.gen_shift(*dst, *lhs, *rhs, *width, false, false);
            }
            IrOp::Shr { dst, lhs, rhs, width, arithmetic } => {
                self.gen_shift(*dst, *lhs, *rhs, *width, true, *arithmetic);
            }

            // -- comparisons ----------------------------------------------
            IrOp::Eq { dst, lhs, rhs, width } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "eq", false);
            }
            IrOp::Ne { dst, lhs, rhs, width } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "ne", false);
            }
            IrOp::Lt { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "lt", *signed);
            }
            IrOp::Le { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "le", *signed);
            }
            IrOp::Gt { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "gt", *signed);
            }
            IrOp::Ge { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "ge", *signed);
            }

            // -- unary ----------------------------------------------------
            IrOp::Neg { dst, src, width } => self.gen_neg(*dst, *src, *width),
            IrOp::Not { dst, src, width } => self.gen_not(*dst, *src, *width),
            IrOp::LogicalNot { dst, src, width } => {
                self.gen_logical_not(*dst, *src, *width);
            }

            // -- moves / conversions --------------------------------------
            IrOp::Copy { dst, src } => self.gen_copy(*dst, *src),
            IrOp::Cast { dst, src, to_type } => self.gen_cast(*dst, *src, to_type),

            // -- control flow ---------------------------------------------
            IrOp::Jump { target } => self.gen_jump(*target),
            IrOp::JumpIfTrue { cond, target } => {
                self.gen_jump_if_true(*cond, *target);
            }
            IrOp::JumpIfFalse { cond, target } => {
                self.gen_jump_if_false(*cond, *target);
            }
            IrOp::Call { func_name, args, dst } => {
                self.gen_call(func_name, args, *dst);
            }
            IrOp::Return { value } => self.gen_return(*value),
            IrOp::Label { label } => {
                self.emit_label(&self.ir_label(*label));
            }

            // -- address-of -----------------------------------------------
            IrOp::AddrOfGlobal { dst, name } => self.gen_addr_of_global(*dst, name),

            // -- pointer arithmetic ---------------------------------------
            IrOp::PtrAdd { dst, ptr, offset, element_size } => {
                self.gen_ptr_add(*dst, *ptr, *offset, *element_size);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Individual instruction generators
// ---------------------------------------------------------------------------

impl CodeGenerator {
    // -- LoadImm ----------------------------------------------------------

    fn gen_load_imm(&mut self, dst: VReg, value: i64) {
        match dst.width {
            Width::W8 => {
                let v = (value & 0xFF) as u8;
                // Obtain any eviction ops BEFORE emitting the load so that
                // the old A value is saved (SHLD/STA) prior to the overwrite.
                let ops = self.regalloc.mark_immediate(dst, PhysReg::A, value);
                self.emit_moves(&ops);
                self.emit_inst(&format!("MVI A,{}", v));
            }
            Width::W16 => {
                let v = (value & 0xFFFF) as u16;
                // When HL is already occupied by a live value, prefer DE (then
                // BC) to avoid evicting (spilling) HL just to load an immediate.
                // This typically saves a SHLD + LHLD pair for patterns like:
                //   addr = 32768; addr += 4;  →  LXI D,4 / DAD D
                // instead of:
                //   SHLD __spill / LXI H,4 / XCHG / LHLD __spill / DAD D
                let target = if !self.regalloc.is_free(PhysReg::HL) {
                    if self.regalloc.is_free(PhysReg::DE) {
                        PhysReg::DE
                    } else if self.regalloc.is_free(PhysReg::BC) {
                        PhysReg::BC
                    } else {
                        PhysReg::HL
                    }
                } else {
                    PhysReg::HL
                };
                let ops = self.regalloc.mark_immediate(dst, target, value);
                self.emit_moves(&ops);
                let lxi = match target {
                    PhysReg::DE => format!("LXI D,{}", v),
                    PhysReg::BC => format!("LXI B,{}", v),
                    _ => format!("LXI H,{}", v),
                };
                self.emit_inst(&lxi);
            }
            Width::W32 => {
                let lo = (value & 0xFFFF) as u16;
                let hi = ((value >> 16) & 0xFFFF) as u16;
                self.emit_inst(&format!("LXI H,{}", lo));
                self.mark(dst, PhysReg::HL);
                // Spill immediately so the full value is in memory.
                let save_ops = self.regalloc.save_all();
                self.emit_moves(&save_ops);
                // Store high 16 bits next to the low 16.
                if let Some(Location::Memory(label)) = self.regalloc.get_location(dst).cloned() {
                    self.emit_inst(&format!("LXI H,{}", hi));
                    self.emit_inst(&format!("SHLD {}+2", label));
                }
                // Reload low 16 into HL for downstream use.
                self.emit_inst(&format!("LXI H,{}", lo));
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- LoadGlobal / StoreGlobal -----------------------------------------

    fn gen_load_global(&mut self, dst: VReg, addr_label: &str) {
        match dst.width {
            Width::W8 => {
                self.emit_inst(&format!("LDA {}", addr_label));
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                self.emit_inst(&format!("LHLD {}", addr_label));
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    fn gen_store_global(&mut self, addr_label: &str, src: VReg) {
        match src.width {
            Width::W8 => {
                self.ensure_a(src);
                self.emit_inst(&format!("STA {}", addr_label));
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(src);
                self.emit_inst(&format!("SHLD {}", addr_label));
            }
        }
    }

    // -- LoadLocal / StoreLocal -------------------------------------------
    // In global mode these become loads/stores to the static address
    // allocated by the call-graph analysis.

    fn gen_load_local(&mut self, dst: VReg, offset: i32) {
        // In global mode the offset is really just an index; the actual
        // address is looked up from the analysis.  For simplicity we
        // fall back to a label-based load using a helper address.
        let label = format!("__local_{}_{}", self.current_func, offset);
        self.gen_load_global(dst, &label);
    }

    fn gen_store_local(&mut self, offset: i32, src: VReg) {
        let label = format!("__local_{}_{}", self.current_func, offset);
        self.gen_store_global(&label, src);
    }

    // -- LoadPtr / StorePtr -----------------------------------------------

    fn gen_load_ptr(&mut self, dst: VReg, ptr: VReg) {
        match dst.width {
            Width::W8 => {
                self.ensure_hl(ptr);
                self.emit_inst("MOV A,M");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                // Fast path: constant address → LHLD addr (1 instruction).
                if let Some(addr) = self.known_imm(ptr) {
                    let addr16 = (addr & 0xFFFF) as u16;
                    self.regalloc.free(ptr);
                    self.emit_inst(&format!("LHLD {}", addr16));
                    self.mark(dst, PhysReg::HL);
                    return;
                }
                // General path: load 16-bit value from (HL): low byte first.
                self.ensure_hl(ptr);
                self.emit_inst("MOV E,M");
                self.emit_inst("INX H");
                self.emit_inst("MOV D,M");
                self.emit_inst("XCHG");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    fn gen_store_ptr(&mut self, ptr: VReg, src: VReg) {
        match src.width {
            Width::W8 => {
                self.ensure_a(src);
                self.ensure_hl(ptr);
                self.emit_inst("MOV M,A");
            }
            Width::W16 | Width::W32 => {
                // Fast path: constant address → SHLD addr (cheaper than MOV M,E / INX H / MOV M,D).
                if let Some(addr) = self.known_imm(ptr) {
                    let addr16 = (addr & 0xFFFF) as u16;
                    self.regalloc.free(ptr);
                    self.ensure_hl(src);
                    self.emit_inst(&format!("SHLD {}", addr16));
                    return;
                }
                self.ensure_de(src);
                self.ensure_hl(ptr);
                self.emit_inst("MOV M,E");
                self.emit_inst("INX H");
                self.emit_inst("MOV M,D");
            }
        }
    }

    // -- Add --------------------------------------------------------------

    fn gen_add(&mut self, dst: VReg, lhs: VReg, rhs: VReg, width: Width) {
        match width {
            Width::W8 => {
                self.ensure_a(lhs);
                let rhs_loc = self.regalloc.get_location(rhs).cloned();
                match rhs_loc {
                    Some(Location::Reg(PhysReg::A)) => {
                        self.emit_inst("ADD A");
                    }
                    Some(Location::Reg(r)) => {
                        let reg_name = low_byte_name(r);
                        self.emit_inst(&format!("ADD {}", reg_name));
                    }
                    _ => {
                        self.ensure_a(lhs);
                        // rhs must be reloaded; use B as temp
                        self.ensure(rhs, PhysReg::BC);
                        self.ensure_a(lhs);
                        self.emit_inst("ADD C");
                    }
                }
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                // Fast path: Add(x, ±k) or Add(±k, x) for small k.
                // Using INX H / DCX H avoids loading k into DE, which would
                // otherwise force a spill/reload of lhs (x).
                let rhs_k = self.known_imm(rhs);
                let lhs_k = self.known_imm(lhs);
                let fast: Option<(VReg, VReg, i64)> = match (rhs_k, lhs_k) {
                    (Some(k), _)
                        if k != 0
                            && k.abs() <= 3
                            && self.last_use.get(&rhs.id).copied()
                                == Some(self.instr_index) =>
                    {
                        Some((lhs, rhs, k)) // var_op, const_op, k
                    }
                    (_, Some(k))
                        if k != 0
                            && k.abs() <= 3
                            && self.last_use.get(&lhs.id).copied()
                                == Some(self.instr_index) =>
                    {
                        Some((rhs, lhs, k)) // add is commutative
                    }
                    _ => None,
                };
                if let Some((var_op, const_op, k)) = fast {
                    // Release the constant register before loading var_op so
                    // that HL is available without a detour through spill/reload.
                    self.regalloc.free(const_op);
                    self.ensure_hl(var_op);
                    // Release var_op here too if it dies; prevents a dead spill
                    // when mark(dst, HL) later evicts the occupant of HL.
                    if self.last_use.get(&var_op.id).copied() == Some(self.instr_index) {
                        self.regalloc.free(var_op);
                    }
                    let inst = if k > 0 { "INX H" } else { "DCX H" };
                    for _ in 0..k.abs() {
                        self.emit_inst(inst);
                    }
                    self.mark(dst, PhysReg::HL);
                } else {
                    // General case: DAD D.
                    // If one operand is a known immediate, emit LXI D,N / DAD D
                    // (or LXI B,N / DAD B) directly instead of going through
                    // ensure_de, which would spill HL if the immediate is there.
                    let rhs_k = self.known_imm(rhs);
                    let lhs_k = self.known_imm(lhs);
                    let imm_info: Option<(VReg, VReg, i64)> = match (rhs_k, lhs_k) {
                        (Some(k), _) => Some((lhs, rhs, k)),
                        (_, Some(k)) => Some((rhs, lhs, k)),
                        _ => None,
                    };
                    if let Some((var_op, const_op, k)) = imm_info {
                        let k16 = (k & 0xFFFF) as u16;
                        // Check whether gen_load_imm already placed the
                        // constant in DE or BC (preferred-register logic).
                        let const_in_de =
                            self.regalloc.occupant(PhysReg::DE) == Some(const_op.id);
                        let const_in_bc =
                            self.regalloc.occupant(PhysReg::BC) == Some(const_op.id);
                        // Free the constant register to make HL available.
                        self.regalloc.free(const_op);
                        self.ensure_hl(var_op);
                        if self.last_use.get(&var_op.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(var_op);
                        }
                        if const_in_de {
                            // Already in DE — LXI D,k already emitted by gen_load_imm.
                            self.emit_inst("DAD D");
                        } else if const_in_bc {
                            self.emit_inst("DAD B");
                        } else if self.regalloc.is_free(PhysReg::DE) {
                            self.emit_inst(&format!("LXI D,{}", k16));
                            self.emit_inst("DAD D");
                        } else {
                            self.emit_inst(&format!("LXI B,{}", k16));
                            self.emit_inst("DAD B");
                        }
                    } else {
                        self.ensure_de(rhs);
                        self.ensure_hl(lhs);
                        self.emit_inst("DAD D");
                    }
                    self.mark(dst, PhysReg::HL);
                }
            }
        }
    }

    // -- Sub --------------------------------------------------------------

    fn gen_sub(&mut self, dst: VReg, lhs: VReg, rhs: VReg, width: Width) {
        match width {
            Width::W8 => {
                self.ensure(rhs, PhysReg::BC);
                self.ensure_a(lhs);
                self.emit_inst("SUB C");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                // Fast path: Sub(x, ±k) for small k — use DCX H / INX H.
                if let Some(k) = self.known_imm(rhs) {
                    if k != 0
                        && k.abs() <= 3
                        && self.last_use.get(&rhs.id).copied() == Some(self.instr_index)
                    {
                        self.regalloc.free(rhs);
                        self.ensure_hl(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
                        // lhs - k: DCX H repeated k times (or INX H if k < 0)
                        let inst = if k > 0 { "DCX H" } else { "INX H" };
                        for _ in 0..k.abs() {
                            self.emit_inst(inst);
                        }
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                }
                // General sub with immediate rhs: emit LXI D,(-k) / DAD D
                // (or LXI B,(-k) / DAD B) to avoid spilling HL.
                if let Some(k) = self.known_imm(rhs) {
                    if self.last_use.get(&rhs.id).copied() == Some(self.instr_index) {
                        let neg = (k.wrapping_neg() & 0xFFFF) as u16;
                        self.regalloc.free(rhs);
                        self.ensure_hl(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
                        if self.regalloc.is_free(PhysReg::DE) {
                            self.emit_inst(&format!("LXI D,{}", neg));
                            self.emit_inst("DAD D");
                        } else {
                            self.emit_inst(&format!("LXI B,{}", neg));
                            self.emit_inst("DAD B");
                        }
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                }
                // General case: HL = HL - DE  → complement DE, add, increment
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                // negate DE: complement and increment
                self.emit_inst("MOV A,D");
                self.emit_inst("CMA");
                self.emit_inst("MOV D,A");
                self.emit_inst("MOV A,E");
                self.emit_inst("CMA");
                self.emit_inst("MOV E,A");
                self.emit_inst("INX D");
                self.emit_inst("DAD D");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- Mul / Div / Mod (via runtime calls) ------------------------------

    fn gen_mul(&mut self, dst: VReg, lhs: VReg, rhs: VReg, width: Width, _signed: bool) {
        match width {
            Width::W8 => {
                if let Some(k) = self.known_imm(rhs).or_else(|| self.known_imm(lhs)) {
                    let var = if self.known_imm(rhs).is_some() { lhs } else { rhs };
                    match k {
                        0 => {
                            self.emit_inst("MVI A,0");
                            self.mark(dst, PhysReg::A);
                            return;
                        }
                        1 => {
                            self.ensure_a(var);
                            self.mark(dst, PhysReg::A);
                            return;
                        }
                        2 => {
                            self.ensure_a(var);
                            self.emit_inst("ADD A");
                            self.mark(dst, PhysReg::A);
                            return;
                        }
                        _ => {}
                    }
                }
                // Widen to 16-bit and use runtime
                self.spill_live_before_call("__mul16");
                self.ensure_hl(lhs);
                self.ensure_de(rhs);
                self.emit_call_with_effects("__mul16");
                self.mark(dst, PhysReg::HL);
            }
            Width::W16 => {
                if let Some(k) = self.known_imm(rhs).or_else(|| self.known_imm(lhs)) {
                    let var = if self.known_imm(rhs).is_some() { lhs } else { rhs };
                    match k {
                        0 => {
                            self.emit_inst("LXI H,0");
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        1 => {
                            self.ensure_hl(var);
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        2 | 4 | 8 => {
                            self.ensure_hl(var);
                            let shifts = match k {
                                2 => 1,
                                4 => 2,
                                _ => 3,
                            };
                            for _ in 0..shifts {
                                self.emit_inst("DAD H");
                            }
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        3 => {
                            self.ensure_hl(var);
                            self.emit_inst("MOV D,H");
                            self.emit_inst("MOV E,L");
                            self.emit_inst("DAD H");
                            self.emit_inst("DAD D");
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        _ => {}
                    }
                }
                self.spill_live_before_call("__mul16");
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                self.emit_call_with_effects("__mul16");
                self.mark(dst, PhysReg::HL);
            }
            Width::W32 => {
                self.spill_live_before_call("__mul32");
                let lhs_label = self.w32_mem_label(lhs);
                let rhs_label = self.w32_mem_label(rhs);
                self.emit_w32_to_op1(&lhs_label);
                self.emit_w32_to_op2(&rhs_label);
                self.emit_call_with_effects("__mul32");
                // Result is in __op1; copy to dst's location.
                let save_ops = self.regalloc.save_all();
                self.emit_moves(&save_ops);
                self.emit_inst("LHLD __op1");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    fn gen_div(&mut self, dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool) {
        match width {
            Width::W8 => {
                if !signed && self.known_imm(rhs) == Some(1) {
                    self.ensure_a(lhs);
                    self.mark(dst, PhysReg::A);
                    return;
                }
                let helper = if signed { "__div16s" } else { "__div16u" };
                self.spill_live_before_call(helper);
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                self.emit_call_with_effects(helper);
                self.emit_inst("MOV A,L");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 => {
                if !signed && self.known_imm(rhs) == Some(1) {
                    self.ensure_hl(lhs);
                    self.mark(dst, PhysReg::HL);
                    return;
                }
                let helper = if signed { "__div16s" } else { "__div16u" };
                self.spill_live_before_call(helper);
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                self.emit_call_with_effects(helper);
                self.mark(dst, PhysReg::HL);
            }
            Width::W32 => {
                let helper = if signed { "__div32s" } else { "__div32u" };
                self.spill_live_before_call(helper);
                let lhs_label = self.w32_mem_label(lhs);
                let rhs_label = self.w32_mem_label(rhs);
                self.emit_w32_to_op1(&lhs_label);
                self.emit_w32_to_op2(&rhs_label);
                self.emit_call_with_effects(helper);
                let save_ops = self.regalloc.save_all();
                self.emit_moves(&save_ops);
                self.emit_inst("LHLD __op1");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    fn gen_mod(&mut self, dst: VReg, lhs: VReg, rhs: VReg, width: Width, signed: bool) {
        match width {
            Width::W8 => {
                if !signed {
                    if self.known_imm(rhs) == Some(1) {
                        self.emit_inst("MVI A,0");
                        self.mark(dst, PhysReg::A);
                        return;
                    }
                    if self.known_imm(rhs) == Some(2) {
                        self.ensure_a(lhs);
                        self.emit_inst("ANI 1");
                        self.mark(dst, PhysReg::A);
                        return;
                    }
                }
                let helper = if signed { "__mod16s" } else { "__mod16u" };
                self.spill_live_before_call(helper);
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                self.emit_call_with_effects(helper);
                self.emit_inst("MOV A,L");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 => {
                if !signed {
                    if self.known_imm(rhs) == Some(1) {
                        self.emit_inst("LXI H,0");
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                    if self.known_imm(rhs) == Some(2) {
                        self.ensure_hl(lhs);
                        self.emit_inst("MOV A,L");
                        self.emit_inst("ANI 1");
                        self.emit_inst("MOV L,A");
                        self.emit_inst("MVI H,0");
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                }
                let helper = if signed { "__mod16s" } else { "__mod16u" };
                self.spill_live_before_call(helper);
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                self.emit_call_with_effects(helper);
                self.mark(dst, PhysReg::HL);
            }
            Width::W32 => {
                let helper = if signed { "__mod32s" } else { "__mod32u" };
                self.spill_live_before_call(helper);
                let lhs_label = self.w32_mem_label(lhs);
                let rhs_label = self.w32_mem_label(rhs);
                self.emit_w32_to_op1(&lhs_label);
                self.emit_w32_to_op2(&rhs_label);
                self.emit_call_with_effects(helper);
                let save_ops = self.regalloc.save_all();
                self.emit_moves(&save_ops);
                self.emit_inst("LHLD __op1");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- Bitwise (And / Or / Xor) -----------------------------------------

    fn gen_bitwise(
        &mut self,
        dst: VReg,
        lhs: VReg,
        rhs: VReg,
        width: Width,
        reg_op: &str,
        _imm_op: &str,
    ) {
        match width {
            Width::W8 => {
                self.ensure(rhs, PhysReg::BC);
                self.ensure_a(lhs);
                self.emit_inst(&format!("{} C", reg_op));
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                // Byte-by-byte: low bytes then high bytes
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                // low byte
                self.emit_inst("MOV A,L");
                self.emit_inst(&format!("{} E", reg_op));
                self.emit_inst("MOV L,A");
                // high byte
                self.emit_inst("MOV A,H");
                self.emit_inst(&format!("{} D", reg_op));
                self.emit_inst("MOV H,A");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- Shifts -----------------------------------------------------------

    fn gen_shift(
        &mut self,
        dst: VReg,
        lhs: VReg,
        rhs: VReg,
        width: Width,
        is_right: bool,
        arithmetic: bool,
    ) {
        match width {
            Width::W8 => {
                // Simple: use rotate instructions in a loop
                self.ensure(rhs, PhysReg::BC);
                self.ensure_a(lhs);
                let loop_lbl = self.fresh_label();
                let done_lbl = self.fresh_label();
                self.emit_inst("MOV B,C"); // shift count in B
                self.emit_label(&loop_lbl);
                self.emit_inst("MOV A,B");
                self.emit_inst("ORA A");
                self.emit_inst(&format!("JZ {}", done_lbl));
                self.ensure_a(lhs);
                if is_right {
                    self.emit_inst("RAR");
                } else {
                    self.emit_inst("RAL");
                }
                self.emit_inst("DCR B");
                self.emit_inst(&format!("JMP {}", loop_lbl));
                self.emit_label(&done_lbl);
                self.mark(dst, PhysReg::A);
            }
            Width::W16 => {
                if let Some(raw) = self.known_imm(rhs) {
                    let count = (raw & 0x1f) as u32;
                    if count == 0 {
                        self.ensure_hl(lhs);
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                    if !is_right && count <= 3 {
                        self.ensure_hl(lhs);
                        for _ in 0..count {
                            self.emit_inst("DAD H");
                        }
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                    if is_right && !arithmetic && count == 1 {
                        self.ensure_hl(lhs);
                        self.emit_inst("MOV A,H");
                        self.emit_inst("ORA A");
                        self.emit_inst("RAR");
                        self.emit_inst("MOV H,A");
                        self.emit_inst("MOV A,L");
                        self.emit_inst("RAR");
                        self.emit_inst("MOV L,A");
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                }
                // Use runtime helpers: shift count in B, value in HL
                let helper = if is_right {
                    if arithmetic {
                        "__shr16s"
                    } else {
                        "__shr16u"
                    }
                } else {
                    "__shl16"
                };
                self.spill_live_before_call(helper);
                self.ensure(rhs, PhysReg::BC);
                self.ensure_hl(lhs);
                self.emit_inst("MOV B,C"); // count from C→B
                self.emit_call_with_effects(helper);
                self.mark(dst, PhysReg::HL);
            }
            Width::W32 => {
                // Use 32-bit runtime helpers: shift count in B, value in __op1.
                // The shift count is a small integer (0..31); only the low byte
                // is meaningful even though the vreg may be W32.
                let helper = if is_right {
                    if arithmetic {
                        "__shr32s"
                    } else {
                        "__shr32u"
                    }
                } else {
                    "__shl32"
                };
                self.spill_live_before_call(helper);
                self.ensure(rhs, PhysReg::BC);
                let shift_count_label = self.w32_mem_label(rhs);
                let lhs_label = self.w32_mem_label(lhs);
                self.emit_w32_to_op1(&lhs_label);
                // Reload shift count (low byte only) into B
                self.emit_inst(&format!("LDA {}", shift_count_label));
                self.emit_inst("MOV B,A");
                self.emit_call_with_effects(helper);
                let save_ops = self.regalloc.save_all();
                self.emit_moves(&save_ops);
                self.emit_inst("LHLD __op1");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- Comparisons ------------------------------------------------------

    fn gen_compare(
        &mut self,
        dst: VReg,
        lhs: VReg,
        rhs: VReg,
        width: Width,
        kind: &str,
        signed: bool,
    ) {
        let true_lbl = self.fresh_label();
        let done_lbl = self.fresh_label();

        match width {
            Width::W8 => {
                self.ensure(rhs, PhysReg::BC);
                self.ensure_a(lhs);
                self.emit_inst("CMP C");
            }
            Width::W16 | Width::W32 => {
                // Subtract: HL - DE, check flags
                self.ensure_de(rhs);
                self.ensure_hl(lhs);
                // For equality/inequality: XOR compare
                if kind == "eq" || kind == "ne" {
                    self.emit_inst("MOV A,L");
                    self.emit_inst("SUB E");
                    self.emit_inst(&format!("JNZ {}", if kind == "eq" { &done_lbl } else { &true_lbl }));
                    self.emit_inst("MOV A,H");
                    self.emit_inst("SUB D");
                    // falls through to flag-based branch below
                } else {
                    // For ordering: use signed/unsigned subtraction
                    // A = H - D first (high byte comparison)
                    if signed {
                        // Signed comparison via runtime or sign-flag trick
                        self.emit_inst("MOV A,H");
                        self.emit_inst("SUB D");
                    } else {
                        self.emit_inst("MOV A,H");
                        self.emit_inst("SUB D");
                    }
                    self.emit_inst(&format!("JNZ __cmp_done_{}", true_lbl));
                    self.emit_inst("MOV A,L");
                    self.emit_inst("SUB E");
                    self.emit_label(&format!("__cmp_done_{}", true_lbl));
                }
            }
        }

        // Now branch based on comparison kind
        let branch = match kind {
            "eq" => "JZ",
            "ne" => "JNZ",
            "lt" => if signed { "JM" } else { "JC" },
            "ge" => if signed { "JP" } else { "JNC" },
            "gt" => {
                // gt: not zero AND not (less/carry)
                // Handle as !(le): we'll use a two-branch sequence
                if signed { "JP" } else { "JNC" }
            }
            "le" => {
                if signed { "JM" } else { "JC" }
            }
            _ => "JZ",
        };

        self.emit_inst(&format!("{} {}", branch, true_lbl));

        // For gt: also need to check zero
        if kind == "gt" {
            self.emit_inst(&format!("JZ {}", done_lbl)); // if equal, not gt
        }
        // For le: also branch on zero
        if kind == "le" {
            self.emit_inst(&format!("JZ {}", true_lbl));
        }

        // Fall through: result = 0
        self.emit_inst("LXI H,0");
        self.emit_inst(&format!("JMP {}", done_lbl));
        // True path: result = 1
        self.emit_label(&true_lbl);
        self.emit_inst("LXI H,1");
        self.emit_label(&done_lbl);
        self.mark(dst, PhysReg::HL);
    }

    // -- Neg (two's complement) -------------------------------------------

    fn gen_neg(&mut self, dst: VReg, src: VReg, width: Width) {
        match width {
            Width::W8 => {
                self.ensure_a(src);
                self.emit_inst("CMA");
                self.emit_inst("INR A");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(src);
                // Complement HL and increment
                self.emit_inst("MOV A,H");
                self.emit_inst("CMA");
                self.emit_inst("MOV H,A");
                self.emit_inst("MOV A,L");
                self.emit_inst("CMA");
                self.emit_inst("MOV L,A");
                self.emit_inst("INX H");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- Not (bitwise) ----------------------------------------------------

    fn gen_not(&mut self, dst: VReg, src: VReg, width: Width) {
        match width {
            Width::W8 => {
                self.ensure_a(src);
                self.emit_inst("CMA");
                self.mark(dst, PhysReg::A);
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(src);
                self.emit_inst("MOV A,H");
                self.emit_inst("CMA");
                self.emit_inst("MOV H,A");
                self.emit_inst("MOV A,L");
                self.emit_inst("CMA");
                self.emit_inst("MOV L,A");
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    // -- LogicalNot -------------------------------------------------------

    fn gen_logical_not(&mut self, dst: VReg, src: VReg, width: Width) {
        let true_lbl = self.fresh_label();
        let done_lbl = self.fresh_label();

        match width {
            Width::W8 => {
                self.ensure_a(src);
                self.emit_inst("ORA A");
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(src);
                self.emit_inst("MOV A,H");
                self.emit_inst("ORA L");
            }
        }

        self.emit_inst(&format!("JZ {}", true_lbl));
        // Non-zero → result 0
        self.emit_inst("LXI H,0");
        self.emit_inst(&format!("JMP {}", done_lbl));
        // Zero → result 1
        self.emit_label(&true_lbl);
        self.emit_inst("LXI H,1");
        self.emit_label(&done_lbl);
        self.mark(dst, PhysReg::HL);
    }

    // -- Copy -------------------------------------------------------------

    fn gen_copy(&mut self, dst: VReg, src: VReg) {
        let (reg, ops) = self.regalloc.allocate(src);
        self.emit_moves(&ops);
        // If src is already allocated, just re-map dst to the same register.
        let src_loc = self.regalloc.get_location(src).cloned();
        if let Some(Location::Reg(r)) = src_loc {
            // Allocate dst to a different register and emit a move.
            let (dst_reg, dst_ops) = self.regalloc.allocate(dst);
            self.emit_moves(&dst_ops);
            if r != dst_reg {
                self.emit_reg_to_reg(r, dst_reg);
            }
        } else {
            // src is in memory or unallocated — just allocate dst to the
            // same place the allocator chose for src.
            let _ = reg;
            let ops = self.regalloc.mark_allocated(dst, reg);
            self.emit_moves(&ops);
        }
    }

    // -- Cast -------------------------------------------------------------

    fn gen_cast(&mut self, dst: VReg, src: VReg, to_type: &CType) {
        let src_w = src.width;
        let dst_w = dst.width;

        if src_w == dst_w {
            // Same width — just copy.
            self.gen_copy(dst, src);
            return;
        }

        match (src_w, dst_w) {
            // Widen 8 → 16: move A into L, clear/sign-extend H
            (Width::W8, Width::W16) | (Width::W8, Width::W32) => {
                self.ensure_a(src);
                self.emit_inst("MOV L,A");
                if to_type.is_signed() {
                    // Sign extend: if bit 7 set, H=0xFF, else H=0
                    let pos_lbl = self.fresh_label();
                    let done_lbl = self.fresh_label();
                    self.emit_inst("ORA A");
                    self.emit_inst(&format!("JP {}", pos_lbl));
                    self.emit_inst("MVI H,255");
                    self.emit_inst(&format!("JMP {}", done_lbl));
                    self.emit_label(&pos_lbl);
                    self.emit_inst("MVI H,0");
                    self.emit_label(&done_lbl);
                } else {
                    self.emit_inst("MVI H,0");
                }
                self.mark(dst, PhysReg::HL);
            }
            // Narrow 16 → 8: take L into A
            (Width::W16, Width::W8) | (Width::W32, Width::W8) => {
                self.ensure_hl(src);
                self.emit_inst("MOV A,L");
                self.mark(dst, PhysReg::A);
            }
            _ => {
                // W16↔W32 or W32→W16: just copy the 16-bit portion
                self.gen_copy(dst, src);
            }
        }
    }

    // -- Jump -------------------------------------------------------------

    fn gen_jump(&mut self, target: Label) {
        self.emit_inst(&format!("JMP {}", self.ir_label(target)));
    }

    // -- JumpIfTrue / JumpIfFalse -----------------------------------------

    fn gen_jump_if_true(&mut self, cond: VReg, target: Label) {
        let lbl = self.ir_label(target);
        match cond.width {
            Width::W8 => {
                self.ensure_a(cond);
                self.emit_inst("ORA A");
                self.emit_inst(&format!("JNZ {}", lbl));
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(cond);
                self.emit_inst("MOV A,H");
                self.emit_inst("ORA L");
                self.emit_inst(&format!("JNZ {}", lbl));
            }
        }
    }

    fn gen_jump_if_false(&mut self, cond: VReg, target: Label) {
        let lbl = self.ir_label(target);
        match cond.width {
            Width::W8 => {
                self.ensure_a(cond);
                self.emit_inst("ORA A");
                self.emit_inst(&format!("JZ {}", lbl));
            }
            Width::W16 | Width::W32 => {
                self.ensure_hl(cond);
                self.emit_inst("MOV A,H");
                self.emit_inst("ORA L");
                self.emit_inst(&format!("JZ {}", lbl));
            }
        }
    }

    // -- Call -------------------------------------------------------------

    /// Returns true if `func_name` is one of our soft-float runtime helpers
    /// that use the __op1/__op2 calling convention for 32-bit operands.
    fn is_float_runtime_call(func_name: &str) -> bool {
        matches!(
            func_name,
            "__fadd" | "__fsub" | "__fmul" | "__fdiv"
                | "__feq" | "__fne" | "__flt" | "__fle" | "__fgt" | "__fge"
                | "__itof" | "__ftoi"
        )
    }

    fn gen_call(&mut self, func_name: &str, args: &[VReg], dst: Option<VReg>) {
        // Float runtime calls use the __op1/__op2 convention.
        if Self::is_float_runtime_call(func_name) {
            self.gen_float_call(func_name, args, dst);
            return;
        }

        // __builtin_va_start: return the saved va_base pointer.
        if func_name == "__builtin_va_start" {
            self.emit_inst(&format!("LHLD __va_base_{}", self.current_func));
            if let Some(d) = dst {
                self.mark(d, PhysReg::HL);
            }
            return;
        }

        self.spill_live_before_call(func_name);

        // Push stack arguments (index >= 2) right-to-left (C convention).
        let stack_arg_count = if args.len() > 2 { args.len() - 2 } else { 0 };
        for &arg in args.iter().skip(2).rev() {
            self.load_stack_arg_for_push(arg);
        }

        // Place register arguments:
        //   arg0 (16-bit) → HL, arg1 (16-bit) → DE, 8-bit arg0 → A
        // Choose load order using a simple cost model to minimize moves/spills.
        if !args.is_empty() {
            let arg0 = args[0];
            let arg0_target = if arg0.width == Width::W8 { PhysReg::A } else { PhysReg::HL };
            let arg0_first_cost = self.arg_place_cost(arg0, arg0_target)
                + if args.len() > 1 { self.arg_place_cost(args[1], PhysReg::DE) } else { 0 };
            let arg1_first_cost = if args.len() > 1 {
                self.arg_place_cost(args[1], PhysReg::DE) + self.arg_place_cost(arg0, arg0_target)
            } else {
                arg0_first_cost
            };

            if args.len() > 1 && arg1_first_cost <= arg0_first_cost {
                self.ensure_de(args[1]);
                if arg0.width == Width::W8 {
                    self.ensure_a(arg0);
                } else {
                    self.ensure_hl(arg0);
                }
            } else {
                if arg0.width == Width::W8 {
                    self.ensure_a(arg0);
                } else {
                    self.ensure_hl(arg0);
                }
                if args.len() > 1 {
                    self.ensure_de(args[1]);
                }
            }
        }

        self.emit_call_with_effects(func_name);

        // Clean up stack arguments.
        for _ in 0..stack_arg_count {
            self.emit_inst("POP B");
        }

        // Result is in HL (16-bit) or A (8-bit).
        if let Some(d) = dst {
            match d.width {
                Width::W8 => self.mark(d, PhysReg::A),
                Width::W16 | Width::W32 => self.mark(d, PhysReg::HL),
            }
        }
    }

    /// Generate a call to a soft-float runtime function using __op1/__op2.
    fn gen_float_call(&mut self, func_name: &str, args: &[VReg], dst: Option<VReg>) {
        self.spill_live_before_call(func_name);

        match func_name {
            // 2-arg: both W32 via __op1/__op2
            "__fadd" | "__fsub" | "__fmul" | "__fdiv"
            | "__feq" | "__fne" | "__flt" | "__fle" | "__fgt" | "__fge" => {
                if args.len() >= 2 {
                    let lhs_label = self.w32_mem_label(args[0]);
                    let rhs_label = self.w32_mem_label(args[1]);
                    self.emit_w32_to_op1(&lhs_label);
                    self.emit_w32_to_op2(&rhs_label);
                }
                self.emit_call_with_effects(func_name);
            }
            // __itof: int16 in HL → float in __op1
            "__itof" => {
                if !args.is_empty() {
                    self.ensure_hl(args[0]);
                }
                self.emit_call_with_effects(func_name);
            }
            // __ftoi: float in __op1 → int16 in HL
            "__ftoi" => {
                if !args.is_empty() {
                    let src_label = self.w32_mem_label(args[0]);
                    self.emit_w32_to_op1(&src_label);
                }
                self.emit_call_with_effects(func_name);
            }
            _ => unreachable!(),
        }

        if let Some(d) = dst {
            match d.width {
                // Comparisons and __ftoi return W16 in HL.
                Width::W16 => self.mark(d, PhysReg::HL),
                // Arithmetic results: low 16 of __op1 in HL.
                Width::W32 => self.mark(d, PhysReg::HL),
                Width::W8 => self.mark(d, PhysReg::A),
            }
        }
    }

    // -- Return -----------------------------------------------------------

    fn gen_return(&mut self, value: Option<VReg>) {
        if let Some(v) = value {
            match v.width {
                Width::W8 => self.ensure_a(v),
                Width::W16 | Width::W32 => self.ensure_hl(v),
            }
        }
        self.emit_inst("RET");
    }

    // -- AddrOfGlobal -----------------------------------------------------

    fn gen_addr_of_global(&mut self, dst: VReg, name: &str) {
        self.emit_inst(&format!("LXI H,{}", name));
        self.mark(dst, PhysReg::HL);
    }

    // -- PtrAdd -----------------------------------------------------------

    fn gen_ptr_add(&mut self, dst: VReg, ptr: VReg, offset: VReg, element_size: u16) {
        if element_size == 1 {
            // Simple: HL = ptr + offset
            self.ensure_de(offset);
            self.ensure_hl(ptr);
            self.emit_inst("DAD D");
        } else {
            // offset * element_size, then add to ptr
            self.spill_live_before_call("__mul16");
            self.ensure_hl(offset);
            self.emit_inst(&format!("LXI D,{}", element_size));
            self.emit_call_with_effects("__mul16");
            // HL = scaled offset, now add ptr
            self.emit_inst("XCHG"); // DE = scaled offset
            self.ensure_hl(ptr);
            self.emit_inst("DAD D");
        }
        self.mark(dst, PhysReg::HL);
    }
}

// ---------------------------------------------------------------------------
// Data section generation
// ---------------------------------------------------------------------------

impl CodeGenerator {
    fn gen_data_section(&mut self, program: &IrProgram) {
        let mut emitted_labels: HashSet<String> = HashSet::new();
        let local_size_map: HashMap<String, usize> = program
            .globals
            .iter()
            .map(|gvar| {
                let label = if gvar.name.starts_with('_') {
                    gvar.name.clone()
                } else {
                    format!("_g_{}", gvar.name)
                };
                (label, gvar.ty.size_of().unwrap_or(2))
            })
            .collect();

        // Build the set of labels actually referenced in the code section so far.
        // _l_-prefixed locals that are never loaded or stored (because all their
        // uses were constant-folded away) can be omitted from the data section.
        let referenced_in_code: HashSet<String> = self.output.iter()
            .flat_map(|line| {
                let mut refs = Vec::new();
                let mut rest = line.as_str();
                while let Some(pos) = rest.find('_') {
                    let tail = &rest[pos..];
                    let end = tail
                        .find(|c: char| c.is_whitespace() || c == ',' || c == '+' || c == ';')
                        .unwrap_or(tail.len());
                    refs.push(tail[..end].to_string());
                    rest = &tail[1..];
                }
                refs
            })
            .collect();

        // Global variables
        for gvar in &program.globals {
            // IR symbols may already be canonical labels (e.g. `_g_x`, `_l_f_a`).
            let label = if gvar.name.starts_with('_') {
                gvar.name.clone()
            } else {
                format!("_g_{}", gvar.name)
            };
            if self.analysis.local_allocs.contains_key(&label) {
                continue;
            }
            // Skip _l_-prefixed function-local slots that are never referenced
            // in the generated code (e.g. loop IVs fully folded to constants).
            if label.starts_with("_l_") && !referenced_in_code.contains(&label) {
                continue;
            }
            if !emitted_labels.insert(label.clone()) {
                continue;
            }
            self.emit_label(&label);
            let size = gvar.ty.size_of().unwrap_or(2);
            if let Some(init) = &gvar.init {
                for byte in init {
                    self.emit_inst(&format!("DB {}", byte));
                }
                // Pad if init is shorter than type size
                for _ in init.len()..size {
                    self.emit_inst("DB 0");
                }
            } else {
                self.emit_inst(&format!(".storage {}", size));
            }
        }

        // String literals
        for slit in &program.strings {
            if !emitted_labels.insert(slit.label.clone()) {
                continue;
            }
            self.emit_label(&slit.label);
            for byte in &slit.data {
                self.emit_inst(&format!("DB {}", byte));
            }
        }

        // Spill slots — emit enough space for all spill labels used.
        // We scan the output for __spill_ references and emit them.
        let mut spill_labels: Vec<String> = Vec::new();
        for line in &self.output {
            if let Some(pos) = line.find("__spill_") {
                let rest = &line[pos..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == ',' || c == ')')
                    .unwrap_or(rest.len());
                let label = rest[..end].to_string();
                if !spill_labels.contains(&label) {
                    spill_labels.push(label);
                }
            }
        }
        for label in &spill_labels {
            if !emitted_labels.insert(label.clone()) {
                continue;
            }
            self.emit_label(label);
            self.emit_inst(".storage 2");
        }

        // Static local/param allocations from the analysis.
        // Labels that share the same address are emitted as aliases to the
        // same storage block, which is how call-tree frame reuse becomes a
        // real space saving in the output assembly.
        let mut slot_groups: HashMap<u16, Vec<String>> = HashMap::new();
        for (label, &addr) in &self.analysis.local_allocs {
            slot_groups.entry(addr).or_default().push(label.clone());
        }
        let mut ordered_slots: Vec<(u16, Vec<String>)> = slot_groups.into_iter().collect();
        ordered_slots.sort_by_key(|(addr, _)| *addr);
        for (_addr, mut labels) in ordered_slots {
            labels.sort();
            let storage_size = labels
                .iter()
                .map(|label| local_size_map.get(label).copied().unwrap_or(2))
                .max()
                .unwrap_or(2);
            for label in labels {
                if !emitted_labels.insert(label.clone()) {
                    continue;
                }
                self.emit_label(&label);
            }
            self.emit_inst(&format!(".storage {}", storage_size));
        }

        // va_base labels for variadic functions
        for func in &program.functions {
            if func.is_variadic {
                let label = format!("__va_base_{}", func.name);
                if !emitted_labels.insert(label.clone()) {
                    continue;
                }
                self.emit_label(&label);
                self.emit_inst(".storage 2");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Map a physical register pair to its low-byte 8080 register name.
fn low_byte_name(reg: PhysReg) -> &'static str {
    match reg {
        PhysReg::A => "A",
        PhysReg::BC => "C",
        PhysReg::DE => "E",
        PhysReg::HL => "L",
    }
}

// ---------------------------------------------------------------------------
// Liveness analysis for global register allocation
// ---------------------------------------------------------------------------

/// Compute the last instruction index where each vreg is *used* (read as
/// a source operand).  This enables the code generator to free a register
/// as soon as the value it holds is no longer needed — a simple form of
/// whole-function (global) register allocation.
fn compute_last_use(body: &[IrInstr]) -> HashMap<u32, usize> {
    let mut last: HashMap<u32, usize> = HashMap::new();
    for (idx, instr) in body.iter().enumerate() {
        for id in collect_op_src_ids(&instr.op) {
            last.insert(id, idx);
        }
    }
    last
}

/// Collect all vreg IDs that are *read* by an IR operation (source operands).
fn collect_op_src_ids(op: &IrOp) -> Vec<u32> {
    let mut ids = Vec::new();
    match op {
        IrOp::LoadImm { .. } | IrOp::LoadGlobal { .. } | IrOp::LoadLocal { .. }
        | IrOp::Label { .. } | IrOp::Jump { .. } | IrOp::AddrOfGlobal { .. } => {}
        IrOp::StoreGlobal { src, .. } | IrOp::StoreLocal { src, .. } => { ids.push(src.id); }
        IrOp::LoadPtr { ptr, .. } => { ids.push(ptr.id); }
        IrOp::StorePtr { ptr, src } => { ids.push(ptr.id); ids.push(src.id); }
        IrOp::Add { lhs, rhs, .. } | IrOp::Sub { lhs, rhs, .. }
        | IrOp::Mul { lhs, rhs, .. } | IrOp::Div { lhs, rhs, .. }
        | IrOp::Mod { lhs, rhs, .. } | IrOp::And { lhs, rhs, .. }
        | IrOp::Or { lhs, rhs, .. } | IrOp::Xor { lhs, rhs, .. }
        | IrOp::Shl { lhs, rhs, .. } | IrOp::Shr { lhs, rhs, .. }
        | IrOp::Eq { lhs, rhs, .. } | IrOp::Ne { lhs, rhs, .. }
        | IrOp::Lt { lhs, rhs, .. } | IrOp::Le { lhs, rhs, .. }
        | IrOp::Gt { lhs, rhs, .. } | IrOp::Ge { lhs, rhs, .. } => {
            ids.push(lhs.id);
            ids.push(rhs.id);
        }
        IrOp::Neg { src, .. } | IrOp::Not { src, .. } | IrOp::LogicalNot { src, .. } => {
            ids.push(src.id);
        }
        IrOp::Copy { src, .. } | IrOp::Cast { src, .. } => { ids.push(src.id); }
        IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => { ids.push(cond.id); }
        IrOp::Call { args, .. } => {
            for a in args { ids.push(a.id); }
        }
        IrOp::Return { value } => {
            if let Some(v) = value { ids.push(v.id); }
        }
        IrOp::PtrAdd { ptr, offset, .. } => {
            ids.push(ptr.id);
            ids.push(offset.id);
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callgraph;
    use crate::ir::*;
    use crate::types::CType;

    /// Build a trivial CallGraphAnalysis for a program.
    fn simple_analysis(prog: &IrProgram) -> CallGraphAnalysis {
        callgraph::analyze(prog, Some(0x8000))
    }

    /// Helper: build a program with a single function, generate code, and
    /// return the output lines.
    fn gen_single_func(func: IrFunction) -> Vec<String> {
        let prog = IrProgram {
            globals: Vec::new(),
            functions: vec![func],
            strings: Vec::new(),
        };
        let analysis = simple_analysis(&prog);
        generate(&prog, &analysis)
    }

    /// Check that the output contains a line matching `needle` (substring).
    fn has_line(output: &[String], needle: &str) -> bool {
        output.iter().any(|l| l.contains(needle))
    }

    // -- LoadImm ---------------------------------------------------------

    #[test]
    fn load_imm_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let dst = VReg::new(0, Width::W16);
        f.push_op(IrOp::load_imm(dst, 42));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LXI H,42"));
    }

    #[test]
    fn load_imm_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let dst = VReg::new(0, Width::W8);
        f.push_op(IrOp::load_imm(dst, 7));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MVI A,7"));
    }

    // -- LoadGlobal / StoreGlobal ----------------------------------------

    #[test]
    fn load_store_global_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let r = VReg::new(0, Width::W16);
        f.push_op(IrOp::load_global(r, "_g_x"));
        f.push_op(IrOp::store_global("_g_y", r));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LHLD _g_x"));
        assert!(has_line(&out, "SHLD _g_y"));
    }

    #[test]
    fn load_store_global_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let r = VReg::new(0, Width::W8);
        f.push_op(IrOp::load_global(r, "_g_c"));
        f.push_op(IrOp::store_global("_g_d", r));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LDA _g_c"));
        assert!(has_line(&out, "STA _g_d"));
    }

    // -- Add --------------------------------------------------------------

    #[test]
    fn add_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 10));
        f.push_op(IrOp::load_imm(b, 20));
        f.push_op(IrOp::add(c, a, b, Width::W16));
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "DAD D"));
    }

    #[test]
    fn add_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 3));
        f.push_op(IrOp::load_imm(b, 4));
        f.push_op(IrOp::add(c, a, b, Width::W8));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        // Should contain an ADD instruction
        assert!(has_line(&out, "ADD"));
    }

    // -- Sub --------------------------------------------------------------

    #[test]
    fn sub_16bit_uses_complement_and_dad() {
        // With a known-immediate rhs the optimised path emits LXI D,(-k) / DAD D
        // instead of the complement-and-DAD sequence.  The complement path is
        // still exercised via gen_sub's general (non-immediate) fallback.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 100));
        f.push_op(IrOp::load_imm(b, 30));
        f.push_op(IrOp::sub(c, a, b, Width::W16));
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        // Expect the two's-complement equivalent: LXI D,(65536-30)=65506 / DAD D
        assert!(has_line(&out, "LXI D"), "expected LXI D but got:\n{}", out.join("\n"));
        assert!(has_line(&out, "DAD D"), "expected DAD D but got:\n{}", out.join("\n"));
        assert!(!has_line(&out, "CMA"), "unexpected CMA — should use LXI D path");
    }

    // -- Mul / Div / Mod --------------------------------------------------

    #[test]
    fn mul_calls_runtime() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 5));
        f.push_op(IrOp::load_imm(b, 6));
        f.push_op(IrOp::Mul {
            dst: c, lhs: a, rhs: b, width: Width::W16, signed: false,
        });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __mul16"));
    }

    #[test]
    fn div_calls_runtime_signed() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 10));
        f.push_op(IrOp::load_imm(b, 3));
        f.push_op(IrOp::Div {
            dst: c, lhs: a, rhs: b, width: Width::W16, signed: true,
        });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __div16s"));
    }

    #[test]
    fn mod_calls_runtime_unsigned() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 10));
        f.push_op(IrOp::load_imm(b, 3));
        f.push_op(IrOp::Mod {
            dst: c, lhs: a, rhs: b, width: Width::W16, signed: false,
        });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __mod16u"));
    }

    // -- Neg / Not / LogicalNot -------------------------------------------

    #[test]
    fn neg_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W8);
        f.push_op(IrOp::load_imm(a, 5));
        f.push_op(IrOp::Neg { dst: b, src: a, width: Width::W8 });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CMA"));
        assert!(has_line(&out, "INR A"));
    }

    #[test]
    fn not_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_imm(a, 0xFF00));
        f.push_op(IrOp::Not { dst: b, src: a, width: Width::W16 });
        f.push_op(IrOp::ret(Some(b)));
        let out = gen_single_func(f);
        // Should complement both H and L
        let cma_count = out.iter().filter(|l| l.contains("CMA")).count();
        assert!(cma_count >= 2);
    }

    #[test]
    fn logical_not_produces_0_or_1() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_imm(a, 42));
        f.push_op(IrOp::LogicalNot { dst: b, src: a, width: Width::W16 });
        f.push_op(IrOp::ret(Some(b)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LXI H,0"));
        assert!(has_line(&out, "LXI H,1"));
    }

    // -- Jump / JumpIfTrue / JumpIfFalse ----------------------------------

    #[test]
    fn unconditional_jump() {
        let mut f = IrFunction::new("test", CType::Void);
        let lbl = Label::new(5);
        f.push_op(IrOp::label(lbl));
        f.push_op(IrOp::jump(lbl));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "JMP L5__test"));
        assert!(has_line(&out, "L5__test:"));
    }

    #[test]
    fn jump_if_false_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let cond = VReg::new(0, Width::W16);
        let lbl = Label::new(3);
        f.push_op(IrOp::load_imm(cond, 0));
        f.push_op(IrOp::jump_if_false(cond, lbl));
        f.push_op(IrOp::label(lbl));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "ORA L"));
        assert!(has_line(&out, "JZ L3"));
    }

    #[test]
    fn jump_if_true_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let cond = VReg::new(0, Width::W8);
        let lbl = Label::new(1);
        f.push_op(IrOp::load_imm(cond, 1));
        f.push_op(IrOp::jump_if_true(cond, lbl));
        f.push_op(IrOp::label(lbl));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "ORA A"));
        assert!(has_line(&out, "JNZ L1"));
    }

    // -- Call / Return ----------------------------------------------------

    #[test]
    fn call_emits_call_instruction() {
        let mut f = IrFunction::new("test", CType::Void);
        let r = VReg::new(0, Width::W16);
        f.push_op(IrOp::call("puts", vec![], Some(r)));
        f.push_op(IrOp::ret(Some(r)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL puts"));
    }

    #[test]
    fn return_with_value() {
        let mut f = IrFunction::new("test", CType::int_signed());
        let r = VReg::new(0, Width::W16);
        f.push_op(IrOp::load_imm(r, 99));
        f.push_op(IrOp::ret(Some(r)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "RET"));
        assert!(has_line(&out, "LXI H,99"));
    }

    // -- Comparison -------------------------------------------------------

    #[test]
    fn eq_comparison_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 5));
        f.push_op(IrOp::load_imm(b, 5));
        f.push_op(IrOp::Eq { dst: c, lhs: a, rhs: b, width: Width::W8 });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CMP C"));
        assert!(has_line(&out, "JZ"));
        assert!(has_line(&out, "LXI H,1"));
        assert!(has_line(&out, "LXI H,0"));
    }

    // -- AddrOfGlobal ----------------------------------------------------

    #[test]
    fn addr_of_global() {
        let mut f = IrFunction::new("test", CType::Void);
        let r = VReg::new(0, Width::W16);
        f.push_op(IrOp::addr_of_global(r, "_g_buf"));
        f.push_op(IrOp::ret(Some(r)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LXI H,_g_buf"));
    }

    // -- LoadPtr / StorePtr -----------------------------------------------

    #[test]
    fn load_ptr_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let val = VReg::new(1, Width::W8);
        f.push_op(IrOp::load_imm(ptr, 0x1000));
        f.push_op(IrOp::load_ptr(val, ptr));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MOV A,M"));
    }

    #[test]
    fn store_ptr_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let val = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_imm(ptr, 0x2000));
        f.push_op(IrOp::load_imm(val, 0x1234));
        f.push_op(IrOp::store_ptr(ptr, val));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        // With a constant ptr address, codegen uses SHLD instead of MOV M,E/D.
        assert!(has_line(&out, "SHLD"), "expected SHLD but got:\n{}", out.join("\n"));
    }

    // -- PtrAdd -----------------------------------------------------------

    #[test]
    fn ptr_add_element_size_1() {
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let off = VReg::new(1, Width::W16);
        let dst = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(ptr, 0x1000));
        f.push_op(IrOp::load_imm(off, 5));
        f.push_op(IrOp::ptr_add(dst, ptr, off, 1));
        f.push_op(IrOp::ret(Some(dst)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "DAD D"));
    }

    #[test]
    fn ptr_add_element_size_2() {
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let off = VReg::new(1, Width::W16);
        let dst = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(ptr, 0x1000));
        f.push_op(IrOp::load_imm(off, 3));
        f.push_op(IrOp::ptr_add(dst, ptr, off, 2));
        f.push_op(IrOp::ret(Some(dst)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __mul16"));
        assert!(has_line(&out, "DAD D"));
    }

    // -- Shift ------------------------------------------------------------

    #[test]
    fn shl_16bit_calls_runtime() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 1));
        f.push_op(IrOp::load_imm(b, 4));
        f.push_op(IrOp::Shl { dst: c, lhs: a, rhs: b, width: Width::W16 });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __shl16"));
    }

    // -- Bitwise ----------------------------------------------------------

    #[test]
    fn and_8bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 0xFF));
        f.push_op(IrOp::load_imm(b, 0x0F));
        f.push_op(IrOp::And { dst: c, lhs: a, rhs: b, width: Width::W8 });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "ANA C"));
    }

    #[test]
    fn or_16bit() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 0x00FF));
        f.push_op(IrOp::load_imm(b, 0xFF00));
        f.push_op(IrOp::Or { dst: c, lhs: a, rhs: b, width: Width::W16 });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "ORA E"));
        assert!(has_line(&out, "ORA D"));
    }

    // -- Data section -----------------------------------------------------

    #[test]
    fn data_section_globals() {
        let prog = IrProgram {
            globals: vec![crate::ir::GlobalVar {
                name: "x".into(),
                ty: CType::int_signed(),
                init: None,
            }],
            functions: vec![],
            strings: vec![],
        };
        let analysis = simple_analysis(&prog);
        let out = generate(&prog, &analysis);
        assert!(has_line(&out, "_g_x:"));
        assert!(has_line(&out, ".storage 2"));
    }

    #[test]
    fn data_section_strings() {
        let prog = IrProgram {
            globals: vec![],
            functions: vec![],
            strings: vec![crate::ir::StringLiteral {
                label: "_S0".into(),
                data: vec![72, 105, 0],
            }],
        };
        let analysis = simple_analysis(&prog);
        let out = generate(&prog, &analysis);
        assert!(has_line(&out, "_S0:"));
        assert!(has_line(&out, "DB 72"));
    }

    // -- Cast -------------------------------------------------------------

    #[test]
    fn cast_widen_8_to_16_unsigned() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_imm(a, 42));
        f.push_op(IrOp::cast(b, a, CType::int_unsigned()));
        f.push_op(IrOp::ret(Some(b)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MOV L,A"));
        assert!(has_line(&out, "MVI H,0"));
    }

    #[test]
    fn cast_narrow_16_to_8() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W8);
        f.push_op(IrOp::load_imm(a, 0x1234));
        f.push_op(IrOp::cast(b, a, CType::char_signed()));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MOV A,L"));
    }

    // -- Function label and RET -------------------------------------------

    #[test]
    fn function_emits_label_and_ret() {
        let f = IrFunction::new("my_func", CType::Void);
        let out = gen_single_func(f);
        assert!(has_line(&out, "my_func:"));
        assert!(has_line(&out, "RET"));
    }
}
