//! IR optimization passes for the v6c compiler (Phase 2).
//!
//! This module implements:
//!
//! * **Constant folding** — evaluate binary/unary ops on constants at compile time.
//! * **Constant propagation** — track which virtual registers hold known constants
//!   and substitute them into downstream instructions.
//! * **Dead-code elimination** — remove unreachable instructions after unconditional
//!   jumps and remove stores whose results are never used.
//! * **Strength reduction** — replace multiply/divide/modulo by powers of two with
//!   shift/mask operations.
//! * **Common sub-expression elimination (CSE)** — within basic blocks, detect and
//!   reuse identical computations.
//!
//! The public entry point is [`optimize`], which takes an [`IrProgram`] and returns
//! an optimized copy.

use std::collections::{HashMap, HashSet};

use crate::ir::{IrFunction, IrInstr, IrOp, IrProgram, VReg, Width};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Optimize an entire IR program in-place.
pub fn optimize(program: &mut IrProgram) {
    for func in &mut program.functions {
        optimize_function(func);
    }
}

/// Run all optimization passes on a single function.
fn optimize_function(func: &mut IrFunction) {
    // Run passes in a fixed-point loop until no more changes.
    loop {
        let mut changed = false;
        changed |= constant_fold_and_propagate(func);
        changed |= strength_reduce(func);
        changed |= dead_code_eliminate(func);
        changed |= cse(func);
        if !changed {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Constant folding & propagation
// ---------------------------------------------------------------------------

/// Track known constant values for virtual registers and fold operations.
fn constant_fold_and_propagate(func: &mut IrFunction) -> bool {
    let mut changed = false;
    // Map from VReg id → known constant value
    let mut constants: HashMap<u32, i64> = HashMap::new();

    let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());

    for instr in &func.body {
        match &instr.op {
            IrOp::LoadImm { dst, value } => {
                constants.insert(dst.id, *value);
                new_body.push(instr.clone());
            }

            IrOp::Copy { dst, src } => {
                if let Some(&val) = constants.get(&src.id) {
                    constants.insert(dst.id, val);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: val },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Binary arithmetic — try to fold if both operands are constants
            IrOp::Add { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a.wrapping_add(b), *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Sub { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a.wrapping_sub(b), *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Mul { dst, lhs, rhs, width, .. } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a.wrapping_mul(b), *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Div { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    if b != 0 {
                        let result = if *signed {
                            wrap_result(a.wrapping_div(b), *width)
                        } else {
                            let ua = to_unsigned(a, *width);
                            let ub = to_unsigned(b, *width);
                            wrap_result(ua.wrapping_div(ub) as i64, *width)
                        };
                        constants.insert(dst.id, result);
                        new_body.push(IrInstr {
                            op: IrOp::LoadImm { dst: *dst, value: result },
                            line: instr.line,
                        });
                        changed = true;
                    } else {
                        constants.remove(&dst.id);
                        new_body.push(instr.clone());
                    }
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Mod { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    if b != 0 {
                        let result = if *signed {
                            wrap_result(a.wrapping_rem(b), *width)
                        } else {
                            let ua = to_unsigned(a, *width);
                            let ub = to_unsigned(b, *width);
                            wrap_result((ua % ub) as i64, *width)
                        };
                        constants.insert(dst.id, result);
                        new_body.push(IrInstr {
                            op: IrOp::LoadImm { dst: *dst, value: result },
                            line: instr.line,
                        });
                        changed = true;
                    } else {
                        constants.remove(&dst.id);
                        new_body.push(instr.clone());
                    }
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Bitwise ops
            IrOp::And { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a & b, *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Or { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a | b, *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Xor { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a ^ b, *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Shift ops
            IrOp::Shl { dst, lhs, rhs, width } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = wrap_result(a.wrapping_shl(b as u32), *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Shr { dst, lhs, rhs, width, arithmetic } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = if *arithmetic {
                        // Arithmetic (signed) shift right
                        let signed_val = sign_extend(a, *width);
                        wrap_result(signed_val >> (b as u32), *width)
                    } else {
                        let unsigned_val = to_unsigned(a, *width);
                        wrap_result((unsigned_val >> (b as u32)) as i64, *width)
                    };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Comparison ops — fold to 0 or 1
            IrOp::Eq { dst, lhs, rhs, .. } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = if a == b { 1i64 } else { 0 };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Ne { dst, lhs, rhs, .. } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = if a != b { 1i64 } else { 0 };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Lt { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    let result = fold_cmp_lt(a, b, *width, *signed);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Le { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    // a <= b  is  !(b < a)
                    let result = if fold_cmp_lt(b, a, *width, *signed) == 1 { 0i64 } else { 1 };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Gt { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    // a > b  is  b < a
                    let result = fold_cmp_lt(b, a, *width, *signed);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Ge { dst, lhs, rhs, width, signed } => {
                if let (Some(&a), Some(&b)) = (constants.get(&lhs.id), constants.get(&rhs.id)) {
                    // a >= b  is  !(a < b)
                    let result = if fold_cmp_lt(a, b, *width, *signed) == 1 { 0i64 } else { 1 };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Unary ops
            IrOp::Neg { dst, src, width } => {
                if let Some(&a) = constants.get(&src.id) {
                    let result = wrap_result(a.wrapping_neg(), *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::Not { dst, src, width } => {
                if let Some(&a) = constants.get(&src.id) {
                    let result = wrap_result(!a, *width);
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }
            IrOp::LogicalNot { dst, src, .. } => {
                if let Some(&a) = constants.get(&src.id) {
                    let result = if a == 0 { 1i64 } else { 0 };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Labels invalidate nothing, but mark basic-block boundaries for
            // safety — we clear the constant map at labels since they can be
            // reached from multiple predecessors.
            IrOp::Label { .. } => {
                constants.clear();
                new_body.push(instr.clone());
            }

            // Control flow instructions invalidate constants.
            IrOp::Jump { .. } | IrOp::JumpIfTrue { .. } | IrOp::JumpIfFalse { .. } => {
                new_body.push(instr.clone());
            }

            // Calls may clobber everything.
            IrOp::Call { dst, .. } => {
                if let Some(d) = dst {
                    constants.remove(&d.id);
                }
                new_body.push(instr.clone());
            }

            // Stores/loads from globals — the destination vreg gets an unknown value.
            IrOp::LoadGlobal { dst, .. }
            | IrOp::LoadLocal { dst, .. }
            | IrOp::LoadPtr { dst, .. }
            | IrOp::AddrOfGlobal { dst, .. }
            | IrOp::PtrAdd { dst, .. } => {
                constants.remove(&dst.id);
                new_body.push(instr.clone());
            }

            IrOp::Cast { dst, src, to_type } => {
                if let Some(&val) = constants.get(&src.id) {
                    let dst_w = dst.width;
                    let src_w = src.width;
                    let result = if src_w == dst_w {
                        val
                    } else {
                        match (src_w, dst_w) {
                            (Width::W8, Width::W16) | (Width::W8, Width::W32) => {
                                if to_type.is_signed() {
                                    sign_extend(val, Width::W8)
                                } else {
                                    val & 0xFF
                                }
                            }
                            (Width::W16, Width::W8) | (Width::W32, Width::W8) => val & 0xFF,
                            (Width::W16, Width::W32) => {
                                if to_type.is_signed() {
                                    sign_extend(val, Width::W16)
                                } else {
                                    val & 0xFFFF
                                }
                            }
                            (Width::W32, Width::W16) => val & 0xFFFF,
                            _ => val,
                        }
                    };
                    constants.insert(dst.id, result);
                    new_body.push(IrInstr {
                        op: IrOp::LoadImm { dst: *dst, value: result },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    constants.remove(&dst.id);
                    new_body.push(instr.clone());
                }
            }

            // Everything else: keep the instruction, invalidate dst if any.
            _ => {
                new_body.push(instr.clone());
            }
        }
    }

    if changed {
        func.body = new_body;
    }
    changed
}

// ---------------------------------------------------------------------------
// Strength reduction
// ---------------------------------------------------------------------------

/// Replace multiply/divide/modulo by powers of two with shift/mask.
fn strength_reduce(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // First pass: build map of known constant values (from LoadImm).
    let constants: HashMap<u32, i64> = func
        .body
        .iter()
        .filter_map(|instr| {
            if let IrOp::LoadImm { dst, value } = &instr.op {
                Some((dst.id, *value))
            } else {
                None
            }
        })
        .collect();

    for instr in &mut func.body {
        match &instr.op {
            // Multiply by power of 2 → shift left
            IrOp::Mul { dst, lhs, rhs, width, signed: _ } => {
                if let Some(&val) = constants.get(&rhs.id) {
                    if val > 0 && is_power_of_two(val) {
                        let shift_reg = VReg::new(rhs.id, rhs.width);
                        instr.op = IrOp::Shl {
                            dst: *dst,
                            lhs: *lhs,
                            rhs: shift_reg,
                            width: *width,
                        };
                        // The rhs LoadImm value is updated in a second pass below
                        // to hold the shift amount (log2(val)).
                        changed = true;
                        continue;
                    }
                }
                // Also check if lhs is the power-of-2 constant (commutative)
                if let Some(&val) = constants.get(&lhs.id) {
                    if val > 0 && is_power_of_two(val) {
                        let shift_reg = VReg::new(lhs.id, lhs.width);
                        instr.op = IrOp::Shl {
                            dst: *dst,
                            lhs: *rhs,
                            rhs: shift_reg,
                            width: *width,
                        };
                        changed = true;
                        continue;
                    }
                }
            }

            // Unsigned divide by power of 2 → shift right
            IrOp::Div { dst, lhs, rhs, width, signed } => {
                if !*signed {
                    if let Some(&val) = constants.get(&rhs.id) {
                        if val > 0 && is_power_of_two(val) {
                            let shift_reg = VReg::new(rhs.id, rhs.width);
                            instr.op = IrOp::Shr {
                                dst: *dst,
                                lhs: *lhs,
                                rhs: shift_reg,
                                width: *width,
                                arithmetic: false,
                            };
                            changed = true;
                        }
                    }
                }
            }

            // Unsigned modulo by power of 2 → bitwise AND with (val - 1)
            IrOp::Mod { dst, lhs, rhs, width, signed } => {
                if !*signed {
                    if let Some(&val) = constants.get(&rhs.id) {
                        if val > 0 && is_power_of_two(val) {
                            let mask_reg = VReg::new(rhs.id, rhs.width);
                            instr.op = IrOp::And {
                                dst: *dst,
                                lhs: *lhs,
                                rhs: mask_reg,
                                width: *width,
                            };
                            changed = true;
                        }
                    }
                }
            }

            _ => {}
        }
    }

    // Second pass: update the LoadImm values for rewritten strength-reduction ops.
    // We need to change the constant values used as shift amounts or masks.
    if changed {
        // Rebuild the set of vregs that are now used as shift amounts or masks
        let mut shift_amounts: HashMap<u32, i64> = HashMap::new();
        let mut mask_vals: HashMap<u32, i64> = HashMap::new();

        for instr in &func.body {
            match &instr.op {
                IrOp::Shl { rhs, .. } | IrOp::Shr { rhs, .. } => {
                    if let Some(&orig_val) = constants.get(&rhs.id) {
                        if orig_val > 0 && is_power_of_two(orig_val) {
                            let shift = orig_val.trailing_zeros() as i64;
                            shift_amounts.insert(rhs.id, shift);
                        }
                    }
                }
                IrOp::And { rhs, .. } => {
                    if let Some(&orig_val) = constants.get(&rhs.id) {
                        if orig_val > 0 && is_power_of_two(orig_val) {
                            mask_vals.insert(rhs.id, orig_val - 1);
                        }
                    }
                }
                _ => {}
            }
        }

        for instr in &mut func.body {
            if let IrOp::LoadImm { dst, value } = &mut instr.op {
                if let Some(&shift) = shift_amounts.get(&dst.id) {
                    *value = shift;
                }
                if let Some(&mask) = mask_vals.get(&dst.id) {
                    *value = mask;
                }
            }
        }
    }

    changed
}

// ---------------------------------------------------------------------------
// Dead-code elimination
// ---------------------------------------------------------------------------

/// Remove unreachable code after unconditional jumps and unused definitions.
fn dead_code_eliminate(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // Pass 1: Remove unreachable code after unconditional jumps.
    let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());
    let mut unreachable = false;
    for instr in &func.body {
        match &instr.op {
            IrOp::Jump { .. } => {
                new_body.push(instr.clone());
                unreachable = true;
            }
            IrOp::Return { .. } => {
                new_body.push(instr.clone());
                unreachable = true;
            }
            IrOp::Label { .. } => {
                unreachable = false;
                new_body.push(instr.clone());
            }
            _ => {
                if unreachable {
                    changed = true;
                } else {
                    new_body.push(instr.clone());
                }
            }
        }
    }
    func.body = new_body;

    // Pass 2: Remove stores to vregs that are never read.
    // Collect all vregs that are *used* (read) by any instruction.
    let used_vregs = collect_used_vregs(&func.body);

    let old_len = func.body.len();
    func.body.retain(|instr| {
        // Only eliminate instructions that define a vreg but that vreg is never used.
        // Do NOT eliminate instructions with side effects (stores, calls, jumps, etc.)
        if let Some(dst) = get_pure_dst(&instr.op) {
            if !used_vregs.contains(&dst.id) {
                return false; // Remove this dead instruction
            }
        }
        true
    });
    if func.body.len() != old_len {
        changed = true;
    }

    changed
}

/// Get the destination vreg of a pure (side-effect-free) instruction.
/// Returns `None` for instructions with side effects.
fn get_pure_dst(op: &IrOp) -> Option<VReg> {
    match op {
        IrOp::LoadImm { dst, .. } => Some(*dst),
        IrOp::Add { dst, .. }
        | IrOp::Sub { dst, .. }
        | IrOp::Mul { dst, .. }
        | IrOp::Div { dst, .. }
        | IrOp::Mod { dst, .. }
        | IrOp::And { dst, .. }
        | IrOp::Or { dst, .. }
        | IrOp::Xor { dst, .. }
        | IrOp::Shl { dst, .. }
        | IrOp::Shr { dst, .. }
        | IrOp::Eq { dst, .. }
        | IrOp::Ne { dst, .. }
        | IrOp::Lt { dst, .. }
        | IrOp::Le { dst, .. }
        | IrOp::Gt { dst, .. }
        | IrOp::Ge { dst, .. }
        | IrOp::Neg { dst, .. }
        | IrOp::Not { dst, .. }
        | IrOp::LogicalNot { dst, .. }
        | IrOp::Copy { dst, .. }
        | IrOp::Cast { dst, .. }
        | IrOp::AddrOfGlobal { dst, .. }
        | IrOp::PtrAdd { dst, .. } => Some(*dst),
        // Instructions with side effects — never eliminate
        _ => None,
    }
}

/// Collect the set of all vreg IDs that are *read* (used as operands).
fn collect_used_vregs(body: &[IrInstr]) -> HashSet<u32> {
    let mut used = HashSet::new();
    for instr in body {
        match &instr.op {
            IrOp::LoadImm { .. } => {}
            IrOp::LoadGlobal { .. } => {}
            IrOp::StoreGlobal { src, .. } => { used.insert(src.id); }
            IrOp::LoadLocal { .. } => {}
            IrOp::StoreLocal { src, .. } => { used.insert(src.id); }
            IrOp::LoadPtr { ptr, .. } => { used.insert(ptr.id); }
            IrOp::StorePtr { ptr, src } => { used.insert(ptr.id); used.insert(src.id); }
            IrOp::Add { lhs, rhs, .. }
            | IrOp::Sub { lhs, rhs, .. }
            | IrOp::Mul { lhs, rhs, .. }
            | IrOp::Div { lhs, rhs, .. }
            | IrOp::Mod { lhs, rhs, .. }
            | IrOp::And { lhs, rhs, .. }
            | IrOp::Or { lhs, rhs, .. }
            | IrOp::Xor { lhs, rhs, .. }
            | IrOp::Shl { lhs, rhs, .. }
            | IrOp::Shr { lhs, rhs, .. }
            | IrOp::Eq { lhs, rhs, .. }
            | IrOp::Ne { lhs, rhs, .. }
            | IrOp::Lt { lhs, rhs, .. }
            | IrOp::Le { lhs, rhs, .. }
            | IrOp::Gt { lhs, rhs, .. }
            | IrOp::Ge { lhs, rhs, .. } => {
                used.insert(lhs.id);
                used.insert(rhs.id);
            }
            IrOp::Neg { src, .. }
            | IrOp::Not { src, .. }
            | IrOp::LogicalNot { src, .. } => { used.insert(src.id); }
            IrOp::Copy { src, .. } => { used.insert(src.id); }
            IrOp::Cast { src, .. } => { used.insert(src.id); }
            IrOp::Jump { .. } | IrOp::Label { .. } => {}
            IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => {
                used.insert(cond.id);
            }
            IrOp::Call { args, .. } => {
                for a in args {
                    used.insert(a.id);
                }
            }
            IrOp::Return { value } => {
                if let Some(v) = value {
                    used.insert(v.id);
                }
            }
            IrOp::AddrOfGlobal { .. } => {}
            IrOp::PtrAdd { ptr, offset, .. } => {
                used.insert(ptr.id);
                used.insert(offset.id);
            }
        }
    }
    used
}

// ---------------------------------------------------------------------------
// Common sub-expression elimination (CSE)
// ---------------------------------------------------------------------------

/// Within each basic block, detect identical computations and reuse them.
fn cse(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // Key: a hashable representation of a computation.
    // Value: the vreg that already holds the result.
    let mut expr_map: HashMap<CseKey, VReg> = HashMap::new();
    let mut replacements: HashMap<u32, VReg> = HashMap::new(); // old vreg id → replacement vreg

    let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());

    for instr in &func.body {
        // At basic-block boundaries (labels, jumps), clear the CSE map.
        match &instr.op {
            IrOp::Label { .. } | IrOp::Jump { .. }
            | IrOp::JumpIfTrue { .. } | IrOp::JumpIfFalse { .. }
            | IrOp::Call { .. } | IrOp::Return { .. } => {
                expr_map.clear();
                new_body.push(instr.clone());
                continue;
            }
            _ => {}
        }

        // Try to compute a CSE key for this instruction.
        if let Some((key, dst)) = cse_key(&instr.op) {
            if let Some(&existing) = expr_map.get(&key) {
                // This computation was already done — replace with a Copy.
                new_body.push(IrInstr {
                    op: IrOp::Copy { dst, src: existing },
                    line: instr.line,
                });
                replacements.insert(dst.id, existing);
                changed = true;
                continue;
            } else {
                expr_map.insert(key, dst);
            }
        }

        new_body.push(instr.clone());
    }

    if changed {
        func.body = new_body;
    }
    changed
}

/// A hashable key representing a computation for CSE.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CseKey {
    BinOp { op: &'static str, lhs_id: u32, rhs_id: u32, width: Width },
    UnaryOp { op: &'static str, src_id: u32, width: Width },
}

/// Extract a CSE key and the destination vreg from an instruction, if applicable.
fn cse_key(op: &IrOp) -> Option<(CseKey, VReg)> {
    match op {
        IrOp::Add { dst, lhs, rhs, width } => Some((
            CseKey::BinOp { op: "add", lhs_id: lhs.id, rhs_id: rhs.id, width: *width },
            *dst,
        )),
        IrOp::Sub { dst, lhs, rhs, width } => Some((
            CseKey::BinOp { op: "sub", lhs_id: lhs.id, rhs_id: rhs.id, width: *width },
            *dst,
        )),
        IrOp::Mul { dst, lhs, rhs, width, signed } => Some((
            CseKey::BinOp {
                op: if *signed { "muls" } else { "mulu" },
                lhs_id: lhs.id,
                rhs_id: rhs.id,
                width: *width,
            },
            *dst,
        )),
        IrOp::And { dst, lhs, rhs, width } => Some((
            CseKey::BinOp { op: "and", lhs_id: lhs.id, rhs_id: rhs.id, width: *width },
            *dst,
        )),
        IrOp::Or { dst, lhs, rhs, width } => Some((
            CseKey::BinOp { op: "or", lhs_id: lhs.id, rhs_id: rhs.id, width: *width },
            *dst,
        )),
        IrOp::Xor { dst, lhs, rhs, width } => Some((
            CseKey::BinOp { op: "xor", lhs_id: lhs.id, rhs_id: rhs.id, width: *width },
            *dst,
        )),
        IrOp::Neg { dst, src, width } => Some((
            CseKey::UnaryOp { op: "neg", src_id: src.id, width: *width },
            *dst,
        )),
        IrOp::Not { dst, src, width } => Some((
            CseKey::UnaryOp { op: "not", src_id: src.id, width: *width },
            *dst,
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Wrap a value to the appropriate width (truncate to 8/16/32 bits).
fn wrap_result(val: i64, width: Width) -> i64 {
    match width {
        Width::W8 => (val as u8) as i64,
        Width::W16 => (val as u16) as i64,
        Width::W32 => (val as u32) as i64,
    }
}

/// Sign-extend a value from the given width to i64.
fn sign_extend(val: i64, width: Width) -> i64 {
    match width {
        Width::W8 => (val as i8) as i64,
        Width::W16 => (val as i16) as i64,
        Width::W32 => (val as i32) as i64,
    }
}

/// Convert a value to unsigned representation for the given width.
fn to_unsigned(val: i64, width: Width) -> u64 {
    match width {
        Width::W8 => (val as u8) as u64,
        Width::W16 => (val as u16) as u64,
        Width::W32 => (val as u32) as u64,
    }
}

/// Fold a less-than comparison, returning 1 or 0.
fn fold_cmp_lt(a: i64, b: i64, width: Width, signed: bool) -> i64 {
    if signed {
        let sa = sign_extend(a, width);
        let sb = sign_extend(b, width);
        if sa < sb { 1 } else { 0 }
    } else {
        let ua = to_unsigned(a, width);
        let ub = to_unsigned(b, width);
        if ua < ub { 1 } else { 0 }
    }
}

/// Check if a value is a power of two.
fn is_power_of_two(val: i64) -> bool {
    val > 0 && (val & (val - 1)) == 0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::types::CType;

    /// Build a simple function with the given body, optimize it, and return
    /// the optimized body.
    fn opt_body(body: Vec<IrInstr>) -> Vec<IrInstr> {
        let mut func = IrFunction::new("test", CType::Void);
        func.body = body;
        optimize_function(&mut func);
        func.body
    }

    /// Helper to build a LoadImm instruction.
    fn load_imm(id: u32, width: Width, value: i64) -> IrInstr {
        IrInstr::bare(IrOp::LoadImm {
            dst: VReg::new(id, width),
            value,
        })
    }

    // -- Constant folding -------------------------------------------------

    #[test]
    fn fold_add_constants() {
        let body = vec![
            load_imm(0, Width::W16, 10),
            load_imm(1, Width::W16, 20),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        // The Add should be folded to LoadImm 30
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 30 } if dst.id == 2)));
    }

    #[test]
    fn fold_sub_constants() {
        let body = vec![
            load_imm(0, Width::W16, 100),
            load_imm(1, Width::W16, 30),
            IrInstr::bare(IrOp::Sub {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 70 } if dst.id == 2)));
    }

    #[test]
    fn fold_mul_constants() {
        let body = vec![
            load_imm(0, Width::W16, 6),
            load_imm(1, Width::W16, 7),
            IrInstr::bare(IrOp::Mul {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 42 } if dst.id == 2)));
    }

    #[test]
    fn fold_div_by_zero_not_folded() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            load_imm(1, Width::W16, 0),
            IrInstr::bare(IrOp::Div {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: false,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        // Division by zero should NOT be folded
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::Div { .. })));
    }

    #[test]
    fn fold_comparison_eq() {
        let body = vec![
            load_imm(0, Width::W16, 5),
            load_imm(1, Width::W16, 5),
            IrInstr::bare(IrOp::Eq {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 1 } if dst.id == 2)));
    }

    #[test]
    fn fold_neg_constant() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::Neg {
                dst: VReg::new(1, Width::W16),
                src: VReg::new(0, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];
        let result = opt_body(body);
        // -42 as u16 = 65494
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 65494 } if dst.id == 1)));
    }

    #[test]
    fn fold_wraps_8bit() {
        let body = vec![
            load_imm(0, Width::W8, 200),
            load_imm(1, Width::W8, 100),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W8),
                lhs: VReg::new(0, Width::W8),
                rhs: VReg::new(1, Width::W8),
                width: Width::W8,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W8)))),
        ];
        let result = opt_body(body);
        // 200 + 100 = 300, wrapped to 8 bits = 44
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 44 } if dst.id == 2)));
    }

    // -- Constant propagation ---------------------------------------------

    #[test]
    fn propagate_through_copy() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::Copy {
                dst: VReg::new(1, Width::W16),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];
        let result = opt_body(body);
        // Copy of a constant should be replaced with LoadImm
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 42 } if dst.id == 1)));
    }

    // -- Strength reduction -----------------------------------------------

    #[test]
    fn strength_reduce_mul_by_4() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_x".into(),
            }),
            load_imm(1, Width::W16, 4),
            IrInstr::bare(IrOp::Mul {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: false,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_y".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // Should be rewritten to Shl with shift count 2
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::Shl { .. })));
        // The LoadImm for the shift amount should be 2 (log2(4))
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 2 } if dst.id == 1)));
    }

    #[test]
    fn strength_reduce_unsigned_div_by_8() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_x".into(),
            }),
            load_imm(1, Width::W16, 8),
            IrInstr::bare(IrOp::Div {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: false,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_y".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::Shr { arithmetic: false, .. })));
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 3 } if dst.id == 1)));
    }

    #[test]
    fn strength_reduce_unsigned_mod_by_256() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_x".into(),
            }),
            load_imm(1, Width::W16, 256),
            IrInstr::bare(IrOp::Mod {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: false,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_y".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::And { .. })));
        // mask = 256 - 1 = 255
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 255 } if dst.id == 1)));
    }

    #[test]
    fn signed_div_not_strength_reduced() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_x".into(),
            }),
            load_imm(1, Width::W16, 4),
            IrInstr::bare(IrOp::Div {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_y".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // Signed div should NOT be converted to shift
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::Div { signed: true, .. })));
    }

    // -- Dead-code elimination --------------------------------------------

    #[test]
    fn dce_removes_unreachable_after_jump() {
        let body = vec![
            IrInstr::bare(IrOp::Jump { target: Label::new(0) }),
            // This instruction is unreachable
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // The LoadImm should be removed
        assert!(!result.iter().any(|i| matches!(&i.op, IrOp::LoadImm { value: 42, .. })));
    }

    #[test]
    fn dce_removes_unused_definitions() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            load_imm(1, Width::W16, 99),
            // Only vreg 1 is used
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];
        let result = opt_body(body);
        // LoadImm for vreg 0 should be removed (unused)
        assert!(!result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 42 } if dst.id == 0)));
        // LoadImm for vreg 1 should still be there
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 99 } if dst.id == 1)));
    }

    // -- CSE --------------------------------------------------------------

    #[test]
    fn cse_eliminates_duplicate_add() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_a".into(),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_b".into(),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            // Duplicate computation
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_c".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_d".into(),
                src: VReg::new(3, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // The second Add should be replaced with a Copy from vreg 2
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::Copy { dst, src } if dst.id == 3 && src.id == 2)));
    }

    #[test]
    fn cse_cleared_at_label() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_a".into(),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_b".into(),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            // Same computation after a label — should NOT be CSE'd
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_c".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_d".into(),
                src: VReg::new(3, Width::W16),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // After a label, CSE is cleared, so the second Add should remain
        let add_count = result.iter().filter(|i| matches!(&i.op, IrOp::Add { .. })).count();
        assert_eq!(add_count, 2);
    }

    // -- Combined optimizations -------------------------------------------

    #[test]
    fn chain_fold_propagate_dce() {
        // x = 3; y = 4; z = x + y; return z;
        // Should fold to: return 7; with x and y eliminated.
        let body = vec![
            load_imm(0, Width::W16, 3),
            load_imm(1, Width::W16, 4),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];
        let result = opt_body(body);
        // Should have just LoadImm 7 and Return
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0].op, IrOp::LoadImm { value: 7, .. }));
        assert!(matches!(&result[1].op, IrOp::Return { .. }));
    }

    #[test]
    fn fold_chained_operations() {
        // a = 2; b = 3; c = a * b; d = c + 1; return d;
        // Should fold to: return 7;
        let body = vec![
            load_imm(0, Width::W16, 2),
            load_imm(1, Width::W16, 3),
            IrInstr::bare(IrOp::Mul {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            load_imm(3, Width::W16, 1),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(4, Width::W16),
                lhs: VReg::new(2, Width::W16),
                rhs: VReg::new(3, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(4, Width::W16)))),
        ];
        let result = opt_body(body);
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0].op, IrOp::LoadImm { value: 7, .. }));
    }

    // -- 8-bit wrapping ---------------------------------------------------

    #[test]
    fn fold_bitwise_and_8bit() {
        let body = vec![
            load_imm(0, Width::W8, 0xFF),
            load_imm(1, Width::W8, 0x0F),
            IrInstr::bare(IrOp::And {
                dst: VReg::new(2, Width::W8),
                lhs: VReg::new(0, Width::W8),
                rhs: VReg::new(1, Width::W8),
                width: Width::W8,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W8)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 0x0F } if dst.id == 2)));
    }

    // -- Logical not folding ----------------------------------------------

    #[test]
    fn fold_logical_not_zero() {
        let body = vec![
            load_imm(0, Width::W16, 0),
            IrInstr::bare(IrOp::LogicalNot {
                dst: VReg::new(1, Width::W16),
                src: VReg::new(0, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 1 } if dst.id == 1)));
    }

    #[test]
    fn fold_logical_not_nonzero() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::LogicalNot {
                dst: VReg::new(1, Width::W16),
                src: VReg::new(0, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];
        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op,
            IrOp::LoadImm { dst, value: 0 } if dst.id == 1)));
    }
}
