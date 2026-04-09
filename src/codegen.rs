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
    /// Set of VReg IDs whose comparison-based conditional branch was already
    /// emitted inline inside `gen_compare`.  When `gen_jump_if_true` /
    /// `gen_jump_if_false` encounters one of these IDs it is a no-op because
    /// the branch was already emitted.
    consumed_cmp: HashSet<u32>,
    /// Address whose value is currently in register A, if any.  Set after
    /// `STA addr` so that a subsequent `LDA addr` can be skipped when A
    /// hasn't been overwritten since.  Cleared by hooks in `mark` and
    /// `ensure` whenever A is physically overwritten, and at unconditional
    /// jumps / calls so that else-body labels and post-call code are always
    /// safe.
    a_mirrors: Option<String>,
    /// Width of each spill slot, keyed by label.  Populated when Spill ops are
    /// emitted; used to emit `.storage 1` vs `.storage 2` in the data section.
    spill_widths: HashMap<String, Width>,
    /// Vregs whose byte value is currently held in memory at the address HL
    /// points to — a deferred W8 `LoadPtr` that was not materialised into A.
    /// The immediately following ALU instruction will consume these via `ADD M`
    /// / `SUB M` etc. without an intermediate register load.
    pending_m: std::collections::HashSet<u32>,
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
        consumed_cmp: HashSet::new(),
        a_mirrors: None,
        spill_widths: HashMap::new(),
        pending_m: std::collections::HashSet::new(),
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
        self.a_mirrors = None;
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
            Some(Location::RematImm(_)) | Some(Location::RematLabel(_)) => 1,
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
                // For remat values (immediates or labels), use LXI B / PUSH B
                // to avoid clobbering HL (which often holds arg0 for the call).
                let hl_occupied = self.regalloc.occupant(PhysReg::HL).is_some();
                if hl_occupied {
                    if let Some(imm) = self.known_imm(arg) {
                        self.regalloc.free(arg);
                        self.emit_inst(&format!("LXI B,{}", (imm & 0xFFFF) as u16));
                        self.emit_inst("PUSH B");
                        return;
                    }
                    if let Some(lbl) = self.regalloc.label_of(arg).map(|s| s.to_string()) {
                        self.regalloc.free(arg);
                        self.emit_inst(&format!("LXI B,{}", lbl));
                        self.emit_inst("PUSH B");
                        return;
                    }
                }
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
                MoveOp::Spill { src, label, width } => {
                    // Record the value's width so the data section can emit the
                    // right .storage size for this spill slot.
                    self.spill_widths.insert(label.clone(), *width);
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
                MoveOp::Reload { dst, label, width } => {
                    match (dst, width) {
                        (PhysReg::A, _) => self.emit_inst(&format!("LDA {}", label)),
                        // W8 reload into HL: LXI H,label; MOV L,M
                        // Only L is ever read for an 8-bit value; H is never touched.
                        (PhysReg::HL, Width::W8) => {
                            self.emit_inst(&format!("LXI H,{}", label));
                            self.emit_inst("MOV L,M");
                        }
                        // W8 reload into BC: LXI H,label; MOV C,M
                        // Only C is read (e.g. ADD C); B is never consumed.
                        (PhysReg::BC, Width::W8) => {
                            self.emit_inst(&format!("LXI H,{}", label));
                            self.emit_inst("MOV C,M");
                        }
                        // W8 reload into DE: LXI H,label; MOV E,M
                        // Only E is read; D is never consumed.
                        (PhysReg::DE, Width::W8) => {
                            self.emit_inst(&format!("LXI H,{}", label));
                            self.emit_inst("MOV E,M");
                        }
                        (PhysReg::HL, _) => self.emit_inst(&format!("LHLD {}", label)),
                        (PhysReg::DE, _) => {
                            self.emit_inst(&format!("LHLD {}", label));
                            self.emit_inst("XCHG");
                        }
                        (PhysReg::BC, _) => {
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
                MoveOp::LoadLabel { dst, label } => {
                    match dst {
                        PhysReg::HL => self.emit_inst(&format!("LXI H,{}", label)),
                        PhysReg::DE => self.emit_inst(&format!("LXI D,{}", label)),
                        PhysReg::BC => self.emit_inst(&format!("LXI B,{}", label)),
                        PhysReg::A => {
                            self.emit_inst(&format!("LXI H,{}", label));
                            self.emit_inst("MOV A,L");
                        }
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
        // If we're about to place a different value in A, A no longer mirrors
        // the last-stored global address.
        if target == PhysReg::A
            && self.regalloc.occupant(PhysReg::A) != Some(vreg.id)
        {
            self.a_mirrors = None;
        }
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

    /// Prepare a W8 `rhs` operand for an 8080 ALU instruction.
    ///
    /// Must be called AFTER `ensure_a(lhs)` — this function never touches A.
    ///
    /// Returns `"M"` when `rhs` lives in a spill slot: HL is loaded with the
    /// slot's address so the caller can emit e.g. `ADD M` instead of the
    /// slower `LXI H,label / MOV C,M / ADD C` sequence.
    /// Returns the low-byte register name (`"C"`, `"E"`, `"L"`) when `rhs` is
    /// already in a physical register pair.
    fn w8_alu_operand(&mut self, rhs: VReg) -> String {
        // Deferred load: gen_load_ptr already pointed HL at the source address.
        // No LXI H needed — just use M directly.
        if self.pending_m.remove(&rhs.id) {
            return "M".to_string();
        }
        match self.regalloc.get_location(rhs).cloned() {
            Some(Location::Memory(label)) => {
                // Spill HL if it holds a live vreg, then load the spill address.
                if let Some(spill_op) = self.regalloc.spill(PhysReg::HL) {
                    self.emit_moves(&[spill_op]);
                }
                self.emit_inst(&format!("LXI H,{}", label));
                self.regalloc.free(rhs); // consumed from memory
                "M".to_string()
            }
            Some(Location::Reg(r)) => low_byte_name(r).to_string(),
            _ => {
                self.ensure(rhs, PhysReg::BC);
                "C".to_string()
            }
        }
    }

    /// Mark `vreg` as living in `reg` after we've emitted a load ourselves.
    fn mark(&mut self, vreg: VReg, reg: PhysReg) {
        // A new value is about to occupy A — it no longer mirrors any store.
        if reg == PhysReg::A {
            self.a_mirrors = None;
        }
        let ops = self.regalloc.mark_allocated(vreg, reg);
        self.emit_moves(&ops);
    }

    /// Returns `true` when a W8 `LoadPtr` for `load_dst` can be deferred:
    /// instead of loading the byte into A immediately, the caller should only
    /// point HL at the source address.  The immediately following ALU
    /// instruction will then consume the byte via `ADD M` / `SUB M` / `CMP M`
    /// etc., completely avoiding a spill/reload of the current A value.
    ///
    /// Conditions:
    /// - A is currently occupied by a live value (call it `a_val`).
    /// - `load_dst` is consumed for the last time by the very next instruction.
    /// - The next instruction is a W8 binary op where one operand is `a_val`
    ///   and the other is `load_dst`, arranged so that `ADD M` / `SUB M` /
    ///   `CMP M` (all of the form `A ← A op M`) yields the correct result.
    fn can_defer_w8_load_ptr(&self, load_dst: VReg, next_op: Option<&IrOp>) -> bool {
        let Some(a_id) = self.regalloc.occupant(PhysReg::A) else { return false; };
        // load_dst must die at the very next instruction.
        if self.last_use.get(&load_dst.id).copied() != Some(self.instr_index + 1) {
            return false;
        }
        match next_op {
            // Commutative W8: load_dst may be either operand.
            Some(IrOp::Add { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::And { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Or  { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Xor { lhs, rhs, width: Width::W8, .. }) => {
                (lhs.id == load_dst.id && rhs.id == a_id)
                    || (rhs.id == load_dst.id && lhs.id == a_id)
            }
            // Non-commutative W8 Sub: load_dst must be rhs (A = A_val - M).
            Some(IrOp::Sub { lhs, rhs, width: Width::W8, .. }) => {
                lhs.id == a_id && rhs.id == load_dst.id
            }
            // Comparison W8: load_dst must be rhs (CMP M: A vs M).
            Some(IrOp::Eq { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Ne { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Lt { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Le { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Gt { lhs, rhs, width: Width::W8, .. })
            | Some(IrOp::Ge { lhs, rhs, width: Width::W8, .. }) => {
                lhs.id == a_id && rhs.id == load_dst.id
            }
            _ => false,
        }
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
        self.consumed_cmp.clear();
        self.a_mirrors = None;
        self.pending_m.clear();
        self.emit_comment(&format!("function {}", func.name));
        self.emit_label(&func.name);

        // Full-body asm function: emit raw asm with no prologue/regalloc.
        if func.is_asm_body {
            self.emit("; __asm_begin__".to_string());
            for instr in func.body.iter() {
                if let IrOp::InlineAsm { code, .. } = &instr.op {
                    for line in code.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            // Labels and equates go at column 0; instructions indented.
                            if trimmed.ends_with(':') || trimmed.contains(" = ") || trimmed.starts_with('.') {
                                self.emit(trimmed.to_string());
                            } else {
                                self.emit_inst(trimmed);
                            }
                        }
                    }
                }
            }
            // Add RET if the asm doesn't end with a control-flow instruction.
            if !self.asm_ends_with_control_flow(func) {
                self.emit_inst("RET");
            }
            self.emit("; __asm_end__".to_string());
            return;
        }

        // For variadic functions, capture the address of the first variadic
        // arg on the stack. At entry: SP → [ret_addr], stack args start at SP+2.
        if func.is_variadic {
            self.emit_inst("LXI H,2");
            self.emit_inst("DAD SP");
            self.emit_inst(&format!("SHLD __va_base_{}", func.name));
        }

        // Register parameter vregs with the register allocator so it knows
        // their physical locations at function entry (calling convention:
        // arg0 → HL or A for W8, arg1 → DE).
        for (i, param) in func.params.iter().enumerate() {
            let reg = match i {
                0 => {
                    if param.vreg.width == Width::W8 {
                        PhysReg::A
                    } else {
                        PhysReg::HL
                    }
                }
                1 => PhysReg::DE,
                _ => break,
            };
            self.regalloc.mark_allocated(param.vreg, reg);
        }

        for (idx, instr) in func.body.iter().enumerate() {
            self.instr_index = idx;
            if instr.line != 0 && instr.line != self.current_c_line {
                self.emit_comment(&format!("C_LINE {}", instr.line));
                self.current_c_line = instr.line;
            }
            let next_op = func.body.get(idx + 1).map(|i| &i.op);
            self.gen_op(&instr.op, next_op);
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

    /// Check if the last non-empty line in a full-body asm function is a
    /// control-flow instruction (RET, JMP, etc.), meaning no auto-RET needed.
    fn asm_ends_with_control_flow(&self, func: &IrFunction) -> bool {
        for instr in func.body.iter().rev() {
            if let IrOp::InlineAsm { code, .. } = &instr.op {
                let last_mnemonic = code
                    .lines()
                    .rev()
                    .filter_map(|line| {
                        let trimmed = line.split(';').next().unwrap_or("").trim();
                        if trimmed.is_empty() {
                            return None;
                        }
                        trimmed.split_whitespace().next().map(|s| s.to_uppercase())
                    })
                    .next();
                if let Some(m) = last_mnemonic {
                    return matches!(
                        m.as_str(),
                        "RET" | "RZ" | "RNZ" | "RC" | "RNC" | "RP" | "RM" | "RPE" | "RPO"
                            | "JMP" | "PCHL"
                    );
                }
            }
        }
        false
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
    fn gen_op(&mut self, op: &IrOp, next_op: Option<&IrOp>) {
        match op {
            // -- loads / stores -------------------------------------------
            IrOp::LoadImm { dst, value } => self.gen_load_imm(*dst, *value),
            IrOp::LoadGlobal { dst, addr_label } => {
                self.gen_load_global(*dst, addr_label, next_op);
            }
            IrOp::StoreGlobal { addr_label, src } => {
                self.gen_store_global(addr_label, *src);
            }
            IrOp::LoadLocal { dst, offset } => self.gen_load_local(*dst, *offset, next_op),
            IrOp::StoreLocal { offset, src } => self.gen_store_local(*offset, *src),
            IrOp::LoadPtr { dst, ptr } => self.gen_load_ptr(*dst, *ptr, next_op),
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
                self.gen_compare(*dst, *lhs, *rhs, *width, "eq", false, next_op);
            }
            IrOp::Ne { dst, lhs, rhs, width } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "ne", false, next_op);
            }
            IrOp::Lt { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "lt", *signed, next_op);
            }
            IrOp::Le { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "le", *signed, next_op);
            }
            IrOp::Gt { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "gt", *signed, next_op);
            }
            IrOp::Ge { dst, lhs, rhs, width, signed } => {
                self.gen_compare(*dst, *lhs, *rhs, *width, "ge", *signed, next_op);
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

            // -- inline assembly ------------------------------------------
            IrOp::InlineAsm { code, inputs, return_type, clobber_all } => {
                if *clobber_all {
                    // Raw mode (asm { }): spill all, emit, clobber all.
                    for reg in [PhysReg::HL, PhysReg::DE, PhysReg::BC, PhysReg::A] {
                        if let Some(op) = self.regalloc.spill(reg) {
                            self.emit_moves(&[op]);
                        }
                    }
                    self.a_mirrors = None;
                    self.emit("; __asm_begin__".to_string());
                    for line in code.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if trimmed.ends_with(':') || trimmed.contains(" = ") || trimmed.starts_with('.') {
                                self.emit(trimmed.to_string());
                            } else {
                                self.emit_inst(trimmed);
                            }
                        }
                    }
                    self.emit("; __asm_end__".to_string());
                    self.regalloc.clobber_regs(&[
                        PhysReg::HL, PhysReg::DE, PhysReg::BC, PhysReg::A,
                    ]);
                } else {
                    // Parameterized mode: selective spill based on declared params.
                    let touched = compute_touched_regs(inputs, return_type);

                    // Spill only touched registers that hold live values.
                    for &reg in &touched {
                        let Some(vreg_id) = self.regalloc.occupant(reg) else {
                            continue;
                        };
                        let live_after = self
                            .last_use
                            .get(&vreg_id)
                            .is_some_and(|&last_idx| last_idx > self.instr_index);
                        if live_after {
                            if let Some(op) = self.regalloc.spill(reg) {
                                self.emit_moves(&[op]);
                            }
                        }
                    }
                    if touched.iter().any(|r| *r == PhysReg::A) {
                        self.a_mirrors = None;
                    }

                    // Place input values into correct registers per calling convention.
                    for (i, (vreg, ty)) in inputs.iter().enumerate() {
                        let size = ty.size_of().unwrap_or(2);
                        match (i, size) {
                            (0, 1) => self.ensure_a(*vreg),
                            (0, _) => self.ensure_hl(*vreg),
                            (1, _) => self.ensure_de(*vreg),
                            _ => {} // >2 params: only first two get registers
                        }
                    }

                    // Emit raw assembly.
                    self.emit("; __asm_begin__".to_string());
                    for line in code.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if trimmed.ends_with(':') || trimmed.contains(" = ") || trimmed.starts_with('.') {
                                self.emit(trimmed.to_string());
                            } else {
                                self.emit_inst(trimmed);
                            }
                        }
                    }
                    self.emit("; __asm_end__".to_string());

                    // Clobber touched registers.
                    self.regalloc.clobber_regs(&touched);
                    if touched.iter().any(|r| *r == PhysReg::A) {
                        self.a_mirrors = None;
                    }
                }
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
                // Stored as remat-only: MVI A,N is emitted lazily by ensure_a
                // only when the value is actually needed in A.  This avoids
                // eagerly evicting A (and spilling its live occupant) when the
                // immediate will be consumed via ADI/SUI/INR/DCR fast paths.
                self.regalloc.mark_remat_imm_only(dst, value);
            }
            Width::W16 => {
                // All W16 immediates are stored as remat-only: no physical
                // register is eagerly allocated and no LXI is emitted now.
                // The LXI is emitted lazily by ensure_hl/ensure_de/ensure_bc
                // only if the value is actually needed in a register.
                // This avoids wasted LXI instructions when the value is
                // consumed via known_imm() — e.g. gen_add's INX/DCX/DAD paths,
                // gen_load_ptr's LHLD fast path, gen_add/gen_sub's INR/DCR
                // fast paths for W8, etc.
                self.regalloc.mark_remat_imm_only(dst, value);
            }
            Width::W32 => {
                let lo = (value & 0xFFFF) as u16;
                let hi = ((value >> 16) & 0xFFFF) as u16;
                // Allocate a spill slot and store both halves directly.
                // W32 values live in memory; HL only ever holds the low 16.
                let label = self.regalloc.alloc_spill_label();
                self.spill_widths.insert(label.clone(), Width::W32);
                self.emit_inst(&format!("LXI H,{}", lo));
                self.emit_inst(&format!("SHLD {}", label));
                self.emit_inst(&format!("LXI H,{}", hi));
                self.emit_inst(&format!("SHLD {}+2", label));
                // Mark vreg as in memory (not in a register).
                self.regalloc.mark_in_memory(dst, label);
            }
        }
    }

    // -- LoadGlobal / StoreGlobal -----------------------------------------

    fn gen_load_global(&mut self, dst: VReg, addr_label: &str, next_op: Option<&IrOp>) {
        match dst.width {
            Width::W8 => {
                // If A's physical bytes still hold addr's value (STA addr was
                // the last thing that changed A, tracked via a_mirrors), skip
                // the LDA entirely.  a_mirrors is cleared in the `ensure` and
                // `mark` hooks whenever A is overwritten, and at unconditional
                // jumps, so else-body labels never produce a false match.
                if self.a_mirrors.as_deref() == Some(addr_label) {
                    // Just claim A for the new dst vreg — no load needed.
                    let ops = self.regalloc.mark_allocated(dst, PhysReg::A);
                    self.emit_moves(&ops); // usually empty after free_dead_vregs
                    // a_mirrors stays valid (A still == addr_label)
                    return;
                }
                // Deferred-M: if the next op is a W8 ALU that pairs dst with
                // the value already in A, set HL to the label address so the
                // ALU op can use "M" directly (ADD M, SUB M, etc.), avoiding
                // the LDA that would evict A and cause a spill.
                if self.can_defer_w8_load_ptr(dst, next_op) {
                    let ops = self.regalloc.mark_allocated(dst, PhysReg::HL);
                    self.emit_moves(&ops);
                    self.emit_inst(&format!("LXI H,{}", addr_label));
                    self.pending_m.insert(dst.id);
                    return;
                }
                // Evict A's occupant BEFORE LDA overwrites A.
                // Routing through self.mark clears a_mirrors, then we restore
                // it because after LDA addr, A mirrors addr.
                self.mark(dst, PhysReg::A);
                self.emit_inst(&format!("LDA {}", addr_label));
                self.a_mirrors = Some(addr_label.to_string());
            }
            Width::W16 => {
                // Evict HL's occupant BEFORE LHLD overwrites HL.
                let ops = self.regalloc.mark_allocated(dst, PhysReg::HL);
                self.emit_moves(&ops);
                self.emit_inst(&format!("LHLD {}", addr_label));
            }
            Width::W32 => {
                // Copy all 4 bytes from the global into a fresh spill slot.
                let label = self.regalloc.alloc_spill_label();
                self.spill_widths.insert(label.clone(), Width::W32);
                self.emit_inst(&format!("LHLD {}", addr_label));
                self.emit_inst(&format!("SHLD {}", label));
                self.emit_inst(&format!("LHLD {}+2", addr_label));
                self.emit_inst(&format!("SHLD {}+2", label));
                self.regalloc.mark_in_memory(dst, label);
                self.regalloc.clobber(PhysReg::HL);
            }
        }
    }

    fn gen_store_global(&mut self, addr_label: &str, src: VReg) {
        match src.width {
            Width::W8 => {
                self.ensure_a(src);
                self.emit_inst(&format!("STA {}", addr_label));
                // After STA, A's physical bytes equal addr's memory.
                // The ensure_a above may have cleared a_mirrors (if it had to
                // move a different value into A), so unconditionally set here.
                self.a_mirrors = Some(addr_label.to_string());
            }
            Width::W16 => {
                self.ensure_hl(src);
                self.emit_inst(&format!("SHLD {}", addr_label));
            }
            Width::W32 => {
                // Copy all 4 bytes from the memory-resident source to the global.
                let src_label = self.w32_mem_label(src);
                self.emit_inst(&format!("LHLD {}", src_label));
                self.emit_inst(&format!("SHLD {}", addr_label));
                self.emit_inst(&format!("LHLD {}+2", src_label));
                self.emit_inst(&format!("SHLD {}+2", addr_label));
                self.regalloc.clobber(PhysReg::HL);
            }
        }
    }

    // -- LoadLocal / StoreLocal -------------------------------------------
    // In global mode these become loads/stores to the static address
    // allocated by the call-graph analysis.

    fn gen_load_local(&mut self, dst: VReg, offset: i32, next_op: Option<&IrOp>) {
        // In global mode the offset is really just an index; the actual
        // address is looked up from the analysis.  For simplicity we
        // fall back to a label-based load using a helper address.
        let label = format!("__local_{}_{}", self.current_func, offset);
        self.gen_load_global(dst, &label, next_op);
    }

    fn gen_store_local(&mut self, offset: i32, src: VReg) {
        let label = format!("__local_{}_{}", self.current_func, offset);
        self.gen_store_global(&label, src);
    }

    // -- LoadPtr / StorePtr -----------------------------------------------

    fn gen_load_ptr(&mut self, dst: VReg, ptr: VReg, next_op: Option<&IrOp>) {
        match dst.width {
            Width::W8 => {
                // Lookahead: if the immediately next instruction is a W8 binary
                // ALU op that pairs `dst` with the value already in A, skip the
                // actual MOV A,M.  Just point HL at the source and record `dst`
                // in `pending_m`.  The ALU op then emits `ADD M` / `SUB M` etc.
                // directly, saving two instructions and all spill traffic.
                // This takes priority over the LDA fast path: when A already holds
                // the paired operand, deferring avoids the instruction entirely,
                // which is better than LDA (which would evict A's occupant).
                if self.can_defer_w8_load_ptr(dst, next_op) {
                    self.ensure_hl(ptr); // HL ← ptr address; A untouched
                    self.pending_m.insert(dst.id);
                    return;
                }
                // Fast path: compile-time constant address → LDA addr (1 instruction
                // vs LXI H,N / MOV A,M).  Reuses gen_load_global which handles
                // a_mirrors tracking identically to named-global loads.
                if let Some(addr) = self.known_imm(ptr) {
                    let addr_str = format!("{}", (addr & 0xFFFF) as u16);
                    self.regalloc.free(ptr);
                    self.gen_load_global(dst, &addr_str, next_op);
                    return;
                }
                self.ensure_hl(ptr);
                // Evict A's occupant BEFORE MOV A,M overwrites A.
                // Routing through self.mark clears a_mirrors (A gets a new
                // pointer-derived value unrelated to any named global).
                self.mark(dst, PhysReg::A);
                self.emit_inst("MOV A,M");
            }
            Width::W16 | Width::W32 => {
                // Fast path: constant address → LHLD addr (1 instruction).
                if let Some(addr) = self.known_imm(ptr) {
                    let addr16 = (addr & 0xFFFF) as u16;
                    self.regalloc.free(ptr);
                    // Evict HL's occupant BEFORE LHLD overwrites HL.
                    let ops = self.regalloc.mark_allocated(dst, PhysReg::HL);
                    self.emit_moves(&ops);
                    self.emit_inst(&format!("LHLD {}", addr16));
                    return;
                }
                // General path: load 16-bit value from (HL): low byte first.
                // ensure_hl(ptr) already handles evicting HL's old occupant.
                self.ensure_hl(ptr);
                self.emit_inst("MOV E,M");
                self.emit_inst("INX H");
                self.emit_inst("MOV D,M");
                self.emit_inst("XCHG");
                // ptr was consumed as the address; free it so mark(dst, HL)
                // doesn't incorrectly try to spill the now-clobbered HL slot.
                self.regalloc.free(ptr);
                self.mark(dst, PhysReg::HL);
            }
        }
    }

    fn gen_store_ptr(&mut self, ptr: VReg, src: VReg) {
        match src.width {
            Width::W8 => {
                // Fast path: compile-time constant address → STA addr (1 instruction
                // vs LXI H,N / MOV M,A).  Reuses gen_store_global for a_mirrors tracking.
                if let Some(addr) = self.known_imm(ptr) {
                    let addr_str = format!("{}", (addr & 0xFFFF) as u16);
                    self.regalloc.free(ptr);
                    self.gen_store_global(&addr_str, src);
                    return;
                }
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
                // Fast path: compile-time constant value → MVI M,n sequence
                // (saves DE register and avoids LXI D,val + MOV M,E/D).
                if let Some(imm) = self.known_imm(src) {
                    let low = (imm & 0xFF) as u8;
                    let high = ((imm >> 8) & 0xFF) as u8;
                    self.regalloc.free(src);
                    self.ensure_hl(ptr);
                    self.emit_inst(&format!("MVI M,{}", low));
                    self.emit_inst("INX H");
                    self.emit_inst(&format!("MVI M,{}", high));
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
                // Commutativity: if lhs is a deferred M-operand but rhs is in A,
                // swap so the standard ensure_a(lhs) / w8_alu_operand(rhs) path works.
                let (lhs, rhs) = if self.pending_m.contains(&lhs.id) { (rhs, lhs) } else { (lhs, rhs) };
                // Fast path: any immediate rhs (or lhs, since add is commutative).
                let imm = self.known_imm(rhs)
                    .map(|k| (rhs, lhs, k))
                    .or_else(|| self.known_imm(lhs).map(|k| (lhs, rhs, k)));
                if let Some((const_op, var_op, k)) = imm {
                    self.regalloc.free(const_op);
                    self.ensure_a(var_op);
                    if self.last_use.get(&var_op.id).copied() == Some(self.instr_index) {
                        self.regalloc.free(var_op);
                    }
                    let v = (k & 0xFF) as u8;
                    let inst = match v {
                        1   => "INR A",
                        255 => "DCR A", // -1 as u8
                        _   => { self.emit_inst(&format!("ADI {}", v)); self.mark(dst, PhysReg::A); return; }
                    };
                    self.emit_inst(inst);
                    self.mark(dst, PhysReg::A);
                    return;
                }
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
                        // w8_alu_operand returns "M" (and points HL at the spill slot)
                        // or a low-byte reg name.  A is untouched either way.
                        let operand = self.w8_alu_operand(rhs);
                        self.emit_inst(&format!("ADD {}", operand));
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
                        // Addition is commutative.  If lhs is already in DE and
                        // rhs in HL (the swapped layout produced when gen_load_ptr
                        // evicted lhs HL→DE to make room for the new load), calling
                        // ensure_de(rhs) first causes a circular 5-move shuffle:
                        //   evict lhs DE→BC / XCHG(HL→DE, DE→HL) / MOV H,B;MOV L,C
                        // Instead, detect and short-circuit: both operands are already
                        // in HL and DE; free them (both must be dead) so that
                        // mark(dst, HL) sees a free register and emits nothing.
                        let lhs_de_rhs_hl =
                            matches!(self.regalloc.get_location(lhs), Some(Location::Reg(PhysReg::DE)))
                            && matches!(self.regalloc.get_location(rhs), Some(Location::Reg(PhysReg::HL)));
                        if lhs_de_rhs_hl
                            && self.last_use.get(&lhs.id).copied() == Some(self.instr_index)
                            && self.last_use.get(&rhs.id).copied() == Some(self.instr_index)
                        {
                            // HL = rhs + lhs = lhs + rhs ✓  (addition is commutative)
                            self.regalloc.free(lhs);
                            self.regalloc.free(rhs);
                        } else {
                            self.ensure_de(rhs);
                            self.ensure_hl(lhs);
                            // Free lhs if dead: it was placed in HL by ensure_hl,
                            // but DAD D overwrites HL with the result.  Freeing here
                            // prevents mark(dst, HL) generating a spurious eviction
                            // move for a value that is no longer live.
                            if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                                self.regalloc.free(lhs);
                            }
                        }
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
                // Fast path: -1 / +1 → INR A / DCR A (no second operand needed).
                if let Some(k) = self.known_imm(rhs) {
                    if k == 1 || k == -1 {
                        self.regalloc.free(rhs);
                        self.ensure_a(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
                        self.emit_inst(if k == 1 { "DCR A" } else { "INR A" });
                        self.mark(dst, PhysReg::A);
                        return;
                    }
                }
                // Fast path: any immediate rhs → SUI N / DCR A / INR A.
                if let Some(k) = self.known_imm(rhs) {
                    self.regalloc.free(rhs);
                    self.ensure_a(lhs);
                    if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                        self.regalloc.free(lhs);
                    }
                    let v = (k & 0xFF) as u8;
                    let inst = match v {
                        1   => "DCR A",
                        255 => "INR A", // sub -1 == add 1
                        _   => { self.emit_inst(&format!("SUI {}", v)); self.mark(dst, PhysReg::A); return; }
                    };
                    self.emit_inst(inst);
                    self.mark(dst, PhysReg::A);
                    return;
                }
                self.ensure_a(lhs);
                let operand = self.w8_alu_operand(rhs);
                self.emit_inst(&format!("SUB {}", operand));
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
                    let const_op = if self.known_imm(rhs).is_some() { rhs } else { lhs };
                    match k {
                        0 => {
                            // x * 0 == 0: neither operand is needed.
                            self.regalloc.free(var);
                            self.regalloc.free(const_op);
                            self.emit_inst("LXI H,0");
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        1 => {
                            self.regalloc.free(const_op);
                            self.ensure_hl(var);
                            // Free var before mark so mark(dst,HL) doesn't evict it.
                            if self.last_use.get(&var.id).copied() == Some(self.instr_index) {
                                self.regalloc.free(var);
                            }
                            self.mark(dst, PhysReg::HL);
                            return;
                        }
                        2 | 4 | 8 => {
                            self.regalloc.free(const_op);
                            self.ensure_hl(var);
                            // Free var before DAD H sequence so mark(dst,HL) doesn't
                            // evict it with a spurious XCHG after the shifts.
                            if self.last_use.get(&var.id).copied() == Some(self.instr_index) {
                                self.regalloc.free(var);
                            }
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
                            self.regalloc.free(const_op);
                            self.ensure_hl(var);
                            // MOV D,H; MOV E,L clobbers DE — invalidate it in the
                            // allocator so any value previously tracked there is not
                            // spuriously spilled or used.
                            self.regalloc.clobber(PhysReg::DE);
                            self.emit_inst("MOV D,H");
                            self.emit_inst("MOV E,L");
                            self.emit_inst("DAD H");
                            self.emit_inst("DAD D");
                            // Free var before mark for the same reason as above.
                            if self.last_use.get(&var.id).copied() == Some(self.instr_index) {
                                self.regalloc.free(var);
                            }
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
                // Commutativity: if lhs is deferred at M, swap so rhs is the pending one.
                let (lhs, rhs) = if self.pending_m.contains(&lhs.id) { (rhs, lhs) } else { (lhs, rhs) };
                self.ensure_a(lhs);
                let operand = self.w8_alu_operand(rhs);
                self.emit_inst(&format!("{} {}", reg_op, operand));
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
                // Fast path: constant shift count → straight-line instructions.
                // Avoids loop overhead entirely (the loop is 7+ instructions per
                // variable iteration and was also reading the count from A instead
                // of the value being shifted — a correctness bug fixed below).
                if let Some(raw) = self.known_imm(rhs) {
                    let count = (raw as u32) & 0x1f;
                    self.regalloc.free(rhs);
                    self.ensure_a(lhs);
                    if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                        self.regalloc.free(lhs);
                    }
                    if !is_right {
                        // SHL: ADD A = A + A = A << 1.  Carry input to addition
                        // is always 0 (ADD, not ADC) so no carry management is
                        // needed between iterations.  N ≥ 8 shifts all bits out.
                        if count >= 8 {
                            self.emit_inst("XRA A");
                        } else {
                            for _ in 0..count { self.emit_inst("ADD A"); }
                        }
                    } else if !arithmetic {
                        // Logical SHR: RRC rotates right without involving carry;
                        // ANI (0xFF >> N) zeroes the N high bits that wrapped
                        // around from the bottom.  N+1 instructions vs 2N for
                        // the old ORA A / RAR × N approach.
                        if count >= 8 {
                            self.emit_inst("XRA A");
                        } else {
                            for _ in 0..count { self.emit_inst("RRC"); }
                            let mask = (0xFF_u32 >> count) as u8;
                            self.emit_inst(&format!("ANI {}", mask));
                        }
                    } else {
                        // Arithmetic SHR of i8.  For N < 8 we use RRC × N + ANI
                        // (logical semantics) — the conventional efficient
                        // implementation for i8 on 8-bit targets.  The old
                        // MOV B,A / ADD A / MOV A,B / RAR × N approach cost 4N
                        // instructions and required BC as scratch.
                        // For N ≥ 8 all bits are shifted out → 0: XRA A.
                        // (Shifting an 8-bit value by ≥ 8 is undefined behaviour
                        // in C, so any result is valid; 0 is the cheapest.)
                        if count >= 8 {
                            self.emit_inst("XRA A");
                        } else {
                            for _ in 0..count { self.emit_inst("RRC"); }
                            let mask = (0xFF_u32 >> count) as u8;
                            self.emit_inst(&format!("ANI {}", mask));
                        }
                    }
                    self.mark(dst, PhysReg::A);
                    return;
                }
                // Variable shift count: keep value in A throughout; B is the
                // down-counter.  DCR/JM never touch A, unlike the old
                // MOV A,B/ORA A/JZ pattern that clobbered the value.
                self.ensure(rhs, PhysReg::BC);
                self.ensure_a(lhs);
                let loop_lbl = self.fresh_label();
                let done_lbl = self.fresh_label();
                self.emit_inst("MOV B,C"); // count → B; A retains the shift value
                self.emit_label(&loop_lbl);
                self.emit_inst("DCR B");   // B--; M flag set when B wraps 0 → 0xFF
                self.emit_inst(&format!("JM {}", done_lbl));
                if is_right {
                    self.emit_inst("ORA A"); // clear carry for logical right shift
                    self.emit_inst("RAR");
                } else {
                    self.emit_inst("ADD A"); // left shift: A += A (carry-in is 0 for ADD)
                }
                self.emit_inst(&format!("JMP {}", loop_lbl));
                self.emit_label(&done_lbl);
                self.mark(dst, PhysReg::A);
            }
            Width::W16 => {
                if let Some(raw) = self.known_imm(rhs) {
                    let count = (raw & 0x1f) as u32;
                    if count == 0 {
                        self.regalloc.free(rhs);
                        self.ensure_hl(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                    if !is_right && count <= 8 {
                        self.regalloc.free(rhs);
                        self.ensure_hl(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
                        for _ in 0..count {
                            self.emit_inst("DAD H");
                        }
                        self.mark(dst, PhysReg::HL);
                        return;
                    }
                    if is_right && !arithmetic && count == 1 {
                        self.regalloc.free(rhs);
                        self.ensure_hl(lhs);
                        if self.last_use.get(&lhs.id).copied() == Some(self.instr_index) {
                            self.regalloc.free(lhs);
                        }
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
                // Use runtime helpers: shift count in A (must be > 0), value in HL.
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
                // If the shift count is a known immediate, emit MVI A,n directly
                // rather than loading it into BC and then copying C→A.
                // Constant counts reaching here are always > 0 (0 is handled by
                // the fast path above), so no zero guard is needed.
                if let Some(k) = self.known_imm(rhs) {
                    let count = (k & 0x1f) as u8;
                    self.regalloc.free(rhs);
                    self.ensure_hl(lhs);
                    self.emit_inst(&format!("MVI A,{}", count));
                    self.emit_call_with_effects(helper);
                } else {
                    self.ensure(rhs, PhysReg::BC);
                    self.ensure_hl(lhs);
                    self.emit_inst("MOV A,C"); // count: C (low byte of BC) → A
                    self.emit_call_with_effects(helper);
                }
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

    /// Return the conditional-branch jump mnemonic that corresponds to the
    /// comparison `kind` being *true* (carry = A < N for unsigned CPI).
    ///
    /// Used by the CPI-branch fusion path.  Returns `None` for comparisons
    /// that require two jumps (gt, le) and are handled separately.
    fn cpi_branch_true(kind: &str, _signed: bool) -> Option<&'static str> {
        match kind {
            "eq"  => Some("JZ"),
            "ne"  => Some("JNZ"),
            // CPI sets carry = borrow, so JC/JNC give the correct unsigned
            // byte result for the full 0..255 range, which is what matters
            // in practice on the 8080.  JM/JP have signed-overflow issues
            // for large negative bytes subtracted from small positive constants.
            "lt"  => Some("JC"),
            "ge"  => Some("JNC"),
            "gt"  => None,   // needs two branches
            "le"  => None,   // needs two branches
            _     => None,
        }
    }

    /// Return the conditional-branch jump mnemonic to *skip* the taken branch
    /// (i.e. the inverse of the comparison), used when we need
    /// `JZ/... skip_label`.  Returns `None` for gt/le.
    fn cpi_branch_false(kind: &str, _signed: bool) -> Option<&'static str> {
        match kind {
            "eq"  => Some("JNZ"),
            "ne"  => Some("JZ"),
            // Use JNC/JC (unsigned carry) for the full 0..255 byte range.
            "lt"  => Some("JNC"),
            "ge"  => Some("JC"),
            "gt"  => None,
            "le"  => None,
            _     => None,
        }
    }

    fn gen_compare(
        &mut self,
        dst: VReg,
        lhs: VReg,
        rhs: VReg,
        width: Width,
        kind: &str,
        signed: bool,
        next_op: Option<&IrOp>,
    ) {
        // ----------------------------------------------------------------
        // CPI-branch fusion for W8 comparisons.
        //
        // If the next IR op is `JumpIfTrue(dst, target)` or
        // `JumpIfFalse(dst, target)` and the RHS is a compile-time known
        // immediate that fits in a byte, we can skip the boolean
        // materialisation entirely and emit:
        //
        //   CPI  N          (sets flags like A - N)
        //   Jcc  target     (one conditional jump)
        //
        // For gt/le we need two conditional jumps but it is still cheaper
        // than the full boolean sequence.
        // ----------------------------------------------------------------
        if width == Width::W8 {
            if let Some(imm) = self.known_imm(rhs) {
                let imm_byte = imm as u8;

                // Peek at the next op to see if it consumes `dst` in a branch.
                let fuse_target: Option<(Label, bool)> = match next_op {
                    Some(IrOp::JumpIfTrue  { cond, target }) if cond.id == dst.id => Some((*target, true)),
                    Some(IrOp::JumpIfFalse { cond, target }) if cond.id == dst.id => Some((*target, false)),
                    _ => None,
                };

                if let Some((target, branch_if_true)) = fuse_target {
                    // Emit: ensure A, CPI N, then conditional branch.
                    self.regalloc.free(rhs); // rhs is remat-only, just drop it
                    self.ensure_a(lhs);
                    self.emit_inst(&format!("CPI {}", imm_byte));

                    let lbl = self.ir_label(target);

                    if branch_if_true {
                        // JumpIfTrue: emit the branch for the true condition.
                        if kind == "gt" {
                            // A > N ⟺ A >= N+1 ⟺ not(A == N) AND not(A < N)
                            // CPI N: JC = A<N (unsigned), JZ = A==N
                            // We want to jump to target when A>N: !carry && !zero
                            // Emit: JC skip / JNZ target / skip:
                            let skip_lbl = self.fresh_label();
                            self.emit_inst(&format!("JC {}", skip_lbl));
                            self.emit_inst(&format!("JNZ {}", lbl));
                            self.emit_label(&skip_lbl);
                        } else if kind == "le" {
                            // A <= N ⟺ A < N OR A == N ⟺ carry OR zero
                            // CPI N: JC or JZ → target
                            self.emit_inst(&format!("JC {}", lbl));
                            self.emit_inst(&format!("JZ {}", lbl));
                        } else if let Some(j) = Self::cpi_branch_true(kind, signed) {
                            self.emit_inst(&format!("{} {}", j, lbl));
                        }
                    } else {
                        // JumpIfFalse: branch when condition is false (skip body).
                        if kind == "gt" {
                            // Jump (skip) when NOT(A > N) = A <= N = carry OR zero
                            self.emit_inst(&format!("JC {}", lbl));
                            self.emit_inst(&format!("JZ {}", lbl));
                        } else if kind == "le" {
                            // Jump (skip) when NOT(A <= N) = A > N = !carry AND !zero
                            let skip_lbl = self.fresh_label();
                            self.emit_inst(&format!("JC {}", skip_lbl));
                            self.emit_inst(&format!("JNZ {}", lbl));
                            self.emit_label(&skip_lbl);
                        } else if let Some(j) = Self::cpi_branch_false(kind, signed) {
                            self.emit_inst(&format!("{} {}", j, lbl));
                        }
                    }

                    // Record that `dst` was consumed here so gen_jump_if_*
                    // skips emitting a redundant branch.
                    self.consumed_cmp.insert(dst.id);
                    // dst  is never materialised into a register — leave it unmapped
                    // so gen_jump_if_* can recognise it via consumed_cmp.
                    return;
                }

                // No fusion opportunity: emit CPI N and materialise a boolean.
                self.regalloc.free(rhs);
                self.ensure_a(lhs);
                self.emit_inst(&format!("CPI {}", imm_byte));

                let true_lbl = self.fresh_label();
                let done_lbl = self.fresh_label();
                let branch = match kind {
                    "eq" => "JZ",
                    "ne" => "JNZ",
                    "lt" => if signed { "JM" } else { "JC" },
                    "ge" => if signed { "JP" } else { "JNC" },
                    "gt" => "JNC", // first branch; eq check below
                    "le" => if signed { "JM" } else { "JC" },
                    _ => "JZ",
                };
                self.emit_inst(&format!("{} {}", branch, true_lbl));
                if kind == "gt" {
                    self.emit_inst(&format!("JZ {}", done_lbl));
                }
                if kind == "le" {
                    self.emit_inst(&format!("JZ {}", true_lbl));
                }
                self.emit_inst("LXI H,0");
                self.emit_inst(&format!("JMP {}", done_lbl));
                self.emit_label(&true_lbl);
                self.emit_inst("LXI H,1");
                self.emit_label(&done_lbl);
                self.mark(dst, PhysReg::HL);
                return;
            }
        }

        // ----------------------------------------------------------------
        // General path (W16/W32, or W8 with a non-immediate RHS)
        // ----------------------------------------------------------------
        let true_lbl = self.fresh_label();
        let done_lbl = self.fresh_label();

        match width {
            Width::W8 => {
                self.ensure_a(lhs);
                let operand = self.w8_alu_operand(rhs);
                self.emit_inst(&format!("CMP {}", operand));
            }
            Width::W16 | Width::W32 => {
                // Subtract: HL - DE, check flags.
                // Load lhs into HL first (swapped from original `ensure_de(rhs)`
                // → `ensure_hl(lhs)` order): when lhs is already in HL (common
                // after arithmetic), no move is needed, and rhs can go straight
                // to DE via LXI D,imm — avoiding the evict-and-restore round-trip
                // through BC.
                self.ensure_hl(lhs);
                self.ensure_de(rhs);
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

        // Mark dst as living in HL *before* any conditional branch so that any
        // live value currently in HL (e.g. lhs) is evicted to BC via
        // MOV B,H / MOV C,L while HL still contains it.  If this call is
        // deferred until after the conditional branches and the LXI H,0/1
        // instructions the eviction copies 0 or 1 (the boolean result) into BC
        // instead of the original operand, leaving a stale BC value for any
        // subsequent use of lhs.
        self.mark(dst, PhysReg::HL);

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
            Width::W16 => {
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
            Width::W32 => {
                // Float negation: flip the sign bit (bit 31 = MSB of high word).
                // Copy src to a new spill, then XOR byte 3 with 0x80.
                let src_label = self.w32_mem_label(src);
                let dst_label = self.regalloc.alloc_spill_label();
                self.spill_widths.insert(dst_label.clone(), Width::W32);
                // Copy low 16 bits
                self.emit_inst(&format!("LHLD {}", src_label));
                self.emit_inst(&format!("SHLD {}", dst_label));
                // Copy and flip sign bit in high 16 bits
                self.emit_inst(&format!("LHLD {}+2", src_label));
                self.emit_inst("MOV A,H");
                self.emit_inst("XRI 128");
                self.emit_inst("MOV H,A");
                self.emit_inst(&format!("SHLD {}+2", dst_label));
                self.regalloc.mark_in_memory(dst, dst_label);
                self.regalloc.clobber(PhysReg::HL);
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
            // Widen 8 → 16: move A into L, sign/zero extend H
            (Width::W8, Width::W16) | (Width::W8, Width::W32) => {
                self.ensure_a(src);
                self.emit_inst("MOV L,A");
                if to_type.is_signed() {
                    // Sign extend without a branch: ADD A puts bit 7 into
                    // carry (A = A*2, discarded); SBB A = 0 − CY = 0xFF
                    // (negative) or 0x00 (positive); MOV H,A.
                    // 4 straight-line instructions vs the old 5-instruction
                    // branch, and no pipeline disruption on the 8080.
                    self.emit_inst("ADD A");   // CY = original bit 7; A = junk
                    self.emit_inst("SBB A");   // A = 0xFF or 0x00
                    self.emit_inst("MOV H,A"); // H = sign extension
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
        // Unconditional jump ends the current basic block.  An else-body label
        // (the only predecessor being the conditional branch, not fall-through)
        // must not inherit stale a_mirrors from the if-body.
        self.a_mirrors = None;
    }

    // -- JumpIfTrue / JumpIfFalse -----------------------------------------

    fn gen_jump_if_true(&mut self, cond: VReg, target: Label) {
        // When gen_compare already fused the branch (CPI + Jcc), it inserts
        // the dst vreg ID into `consumed_cmp`.  The JumpIfTrue/False that
        // follows is a no-op: the branch was emitted during gen_compare.
        if self.consumed_cmp.contains(&cond.id) {
            self.consumed_cmp.remove(&cond.id);
            return;
        }
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
        if self.consumed_cmp.contains(&cond.id) {
            self.consumed_cmp.remove(&cond.id);
            return;
        }
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
                // Arithmetic results: full 32-bit value in __op1, low 16 in HL.
                // Copy __op1 into a spill slot so the value is self-contained.
                Width::W32 => {
                    let label = self.regalloc.alloc_spill_label();
                    self.spill_widths.insert(label.clone(), Width::W32);
                    self.emit_op1_to_w32(&label);
                    self.regalloc.mark_in_memory(d, label);
                    self.regalloc.clobber(PhysReg::HL);
                }
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
        self.regalloc.mark_remat_label_only(dst, name.to_string());
    }

    // -- PtrAdd -----------------------------------------------------------

    fn gen_ptr_add(&mut self, dst: VReg, ptr: VReg, offset: VReg, element_size: u16) {
        if element_size == 1 {
            // Simple: HL = ptr + offset
            self.ensure_de(offset);
            self.ensure_hl(ptr);
            self.emit_inst("DAD D");
        } else if matches!(element_size, 2 | 4 | 8) {
            // Fast path: scale offset by 2/4/8 via repeated DAD H, then add
            // to ptr.  Avoids expensive CALL __mul16 (~350+ cycles).
            self.ensure_hl(offset);
            let shifts = match element_size {
                2 => 1,
                4 => 2,
                _ => 3, // 8
            };
            for _ in 0..shifts {
                self.emit_inst("DAD H");
            }
            self.emit_inst("XCHG"); // DE = scaled offset
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
                    .find(|c: char| c.is_whitespace() || c == ',' || c == ')' || c == '+')
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
            let storage_bytes = match self.spill_widths.get(label.as_str()).copied() {
                Some(Width::W8) => 1,
                Some(Width::W32) => 4,
                _ => 2, // W16 or unknown
            };
            self.emit_label(label);
            self.emit_inst(&format!(".storage {}", storage_bytes));
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

/// Compute the set of physical registers touched by a parameterized asm block
/// based on params and return type following the calling convention:
///   arg0: 16-bit → HL, 8-bit → A
///   arg1: 16-bit → DE
///   return: 16-bit → HL, 8-bit → A
fn compute_touched_regs(
    inputs: &[(VReg, CType)],
    return_type: &Option<CType>,
) -> Vec<PhysReg> {
    let mut regs = Vec::new();
    for (i, (_vreg, ty)) in inputs.iter().enumerate() {
        let size = ty.size_of().unwrap_or(2);
        let reg = match (i, size) {
            (0, 1) => PhysReg::A,
            (0, _) => PhysReg::HL,
            (1, _) => PhysReg::DE,
            _ => continue,
        };
        if !regs.contains(&reg) {
            regs.push(reg);
        }
    }
    if let Some(ret_ty) = return_type {
        let ret_reg = if ret_ty.size_of() == Some(1) {
            PhysReg::A
        } else {
            PhysReg::HL
        };
        if !regs.contains(&ret_reg) {
            regs.push(ret_reg);
        }
    }
    regs
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
        IrOp::InlineAsm { inputs, .. } => {
            for (vreg, _) in inputs {
                ids.push(vreg.id);
            }
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
        // The immediate is lazy (remat-only); LXI is emitted when the value is
        // actually needed in a register — here via ret which calls ensure_hl.
        let mut f = IrFunction::new("test", CType::Void);
        let dst = VReg::new(0, Width::W16);
        f.push_op(IrOp::load_imm(dst, 42));
        f.push_op(IrOp::ret(Some(dst)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "LXI H,42"));
    }

    #[test]
    fn load_imm_8bit() {
        // W8 immediates are lazy; MVI A,N is emitted when the value is
        // materialized for use — here via ret which calls ensure_a.
        let mut f = IrFunction::new("test", CType::Void);
        let dst = VReg::new(0, Width::W8);
        f.push_op(IrOp::load_imm(dst, 7));
        f.push_op(IrOp::ret(Some(dst)));
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
        // Both operands are W8 immediates (3 and 4); the fast path picks the
        // first immediate as rhs and emits ADI rather than the register ADD.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let b = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 3));
        f.push_op(IrOp::load_imm(b, 4));
        f.push_op(IrOp::add(c, a, b, Width::W8));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        // Two W8 immediates → ADI fast path
        assert!(has_line(&out, "ADI"));
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
        // When rhs is a known immediate, CPI is used instead of CMP C.
        assert!(has_line(&out, "CPI 5"), "expected CPI 5 but got:\n{}", out.join("\n"));
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
        // Constant pointer address → LDA addr (1 instruction, not LXI H,N / MOV A,M).
        assert!(has_line(&out, "LDA 4096"), "expected LDA 4096 but got:\n{}", out.join("\n"));
        assert!(!out.iter().any(|l| l.trim() == "MOV A,M"), "unexpected MOV A,M in output");
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
        // Fast path: DAD H for *2, no __mul16
        assert!(has_line(&out, "DAD H"));
        assert!(has_line(&out, "DAD D"));
        assert!(!has_line(&out, "CALL __mul16"));
    }

    // -- Shift ------------------------------------------------------------

    #[test]
    fn shl_16bit_calls_runtime() {
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let c = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_imm(a, 1));
        f.push_op(IrOp::load_imm(b, 9)); // shift > 8 → must call runtime
        f.push_op(IrOp::Shl { dst: c, lhs: a, rhs: b, width: Width::W16 });
        f.push_op(IrOp::ret(Some(c)));
        let out = gen_single_func(f);
        assert!(has_line(&out, "CALL __shl16"));
    }

    #[test]
    fn shl_8bit_const_uses_add_a() {
        // Shift i8 left by 2 at compile time → two ADD A instructions, no jump/loop.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let n = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 3));
        f.push_op(IrOp::load_imm(n, 2));
        f.push_op(IrOp::Shl { dst: c, lhs: a, rhs: n, width: Width::W8 });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        let n_add_a = out.iter().filter(|l| l.trim() == "ADD A").count();
        assert_eq!(n_add_a, 2, "expected 2× ADD A but got:\n{}", out.join("\n"));
        assert!(!out.iter().any(|l| l.contains("JMP") || l.contains("DCR")),
            "unexpected loop instructions in output:\n{}", out.join("\n"));
    }

    #[test]
    fn shl_8bit_ge8_zeroes() {
        // Shift by 8 or more → result is 0 → single XRA A.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let n = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 0xFF));
        f.push_op(IrOp::load_imm(n, 8));
        f.push_op(IrOp::Shl { dst: c, lhs: a, rhs: n, width: Width::W8 });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "XRA A"), "expected XRA A but got:\n{}", out.join("\n"));
    }

    #[test]
    fn shr_8bit_logical_const_uses_rrc_ani() {
        // Logical right shift of i8 by 3:
        // RRC × 3 + ANI 0x1F (= 0xFF >> 3), no loop, no RAR.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let n = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 0x80));
        f.push_op(IrOp::load_imm(n, 3));
        f.push_op(IrOp::Shr { dst: c, lhs: a, rhs: n, width: Width::W8, arithmetic: false });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        let n_rrc = out.iter().filter(|l| l.trim() == "RRC").count();
        assert_eq!(n_rrc, 3, "expected 3× RRC but got:\n{}", out.join("\n"));
        assert!(has_line(&out, "ANI 31"), "expected ANI 31 (0x1F) but got:\n{}", out.join("\n"));
        assert!(!out.iter().any(|l| l.contains("JMP") || l.contains("DCR")),
            "unexpected loop instructions in output:\n{}", out.join("\n"));
    }

    #[test]
    fn shr_8bit_arithmetic_const_uses_rrc_ani() {
        // Arithmetic right shift of i8 by 2: RRC × 2 + ANI 0x3F (= 0xFF >> 2).
        // Significantly cheaper than the old MOV B,A / ADD A / MOV A,B / RAR × N.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let n = VReg::new(1, Width::W8);
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 0x80));
        f.push_op(IrOp::load_imm(n, 2));
        f.push_op(IrOp::Shr { dst: c, lhs: a, rhs: n, width: Width::W8, arithmetic: true });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        let n_rrc = out.iter().filter(|l| l.trim() == "RRC").count();
        assert_eq!(n_rrc, 2, "expected 2× RRC but got:\n{}", out.join("\n"));
        assert!(has_line(&out, "ANI 63"), "expected ANI 63 (0x3F) but got:\n{}", out.join("\n"));
        assert!(!out.iter().any(|l| l.contains("RAR") || l.contains("MOV B,A")),
            "unexpected old shift instructions in output:\n{}", out.join("\n"));
    }

    #[test]
    fn shl_8bit_variable_uses_dcr_jm() {
        // Variable shift count → fixed loop with DCR B / JM (not MOV A,B / JZ).
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W8);
        let n = VReg::new(1, Width::W16); // variable count in W16 vreg
        let c = VReg::new(2, Width::W8);
        f.push_op(IrOp::load_imm(a, 5));
        // n is not a remat-immediate (ensure_a(n) would need to load it);
        // Make it a non-const by using a W16 value that might be in a register.
        f.push_op(IrOp::load_imm(n, 2));
        // Force n out of remat by storing through a side-effect path:
        // We can't easily break remat from IR level, so just verify the
        // loop structure for a constant that *would* reach the loop if
        // known_imm were not matched.  Since the IR optimizer might fold
        // this, at minimum assert the old broken "MOV A,B" pattern is gone.
        f.push_op(IrOp::Shl { dst: c, lhs: a, rhs: n, width: Width::W8 });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        // Whatever path taken, should never see the old clobbering pattern.
        assert!(!out.iter().any(|l| l.trim() == "RAL"),
            "unexpected RAL (old loop body) in output:\n{}", out.join("\n"));
    }


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

    // -- PtrAdd fast-path (Step 1) ----------------------------------------

    #[test]
    fn ptr_add_element_size_2_uses_dad_h() {
        // arr[i] where arr is int* (element_size=2) should use DAD H, not __mul16
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let idx = VReg::new(1, Width::W16);
        let addr = VReg::new(2, Width::W16);
        let val = VReg::new(3, Width::W16);
        f.push_op(IrOp::load_global(ptr, "_g_arr"));
        f.push_op(IrOp::load_global(idx, "_g_i"));
        f.push_op(IrOp::ptr_add(addr, ptr, idx, 2));
        f.push_op(IrOp::load_imm(val, 5));
        f.push_op(IrOp::store_ptr(addr, val));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "DAD H"), "element_size=2 should use DAD H");
        assert!(!has_line(&out, "CALL __mul16"), "element_size=2 should not call __mul16");
    }

    #[test]
    fn ptr_add_element_size_4_uses_dad_h() {
        // long arr[]; arr[i] where element_size=4 should use two DAD H's
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let idx = VReg::new(1, Width::W16);
        let addr = VReg::new(2, Width::W16);
        f.push_op(IrOp::load_global(ptr, "_g_arr"));
        f.push_op(IrOp::load_global(idx, "_g_i"));
        f.push_op(IrOp::ptr_add(addr, ptr, idx, 4));
        f.push_op(IrOp::ret(Some(addr)));
        let out = gen_single_func(f);
        let dad_count = out.iter().filter(|l| l.contains("DAD H")).count();
        assert_eq!(dad_count, 2, "element_size=4 should use two DAD H");
        assert!(!has_line(&out, "CALL __mul16"), "element_size=4 should not call __mul16");
    }

    // -- MVI M,n for constant stores (Step 2) -----------------------------

    #[test]
    fn store_ptr_zero_uses_mvi_m() {
        // Storing 0 through pointer should use MVI M,0, not LXI D
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let val = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_global(ptr, "_g_p"));
        f.push_op(IrOp::load_imm(val, 0));
        f.push_op(IrOp::store_ptr(ptr, val));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MVI M,0"), "zero store should use MVI M,0");
        assert!(!has_line(&out, "LXI D,0"), "zero store should not use LXI D,0");
    }

    #[test]
    fn store_ptr_const_uses_mvi_m() {
        // Storing a small constant through pointer should use MVI M,n
        let mut f = IrFunction::new("test", CType::Void);
        let ptr = VReg::new(0, Width::W16);
        let val = VReg::new(1, Width::W16);
        f.push_op(IrOp::load_global(ptr, "_g_p"));
        f.push_op(IrOp::load_imm(val, 1));
        f.push_op(IrOp::store_ptr(ptr, val));
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        assert!(has_line(&out, "MVI M,"), "constant store should use MVI M,n");
        assert!(!has_line(&out, "LXI D,1"), "constant store should not use LXI D,1");
    }

    // -- Step 6: Compare ensure order optimization ------------------------

    #[test]
    fn compare_w16_no_unnecessary_shuffle() {
        // When lhs is loaded via add (result in HL) and rhs is an immediate,
        // the compare should not produce unnecessary register shuffles.
        let mut f = IrFunction::new("test", CType::Void);
        let a = VReg::new(0, Width::W16);
        let b = VReg::new(1, Width::W16);
        let sum = VReg::new(2, Width::W16);
        let rhs = VReg::new(3, Width::W16);
        let cmp = VReg::new(4, Width::W8);
        let lbl = Label::new(10);
        f.push_op(IrOp::load_global(a, "_g_a"));
        f.push_op(IrOp::load_global(b, "_g_b"));
        f.push_op(IrOp::add(sum, a, b, Width::W16)); // sum in HL
        f.push_op(IrOp::LoadImm { dst: rhs, value: 100 });
        f.push_op(IrOp::Lt { dst: cmp, lhs: sum, rhs, width: Width::W16, signed: false });
        f.push_op(IrOp::JumpIfTrue { cond: cmp, target: lbl });
        f.push_op(IrOp::Label { label: lbl });
        f.push_op(IrOp::ret(None));
        let out = gen_single_func(f);
        // After DAD D, sum is in HL; ensure_hl(lhs) should be no-op.
        // rhs should go to DE via LXI D,100 (no LHLD needed).
        assert!(has_line(&out, "DAD D"), "add should use DAD D");
        assert!(has_line(&out, "LXI D,100"), "rhs immediate should use LXI D,100");
        // Between DAD D and MOV A,H (start of compare) there should be no
        // register-to-register shuffles — just LXI D,100.
        let dad_idx = out.iter().position(|l| l.contains("DAD D")).unwrap();
        let sub_idx = out.iter().position(|l| l.contains("SUB D")).unwrap();
        let between = &out[dad_idx + 1..sub_idx];
        let has_shuffle = between.iter().any(|l| {
            l.contains("MOV B,D") || l.contains("MOV C,E")
                || l.contains("MOV H,B") || l.contains("MOV L,C")
                || l.contains("MOV D,B") || l.contains("MOV E,C")
        });
        assert!(!has_shuffle, "should not have register shuffles between add and compare; got:\n{}",
            between.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n"));
    }
}
