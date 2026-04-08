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
use std::env;

use crate::ir::{IrFunction, IrInstr, IrOp, IrProgram, Label, VReg, Width};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Maximum IR instruction count for a function to be inlined.
const INLINE_THRESHOLD: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptProfile {
    Default,
    Benchmark,
}

fn current_opt_profile() -> OptProfile {
    match env::var("V6C_OPT_PROFILE") {
        Ok(v) if v.eq_ignore_ascii_case("bench") || v.eq_ignore_ascii_case("benchmark") => {
            OptProfile::Benchmark
        }
        _ => OptProfile::Default,
    }
}

/// Optimize an entire IR program in-place.
pub fn optimize(program: &mut IrProgram) {
    let profile = current_opt_profile();

    // First, run per-function optimization passes.
    for func in &mut program.functions {
        optimize_function(func, profile);
    }

    // Then, run whole-program passes (inlining).
    inline_expand(program);
    function_specialization(program, profile);

    // Re-optimize after inlining to clean up.
    for func in &mut program.functions {
        optimize_function(func, profile);
    }

    // Dedup identical specializations before removing dead functions.
    dedup_specializations(program);

    // Remove functions unreachable from main (only when main exists).
    if program.functions.iter().any(|f| f.name == "main") {
        remove_dead_functions(program);
    }

    // Run loop-specific passes once (outside the fixed-point loop to avoid
    // interaction between passes causing unbounded IR growth).
    for func in &mut program.functions {
        let mut loop_changed = false;
        loop_changed |= induction_variable_optimization(func);
        loop_changed |= loop_unrolling(func);
        if loop_changed {
            // Clean up after loop transformations.
            optimize_function(func, profile);
        }
    }

    // Benchmark profile runs one extra cleanup round.
    if profile == OptProfile::Benchmark {
        for func in &mut program.functions {
            optimize_function(func, profile);
        }
    }
}

/// Run all optimization passes on a single function.
fn optimize_function(func: &mut IrFunction, profile: OptProfile) {
    // Run passes in a fixed-point loop until no more changes.
    loop {
        let mut changed = false;
        match profile {
            OptProfile::Default => {
                changed |= constant_fold_and_propagate(func);
                changed |= compare_zero_simplify(func);
                changed |= dead_branch_eliminate(func);
                changed |= load_store_forwarding(func);
                changed |= copy_propagate(func);
                changed |= redundant_store_eliminate(func);
                changed |= remove_dead_labels(func);
                changed |= strength_reduce(func);
                changed |= narrow_promoted_arithmetic(func);
                changed |= narrow_byte_ops(func);
                changed |= dead_code_eliminate(func);
                changed |= cse(func);
                changed |= jump_threading(func);
                changed |= loop_invariant_code_motion(func);
            }
            OptProfile::Benchmark => {
                changed |= constant_fold_and_propagate(func);
                changed |= compare_zero_simplify(func);
                changed |= dead_branch_eliminate(func);
                changed |= load_store_forwarding(func);
                changed |= copy_propagate(func);
                changed |= redundant_store_eliminate(func);
                changed |= remove_dead_labels(func);
                changed |= cse(func);
                changed |= narrow_promoted_arithmetic(func);
                changed |= narrow_byte_ops(func);
                changed |= strength_reduce(func);
                changed |= jump_threading(func);
                changed |= loop_invariant_code_motion(func);
                changed |= dead_code_eliminate(func);
            }
        }
        if !changed {
            break;
        }
    }

    // Final scheduling pass: sink W8 loads closer to their consumers.
    // This runs after the fixed-point loop so that use-counts are stable.
    sink_w8_loads(func);
}

// ---------------------------------------------------------------------------
// Function specialization for constant arguments
// ---------------------------------------------------------------------------

fn function_specialization(program: &mut IrProgram, profile: OptProfile) {
    let size_limit = match profile {
        OptProfile::Default => 40,
        OptProfile::Benchmark => 80,
    };

    let callee_templates: HashMap<String, IrFunction> = program
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    let candidates: HashSet<String> = program
        .functions
        .iter()
        .filter(|f| {
            !f.name.contains("__spec_")
                && !f.is_variadic
                && !f.is_asm_body
                && !has_inline_asm(f)
                && f.body.len() <= size_limit
                && !calls_self(f)
        })
        .map(|f| f.name.clone())
        .collect();

    if candidates.is_empty() {
        return;
    }

    let mut next_vreg = next_vreg_id(program);
    let mut spec_cache: HashMap<String, String> = HashMap::new();
    let mut pending_funcs: Vec<IrFunction> = Vec::new();
    let mut pending_globals: Vec<crate::ir::GlobalVar> = Vec::new();

    for caller in &mut program.functions {
        let mut const_map: HashMap<u32, i64> = HashMap::new();

        for instr in &mut caller.body {
            match &mut instr.op {
                IrOp::LoadImm { dst, value } => {
                    const_map.insert(dst.id, *value);
                }
                IrOp::Copy { dst, src } => {
                    if let Some(v) = const_map.get(&src.id).copied() {
                        const_map.insert(dst.id, v);
                    } else {
                        const_map.remove(&dst.id);
                    }
                }
                IrOp::Cast { dst, src, .. } => {
                    if let Some(v) = const_map.get(&src.id).copied() {
                        const_map.insert(dst.id, v);
                    } else {
                        const_map.remove(&dst.id);
                    }
                }
                IrOp::Label { .. }
                | IrOp::Jump { .. }
                | IrOp::JumpIfTrue { .. }
                | IrOp::JumpIfFalse { .. }
                | IrOp::Return { .. } => {
                    const_map.clear();
                }
                IrOp::Call { func_name, args, .. } => {
                    if candidates.contains(func_name) && !func_name.contains("__spec_") {
                        let const_args: Vec<(usize, i64)> = args
                            .iter()
                            .enumerate()
                            .filter_map(|(i, arg)| const_map.get(&arg.id).copied().map(|v| (i, v)))
                            .collect();

                        if !const_args.is_empty() {
                            let key = specialization_key(func_name, &const_args);
                            let spec_name = if let Some(existing) = spec_cache.get(&key).cloned() {
                                existing
                            } else {
                                let Some(callee) = callee_templates.get(func_name).cloned() else {
                                    continue;
                                };
                                let name = format!("__spec_{}_{}", func_name, spec_cache.len());
                                let (clone_func, clone_globals) = clone_specialized_function(
                                    &callee,
                                    &name,
                                    &const_args,
                                    &mut next_vreg,
                                    &program.globals,
                                );

                                let mut optimized_clone = clone_func;
                                optimize_function(&mut optimized_clone, profile);

                                pending_globals.extend(clone_globals);
                                pending_funcs.push(optimized_clone);
                                spec_cache.insert(key, name.clone());
                                name
                            };
                            *func_name = spec_name;
                        }
                    }

                    const_map.clear();
                }
                _ => {
                    if let Some(dst) = get_dst_vreg(&instr.op) {
                        const_map.remove(&dst.id);
                    }
                }
            }
        }
    }

    if !pending_globals.is_empty() {
        program.globals.extend(pending_globals);
    }
    if !pending_funcs.is_empty() {
        program.functions.extend(pending_funcs);
    }
}

fn next_vreg_id(program: &IrProgram) -> u32 {
    program
        .functions
        .iter()
        .flat_map(|f| f.body.iter())
        .flat_map(|instr| collect_vregs(&instr.op))
        .map(|v| v.id)
        .max()
        .map(|id| id + 1)
        .unwrap_or(0)
}

fn collect_vregs(op: &IrOp) -> Vec<VReg> {
    let mut out = Vec::new();
    if let Some(dst) = get_dst_vreg(op) {
        out.push(dst);
    }
    match op {
        IrOp::StoreGlobal { src, .. } | IrOp::StoreLocal { src, .. } => out.push(*src),
        IrOp::LoadPtr { ptr, .. } => out.push(*ptr),
        IrOp::StorePtr { ptr, src } => {
            out.push(*ptr);
            out.push(*src);
        }
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
            out.push(*lhs);
            out.push(*rhs);
        }
        IrOp::Neg { src, .. } | IrOp::Not { src, .. } | IrOp::LogicalNot { src, .. } => out.push(*src),
        IrOp::Copy { src, .. } | IrOp::Cast { src, .. } => out.push(*src),
        IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => out.push(*cond),
        IrOp::Call { args, dst, .. } => {
            out.extend(args.iter().copied());
            if let Some(d) = dst {
                out.push(*d);
            }
        }
        IrOp::Return { value } => {
            if let Some(v) = value {
                out.push(*v);
            }
        }
        IrOp::PtrAdd { ptr, offset, .. } => {
            out.push(*ptr);
            out.push(*offset);
        }
        _ => {}
    }
    out
}

fn specialization_key(func_name: &str, const_args: &[(usize, i64)]) -> String {
    let mut parts: Vec<String> = const_args
        .iter()
        .map(|(i, v)| format!("{}={}", i, v))
        .collect();
    parts.sort();
    format!("{}|{}", func_name, parts.join(","))
}

fn clone_specialized_function(
    callee: &IrFunction,
    new_name: &str,
    const_args: &[(usize, i64)],
    next_vreg: &mut u32,
    globals: &[crate::ir::GlobalVar],
) -> (IrFunction, Vec<crate::ir::GlobalVar>) {
    let old_name = callee.name.clone();
    let old_prefix = format!("_l_{}", old_name);
    let new_prefix = format!("_l_{}", new_name);

    let const_map: HashMap<usize, i64> = const_args.iter().copied().collect();

    let mut cloned = callee.clone();
    cloned.name = new_name.to_string();

    let mut body = Vec::with_capacity(cloned.body.len() + const_map.len() * 2);

    for instr in &cloned.body {
        let rewritten = rename_local_labels_in_op(&instr.op, &old_prefix, &new_prefix);

        if let IrOp::StoreGlobal { addr_label, .. } = &rewritten {
            if let Some((arg_idx, _param)) = cloned
                .params
                .iter()
                .enumerate()
                .find(|(_, p)| addr_label == &format!("{}_{}", new_prefix, p.name))
            {
                if let Some(&const_val) = const_map.get(&arg_idx) {
                    let width = cloned.params[arg_idx].vreg.width;
                    let c = VReg::new(*next_vreg, width);
                    *next_vreg += 1;
                    body.push(IrInstr {
                        op: IrOp::LoadImm {
                            dst: c,
                            value: const_val,
                        },
                        line: instr.line,
                    });
                    body.push(IrInstr {
                        op: IrOp::StoreGlobal {
                            addr_label: addr_label.clone(),
                            src: c,
                        },
                        line: instr.line,
                    });
                    continue;
                }
            }
        }

        body.push(IrInstr {
            op: rewritten,
            line: instr.line,
        });
    }

    cloned.body = body;

    let cloned_globals: Vec<crate::ir::GlobalVar> = globals
        .iter()
        .filter(|g| g.name.starts_with(&old_prefix))
        .map(|g| crate::ir::GlobalVar {
            name: g.name.replacen(&old_prefix, &new_prefix, 1),
            ty: g.ty.clone(),
            init: g.init.clone(),
        })
        .collect();

    (cloned, cloned_globals)
}

fn rename_local_labels_in_op(op: &IrOp, old_prefix: &str, new_prefix: &str) -> IrOp {
    match op {
        IrOp::LoadGlobal { dst, addr_label } => IrOp::LoadGlobal {
            dst: *dst,
            addr_label: maybe_rewrite_local_label(addr_label, old_prefix, new_prefix),
        },
        IrOp::StoreGlobal { addr_label, src } => IrOp::StoreGlobal {
            addr_label: maybe_rewrite_local_label(addr_label, old_prefix, new_prefix),
            src: *src,
        },
        IrOp::AddrOfGlobal { dst, name } => IrOp::AddrOfGlobal {
            dst: *dst,
            name: maybe_rewrite_local_label(name, old_prefix, new_prefix),
        },
        _ => op.clone(),
    }
}

fn maybe_rewrite_local_label(label: &str, old_prefix: &str, new_prefix: &str) -> String {
    if label.starts_with(old_prefix) {
        label.replacen(old_prefix, new_prefix, 1)
    } else {
        label.to_string()
    }
}

// ---------------------------------------------------------------------------
// Memory-versioned load/store forwarding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct MemValue {
    version: u32,
    src: VReg,
}

/// Substitute source-vreg occurrences through `Copy` and width-preserving
/// `Cast` definitions within a basic block.
///
/// When `Copy { dst: v2, src: v1 }` is seen, all subsequent uses of `v2`
/// are rewritten to `v1`.  Combined with DCE this eliminates the copy
/// instruction entirely, and prevents the codegen from allocating a
/// separate physical register for what is logically the same value.
///
/// Width-preserving `Cast` instructions are treated the same way because
/// on 8080 all pointer types are the same (16-bit) representation.
fn copy_propagate(func: &mut IrFunction) -> bool {
    let mut changed = false;
    // Maps vreg-id → the canonical vreg it was aliased to within this block.
    let mut subs: HashMap<u32, VReg> = HashMap::new();

    for instr in &mut func.body {
        // Block boundaries: aliases must not flow across control-flow edges.
        match &instr.op {
            IrOp::Label { .. }
            | IrOp::Jump { .. }
            | IrOp::JumpIfTrue { .. }
            | IrOp::JumpIfFalse { .. } => {
                subs.clear();
            }
            _ => {}
        }

        // Apply current substitutions to all SOURCE operands of this op.
        if !subs.is_empty() {
            subst_vreg_in_op(&mut instr.op, &subs, &mut changed);
        }

        // Record new alias facts (after applying existing subs so chains
        // are flattened: v3→v2→v1 becomes v3→v1 directly).
        match &instr.op {
            IrOp::Copy { dst, src } => {
                subs.insert(dst.id, *src);
            }
            IrOp::Cast { dst, src, .. } if dst.width == src.width => {
                subs.insert(dst.id, *src);
            }
            _ => {}
        }
    }

    changed
}

/// Apply `subs` (vreg-id → vreg alias) to every *source* vreg in `op`.
fn subst_vreg_in_op(op: &mut IrOp, subs: &HashMap<u32, VReg>, changed: &mut bool) {
    fn sub1(v: &mut VReg, subs: &HashMap<u32, VReg>, changed: &mut bool) {
        if let Some(&s) = subs.get(&v.id) {
            if s.id != v.id {
                *v = s;
                *changed = true;
            }
        }
    }
    match op {
        IrOp::Copy { src, .. } | IrOp::Cast { src, .. } => sub1(src, subs, changed),
        IrOp::StoreGlobal { src, .. } | IrOp::StoreLocal { src, .. } => sub1(src, subs, changed),
        IrOp::LoadPtr { ptr, .. } => sub1(ptr, subs, changed),
        IrOp::StorePtr { ptr, src } => {
            sub1(ptr, subs, changed);
            sub1(src, subs, changed);
        }
        IrOp::PtrAdd { ptr, offset, .. } => {
            sub1(ptr, subs, changed);
            sub1(offset, subs, changed);
        }
        IrOp::Neg { src, .. } | IrOp::Not { src, .. } | IrOp::LogicalNot { src, .. } => {
            sub1(src, subs, changed);
        }
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
            sub1(lhs, subs, changed);
            sub1(rhs, subs, changed);
        }
        IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => {
            sub1(cond, subs, changed);
        }
        IrOp::Return { value } => {
            if let Some(v) = value {
                sub1(v, subs, changed);
            }
        }
        IrOp::Call { args, .. } => {
            for a in args.iter_mut() {
                sub1(a, subs, changed);
            }
        }
        _ => {}
    }
}

/// Forward known values for static/global memory locations within a block.
///
/// This tracks a simple per-label memory version and replaces
/// `LoadGlobal dst, L` with `Copy dst, src` when the latest value for `L`
/// in the current region is known to come from `src`.
fn load_store_forwarding(func: &mut IrFunction) -> bool {
    let mut changed = false;
    let mut mem_state: HashMap<String, MemValue> = HashMap::new();
    let mut version: u32 = 1;
    let mut out = Vec::with_capacity(func.body.len());

    for instr in &func.body {
        match &instr.op {
            // Unconditional control flow and join points: any path can reach
            // what follows, so we conservatively clear the forwarding map.
            IrOp::Label { .. }
            | IrOp::Jump { .. }
            | IrOp::Return { .. } => {
                mem_state.clear();
                out.push(instr.clone());
            }
            // Conditional branches: the fall-through path still sees every
            // store that preceded the branch, so the forwarding map remains
            // valid.  The map will be cleared when the join-point Label is
            // reached below.
            IrOp::JumpIfTrue { .. } | IrOp::JumpIfFalse { .. } => {
                out.push(instr.clone());
            }
            IrOp::StoreGlobal { addr_label, src } => {
                mem_state.insert(
                    addr_label.clone(),
                    MemValue {
                        version,
                        src: *src,
                    },
                );
                version = version.wrapping_add(1);
                out.push(instr.clone());
            }
            IrOp::LoadGlobal { dst, addr_label } => {
                if let Some(mem) = mem_state.get(addr_label) {
                    let _observed_version = mem.version;
                    if mem.src.id != dst.id {
                        out.push(IrInstr {
                            op: IrOp::Copy {
                                dst: *dst,
                                src: mem.src,
                            },
                            line: instr.line,
                        });
                        changed = true;
                    } else {
                        out.push(instr.clone());
                    }
                } else {
                    out.push(instr.clone());
                }
            }
            IrOp::StorePtr { .. } | IrOp::LoadPtr { .. } | IrOp::StoreLocal { .. } => {
                // Unknown memory aliasing may invalidate non-local global
                // forwarding facts.  Compiler-local _l_ labels (local variable
                // storage slots) are never reachable via user pointers, so we
                // keep them.
                mem_state.retain(|label, _| label.starts_with("_l_"));
                out.push(instr.clone());
            }
            IrOp::Call { .. } => {
                // Calls are conservatively treated as memory clobbers.
                mem_state.clear();
                out.push(instr.clone());
            }
            // Inline asm may read/write any memory location.
            IrOp::InlineAsm { .. } => {
                mem_state.clear();
                out.push(instr.clone());
            }
            _ => out.push(instr.clone()),
        }
    }

    if changed {
        func.body = out;
    }
    changed
}

// ---------------------------------------------------------------------------
// Sparse byte-range facts and narrowing
// ---------------------------------------------------------------------------

/// Narrow W16 arithmetic operations to W8 when their result is only consumed
/// for its low 8 bits.
///
/// Eliminates the sign-extension branch emitted for `Cast(W8→W16)` in the
/// common pattern generated by C integer promotion followed by assignment back
/// to a narrower type, e.g.:
///
/// ```c
/// char data3, data4;
/// data3 += data4 + 1;   // data4 is promoted to int, then result truncated back
/// ```
///
/// Transforms `Add(W16, Cast(W16, x), imm)` → `Add(W8, x, imm)` when the
/// W16 result is only used in W8-consuming contexts.  The now-unused
/// `Cast(W8→W16)` is removed by the subsequent dead-code elimination pass.
///
/// Mathematical validity: for Add, Sub, And, Or, Xor, Mul, and Shl,
/// `(a op b) & 0xFF == ((a & 0xFF) op (b & 0xFF)) & 0xFF`, so computing in
/// W8 and discarding the upper byte is always equivalent to truncating a W16
/// result.
fn narrow_promoted_arithmetic(func: &mut IrFunction) -> bool {
    // ---- pass 1: record def sites we can redirect -------------------------
    // cast_origin[W16_id] = the W8 vreg that was widened to produce it.
    let mut cast_origin: HashMap<u32, VReg> = HashMap::new();
    // imm_remat: vreg IDs defined by a LoadImm (codegen handles these as
    // remat immediates via known_imm(), even when the vreg struct says W16).
    let mut imm_remat: HashSet<u32> = HashSet::new();
    for instr in &func.body {
        match &instr.op {
            IrOp::Cast { dst, src, .. }
                if src.width == Width::W8 && dst.width == Width::W16 =>
            {
                cast_origin.insert(dst.id, *src);
            }
            IrOp::LoadImm { dst, .. } => {
                imm_remat.insert(dst.id);
            }
            _ => {}
        }
    }

    // ---- pass 2: use-count all vregs; classify uses as W8-context or not --
    // A "W8-consuming" instruction only reads the low 8 bits of its operands:
    //   - any BinOp with width == W8
    //   - Cast that narrows to W8
    let mut total_uses: HashMap<u32, u32> = HashMap::new();
    let mut w8_uses: HashMap<u32, u32> = HashMap::new();

    for instr in &func.body {
        let dst_id = get_dst_vreg(&instr.op).map(|d| d.id);
        let w8_ctx = matches!(
            &instr.op,
            IrOp::Add  { width: Width::W8, .. }
            | IrOp::Sub  { width: Width::W8, .. }
            | IrOp::Mul  { width: Width::W8, .. }
            | IrOp::And  { width: Width::W8, .. }
            | IrOp::Or   { width: Width::W8, .. }
            | IrOp::Xor  { width: Width::W8, .. }
            | IrOp::Shl  { width: Width::W8, .. }
        ) || matches!(&instr.op, IrOp::Cast { dst, .. } if dst.width == Width::W8);

        for v in collect_vregs(&instr.op) {
            if Some(v.id) == dst_id {
                continue; // skip the definition, only count uses
            }
            *total_uses.entry(v.id).or_insert(0) += 1;
            if w8_ctx {
                *w8_uses.entry(v.id).or_insert(0) += 1;
            }
        }
    }

    // "w8_consumed": every use of this vreg is in a W8-consuming context.
    let w8_consumed: HashSet<u32> = total_uses
        .iter()
        .filter(|(&id, &total)| {
            total > 0 && w8_uses.get(&id).copied().unwrap_or(0) == total
        })
        .map(|(id, _)| *id)
        .collect();

    // ---- pass 3: narrow W16 binops whose result is w8_consumed ------------
    let mut changed = false;
    for instr in &mut func.body {
        match &mut instr.op {
            IrOp::Add { dst, lhs, rhs, width }
            | IrOp::Sub { dst, lhs, rhs, width }
            | IrOp::Mul { dst, lhs, rhs, width, .. }
            | IrOp::And { dst, lhs, rhs, width }
            | IrOp::Or  { dst, lhs, rhs, width }
            | IrOp::Xor { dst, lhs, rhs, width }
            | IrOp::Shl { dst, lhs, rhs, width }
                if *width == Width::W16 && w8_consumed.contains(&dst.id) =>
            {
                // Try to obtain a W8 source for lhs.
                // - Widening cast → redirect to the original W8 vreg.
                // - LoadImm → keep as-is; codegen handles W16 remat imms in
                //   W8 binops via known_imm(), emitting ADI/INR directly.
                let new_lhs = cast_origin
                    .get(&lhs.id)
                    .cloned()
                    .or_else(|| if imm_remat.contains(&lhs.id) { Some(*lhs) } else { None });
                let new_rhs = cast_origin
                    .get(&rhs.id)
                    .cloned()
                    .or_else(|| if imm_remat.contains(&rhs.id) { Some(*rhs) } else { None });

                if let (Some(nl), Some(nr)) = (new_lhs, new_rhs) {
                    *lhs = nl;
                    *rhs = nr;
                    *width = Width::W8;
                    dst.width = Width::W8;
                    changed = true;
                }
            }
            _ => {}
        }
    }

    changed
}

#[derive(Debug, Clone, Copy, Default)]
struct ValueRangeFact {
    min: i64,
    max: i64,
    known: Option<i64>,
    valid: bool,
}

impl ValueRangeFact {
    fn unknown() -> Self {
        Self {
            min: i64::MIN,
            max: i64::MAX,
            known: None,
            valid: false,
        }
    }

    fn exact(value: i64) -> Self {
        Self {
            min: value,
            max: value,
            known: Some(value),
            valid: true,
        }
    }

    fn interval(min: i64, max: i64) -> Self {
        Self {
            min,
            max,
            known: if min == max { Some(min) } else { None },
            valid: true,
        }
    }

    fn is_byte(self) -> bool {
        self.valid && self.min >= 0 && self.max <= 0xFF
    }

    fn is_signed_byte(self) -> bool {
        self.valid && self.min >= -128 && self.max <= 127
    }
}

fn narrow_byte_ops(func: &mut IrFunction) -> bool {
    let facts = compute_value_ranges(func);
    // Build a map from W16-vreg-ID → the W8 source vreg it was widened from,
    // and the immediate value for W16 imm vregs that fit in a byte.
    // Used to redirect narrowed comparison operands back to their W8 origins.
    let mut cast_origin: HashMap<u32, VReg> = HashMap::new();
    for instr in &func.body {
        if let IrOp::Cast { dst, src, .. } = &instr.op {
            if src.width == Width::W8 && dst.width == Width::W16 {
                cast_origin.insert(dst.id, *src);
            }
        }
        // For LoadImm that produce W16 but hold a byte value, note the imm
        // vreg itself as a potential W8 substitute (we'll make a copy).
        // (Handled below in the substitution logic.)
    }
    let mut changed = false;

    for instr in &mut func.body {
        match &mut instr.op {
            IrOp::And { dst, lhs, rhs, width }
            | IrOp::Or { dst, lhs, rhs, width }
            | IrOp::Xor { dst, lhs, rhs, width }
            | IrOp::Add { dst, lhs, rhs, width }
            | IrOp::Sub { dst, lhs, rhs, width }
            | IrOp::Shl { dst, lhs, rhs, width }
                if *width == Width::W16 =>
            {
                let lhs_byte = facts.get(&lhs.id).copied().unwrap_or_default().is_byte();
                let rhs_byte = facts.get(&rhs.id).copied().unwrap_or_default().is_byte();
                let dst_byte = facts.get(&dst.id).copied().unwrap_or_default().is_byte();
                if lhs_byte && rhs_byte && dst_byte {
                    *width = Width::W8;
                    changed = true;
                }
            }
            IrOp::Div {
                dst,
                lhs,
                rhs,
                width,
                signed,
            }
            | IrOp::Mod {
                dst,
                lhs,
                rhs,
                width,
                signed,
            } if *width == Width::W16 && !*signed => {
                let lhs_byte = facts.get(&lhs.id).copied().unwrap_or_default().is_byte();
                let rhs_fact = facts.get(&rhs.id).copied().unwrap_or_default();
                let dst_byte = facts.get(&dst.id).copied().unwrap_or_default().is_byte();
                if lhs_byte && rhs_fact.is_byte() && rhs_fact.min > 0 && dst_byte {
                    *width = Width::W8;
                    changed = true;
                }
            }
            IrOp::Eq { lhs, rhs, width, .. }
            | IrOp::Ne { lhs, rhs, width, .. }
                if *width == Width::W16 =>
            {
                let lhs_byte = facts.get(&lhs.id).copied().unwrap_or_default().is_byte();
                let rhs_byte = facts.get(&rhs.id).copied().unwrap_or_default().is_byte();
                let lhs_sbyte = facts.get(&lhs.id).copied().unwrap_or_default().is_signed_byte();
                let rhs_sbyte = facts.get(&rhs.id).copied().unwrap_or_default().is_signed_byte();
                if (lhs_byte && rhs_byte) || (lhs_sbyte && rhs_sbyte) {
                    *width = Width::W8;
                    // Redirect operands to W8 origins if possible.
                    if let Some(&w8_src) = cast_origin.get(&lhs.id) { *lhs = w8_src; }
                    if let Some(&w8_src) = cast_origin.get(&rhs.id) { *rhs = w8_src; }
                    changed = true;
                }
            }
            IrOp::Lt { lhs, rhs, width, signed, .. }
            | IrOp::Le { lhs, rhs, width, signed, .. }
            | IrOp::Gt { lhs, rhs, width, signed, .. }
            | IrOp::Ge { lhs, rhs, width, signed, .. }
                if *width == Width::W16 =>
            {
                let lhs_f = facts.get(&lhs.id).copied().unwrap_or_default();
                let rhs_f = facts.get(&rhs.id).copied().unwrap_or_default();
                let ok = if *signed {
                    lhs_f.is_signed_byte() && rhs_f.is_signed_byte()
                } else {
                    lhs_f.is_byte() && rhs_f.is_byte()
                };
                if ok {
                    *width = Width::W8;
                    // Redirect operands to W8 origins if possible.
                    if let Some(&w8_src) = cast_origin.get(&lhs.id) { *lhs = w8_src; }
                    if let Some(&w8_src) = cast_origin.get(&rhs.id) { *rhs = w8_src; }
                    changed = true;
                }
            }
            _ => {}
        }
    }

    changed
}

fn compute_value_ranges(func: &IrFunction) -> HashMap<u32, ValueRangeFact> {
    let mut facts: HashMap<u32, ValueRangeFact> = HashMap::new();

    for instr in &func.body {
        match &instr.op {
            IrOp::Label { .. } => {
                // At control-flow join points the facts may not hold on all
                // incoming paths.  However, for the purpose of byte-range
                // narrowing it is sufficient to know that a vreg *could* be
                // a byte value — so we are conservative in the other direction:
                // we keep the facts rather than clearing them.  Any incorrect
                // narrowing would be caught by the is_byte/is_signed_byte
                // guards which require valid=true, and those facts are only
                // inserted when we can prove the range from the definition
                // sites (Cast from W8, LoadImm with small value, etc.).
                //
                // Note: vregs are SSA-like (each is written exactly once), so
                // a fact inserted for vreg X before a label is still valid
                // after the label if X is defined before it.
            }
            IrOp::LoadImm { dst, value } => {
                facts.insert(dst.id, ValueRangeFact::exact(*value));
            }
            IrOp::Copy { dst, src } => {
                let src_fact = facts.get(&src.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                facts.insert(dst.id, src_fact);
            }
            IrOp::Cast { dst, src, to_type } => {
                let src_fact = facts.get(&src.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                let cast_fact = match (src.width, dst.width) {
                    (Width::W8, Width::W16) | (Width::W8, Width::W32) => {
                        if to_type.is_signed() {
                            // Sign-extend: preserve the signed-byte range (-128..127)
                            if src_fact.valid {
                                ValueRangeFact::interval(
                                    (src_fact.min as i8) as i64,
                                    (src_fact.max as i8) as i64,
                                )
                            } else {
                                ValueRangeFact::interval(-128, 127)
                            }
                        } else {
                            // Zero-extend: clamp to 0..255
                            if src_fact.valid {
                                ValueRangeFact::interval(src_fact.min.max(0), src_fact.max.min(0xFF))
                            } else {
                                ValueRangeFact::interval(0, 0xFF)
                            }
                        }
                    }
                    (Width::W16, Width::W8) | (Width::W32, Width::W8) => {
                        ValueRangeFact::interval(0, 0xFF)
                    }
                    _ => src_fact,
                };
                facts.insert(dst.id, cast_fact);
            }
            IrOp::Eq { dst, .. }
            | IrOp::Ne { dst, .. }
            | IrOp::Lt { dst, .. }
            | IrOp::Le { dst, .. }
            | IrOp::Gt { dst, .. }
            | IrOp::Ge { dst, .. }
            | IrOp::LogicalNot { dst, .. } => {
                facts.insert(dst.id, ValueRangeFact::interval(0, 1));
            }
            IrOp::And { dst, lhs, rhs, .. } => {
                let l = facts.get(&lhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                let r = facts.get(&rhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                if l.is_byte() && r.is_byte() {
                    facts.insert(dst.id, ValueRangeFact::interval(0, 0xFF));
                } else if r.known.is_some_and(|v| (v & !0xFF) == 0)
                    || l.known.is_some_and(|v| (v & !0xFF) == 0)
                {
                    facts.insert(dst.id, ValueRangeFact::interval(0, 0xFF));
                } else {
                    facts.remove(&dst.id);
                }
            }
            IrOp::Or { dst, lhs, rhs, .. }
            | IrOp::Xor { dst, lhs, rhs, .. }
            | IrOp::Shl { dst, lhs, rhs, .. }
            | IrOp::Add { dst, lhs, rhs, .. }
            | IrOp::Sub { dst, lhs, rhs, .. } => {
                let l = facts.get(&lhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                let r = facts.get(&rhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                if l.is_byte() && r.is_byte() {
                    facts.insert(dst.id, ValueRangeFact::interval(0, 0xFF));
                } else {
                    facts.remove(&dst.id);
                }
            }
            IrOp::Shr {
                dst,
                lhs,
                rhs,
                arithmetic,
                ..
            } => {
                let l = facts.get(&lhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                let r = facts.get(&rhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                if !*arithmetic && l.is_byte() && r.is_byte() {
                    facts.insert(dst.id, ValueRangeFact::interval(0, 0xFF));
                } else {
                    facts.remove(&dst.id);
                }
            }
            IrOp::Div {
                dst,
                lhs,
                rhs,
                signed,
                ..
            }
            | IrOp::Mod {
                dst,
                lhs,
                rhs,
                signed,
                ..
            } => {
                let l = facts.get(&lhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                let r = facts.get(&rhs.id).copied().unwrap_or_else(ValueRangeFact::unknown);
                if !*signed && l.is_byte() && r.is_byte() && r.min > 0 {
                    facts.insert(dst.id, ValueRangeFact::interval(0, 0xFF));
                } else {
                    facts.remove(&dst.id);
                }
            }
            IrOp::LoadGlobal { dst, .. }
            | IrOp::LoadLocal { dst, .. }
            | IrOp::LoadPtr { dst, .. }
            | IrOp::AddrOfGlobal { dst, .. }
            | IrOp::PtrAdd { dst, .. }
            | IrOp::Neg { dst, .. }
            | IrOp::Not { dst, .. }
            | IrOp::Mul { dst, .. } => {
                facts.remove(&dst.id);
            }
            IrOp::StoreGlobal { .. }
            | IrOp::StoreLocal { .. }
            | IrOp::StorePtr { .. }
            | IrOp::Jump { .. }
            | IrOp::JumpIfTrue { .. }
            | IrOp::JumpIfFalse { .. }
            | IrOp::Return { .. } => {}
            IrOp::Call { dst, .. } => {
                if let Some(d) = dst {
                    facts.remove(&d.id);
                }
            }
            IrOp::InlineAsm { .. } => {}
        }
    }

    facts
}

// ---------------------------------------------------------------------------
// Constant folding & propagation
// ---------------------------------------------------------------------------

/// Track known constant values for virtual registers and fold operations.
fn constant_fold_and_propagate(func: &mut IrFunction) -> bool {
    let mut changed = false;
    // Map from VReg id → known constant value
    let mut constants: HashMap<u32, i64> = HashMap::new();
    // Map from compiler-local label name → known constant value.
    // Compiler-local labels (_l_ prefix) are internal storage cells for local
    // variables; they are never accessible via user pointers, so StorePtr
    // cannot alias them.  This lets us propagate constant values through
    // StoreGlobal / LoadGlobal pairs across pointer stores.
    let mut global_consts: HashMap<String, i64> = HashMap::new();

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
                            let sa = sign_extend(a, *width);
                            let sb = sign_extend(b, *width);
                            wrap_result(sa.wrapping_div(sb), *width)
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
                            let sa = sign_extend(a, *width);
                            let sb = sign_extend(b, *width);
                            wrap_result(sa.wrapping_rem(sb), *width)
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
                global_consts.clear();
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
                global_consts.clear();
                new_body.push(instr.clone());
            }

            // Stores/loads from globals — the destination vreg gets an unknown value.
            // Exception: compiler-local _l_ labels are tracked via global_consts.
            IrOp::LoadGlobal { dst, addr_label } => {
                if addr_label.starts_with("_l_") {
                    if let Some(&val) = global_consts.get(addr_label.as_str()) {
                        // Fold: LoadGlobal → LoadImm (constant known for this label).
                        constants.insert(dst.id, val);
                        new_body.push(IrInstr {
                            op: IrOp::LoadImm { dst: *dst, value: val },
                            line: instr.line,
                        });
                        changed = true;
                        continue;
                    }
                }
                constants.remove(&dst.id);
                new_body.push(instr.clone());
            }
            IrOp::LoadLocal { dst, .. }
            | IrOp::LoadPtr { dst, .. } => {
                constants.remove(&dst.id);
                new_body.push(instr.clone());
            }

            IrOp::AddrOfGlobal { dst, .. } => {
                constants.remove(&dst.id);
                new_body.push(instr.clone());
            }

            IrOp::PtrAdd { dst, ptr, offset, element_size } => {
                let off_val = constants.get(&offset.id).copied();
                // PtrAdd(base, 0, _) → Copy(dst, base) — adding 0 is useless.
                if off_val == Some(0) {
                    new_body.push(IrInstr {
                        op: IrOp::Copy { dst: *dst, src: *ptr },
                        line: instr.line,
                    });
                    changed = true;
                } else {
                    // Check if ptr comes from AddrOfGlobal and offset is constant.
                    // Fold: AddrOfGlobal(name) + const*element_size → AddrOfGlobal("name+N")
                    let addr_name = new_body.iter().rev().find_map(|i| {
                        if let IrOp::AddrOfGlobal { dst: d, name } = &i.op {
                            if d.id == ptr.id { Some(name.clone()) } else { None }
                        } else {
                            None
                        }
                    });
                    if let (Some(name), Some(off)) = (addr_name, off_val) {
                        let byte_offset = off * (*element_size as i64);
                        let folded_name = if byte_offset > 0 {
                            format!("{}+{}", name, byte_offset)
                        } else if byte_offset < 0 {
                            format!("{}{}", name, byte_offset)
                        } else {
                            name
                        };
                        new_body.push(IrInstr {
                            op: IrOp::AddrOfGlobal { dst: *dst, name: folded_name },
                            line: instr.line,
                        });
                        changed = true;
                    } else {
                        constants.remove(&dst.id);
                        new_body.push(instr.clone());
                    }
                }
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
                // Track StoreGlobal to compiler-local labels so subsequent
                // LoadGlobal from the same label can be constant-folded.
                if let IrOp::StoreGlobal { addr_label, src } = &instr.op {
                    if addr_label.starts_with("_l_") {
                        if let Some(&val) = constants.get(&src.id) {
                            global_consts.insert(addr_label.clone(), val);
                        } else {
                            global_consts.remove(addr_label.as_str());
                        }
                    }
                }
                if let Some(dst) = get_dst_vreg(&instr.op) {
                    constants.remove(&dst.id);
                }
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
// Dead label removal
// ---------------------------------------------------------------------------

/// Remove IR Label instructions that are never targeted by any jump.
///
/// After full loop unrolling, continuation labels (L1, L3, …) inside the
/// unrolled body are not jumped to by anything, but they still act as
/// basic-block boundaries — clearing the constants map in
/// `constant_fold_and_propagate` and the forwarding map in
/// `load_store_forwarding`.  Removing them lets those passes propagate values
/// across iteration boundaries, enabling full constant folding of the loop.
fn remove_dead_labels(func: &mut IrFunction) -> bool {
    let mut referenced: HashSet<u32> = HashSet::new();
    for instr in &func.body {
        match &instr.op {
            IrOp::Jump { target } => { referenced.insert(target.0); }
            IrOp::JumpIfTrue { target, .. } | IrOp::JumpIfFalse { target, .. } => {
                referenced.insert(target.0);
            }
            _ => {}
        }
    }
    let old_len = func.body.len();
    func.body.retain(|instr| {
        if let IrOp::Label { label } = &instr.op {
            referenced.contains(&label.0)
        } else {
            true
        }
    });
    func.body.len() != old_len
}

// ---------------------------------------------------------------------------
// Redundant store elimination
// ---------------------------------------------------------------------------

/// Remove stores to global/local slots that are overwritten before being read.
///
/// Scans forward.  When a second `StoreGlobal(L)` is seen while `L` still has
/// a pending (unread) store, the pending store is removed.  Basic-block
/// boundaries (labels, jumps, calls, pointer operations) conservatively flush
/// the pending-store table to avoid incorrect removal across control flow.
///
/// A second step removes all stores to `_l_`-prefixed labels (function-local
/// variables promoted to globals by the code-generator) when those labels have
/// no `LoadGlobal` anywhere in the function body — these are dead IV slots
/// left behind after full loop unrolling.
fn redundant_store_eliminate(func: &mut IrFunction) -> bool {
    // --- Step 1: remove overwritten-before-read stores ---
    // Maps global label → index of pending (not-yet-consumed) StoreGlobal.
    let mut pending_global: HashMap<String, usize> = HashMap::new();
    // Maps local offset → index of pending StoreLocal.
    let mut pending_local: HashMap<i32, usize> = HashMap::new();
    // Indices of instructions to remove.
    let mut dead: HashSet<usize> = HashSet::new();

    for (idx, instr) in func.body.iter().enumerate() {
        match &instr.op {
            IrOp::StoreGlobal { addr_label, .. } => {
                if let Some(prev_idx) = pending_global.insert(addr_label.clone(), idx) {
                    dead.insert(prev_idx);
                }
            }
            IrOp::LoadGlobal { addr_label, .. } => {
                pending_global.remove(addr_label);
            }
            IrOp::StoreLocal { offset, .. } => {
                if let Some(prev_idx) = pending_local.insert(*offset, idx) {
                    dead.insert(prev_idx);
                }
            }
            IrOp::LoadLocal { offset, .. } => {
                pending_local.remove(offset);
            }
            // Control-flow boundaries: flush everything.
            IrOp::Label { .. }
            | IrOp::Jump { .. }
            | IrOp::JumpIfTrue { .. }
            | IrOp::JumpIfFalse { .. }
            | IrOp::Return { .. } => {
                pending_global.clear();
                pending_local.clear();
            }
            // Calls and pointer stores may alias any global.
            IrOp::Call { .. }
            | IrOp::StorePtr { .. }
            | IrOp::LoadPtr { .. } => {
                pending_global.clear();
                pending_local.clear();
            }
            // Inline asm may read/write any memory location.
            IrOp::InlineAsm { .. } => {
                pending_global.clear();
                pending_local.clear();
            }
            _ => {}
        }
    }

    // --- Step 2: remove stores to _l_-prefixed locals that are never loaded ---
    //
    // After full loop unrolling + constant folding the loop IV (e.g. _l_main_i)
    // may have all its LoadGlobal instructions replaced by constants, leaving
    // only orphaned StoreGlobal instructions.
    //
    // Similarly, after inlining a callee the parameter-setup store
    // (StoreGlobal _l_callee_param) becomes dead when load_store_forwarding has
    // already forwarded the value through the inlined body, removing the
    // LoadGlobal.  Because parameter passing uses the Call.args VRegs — never
    // pre-placed StoreGlobal — any _l_*-prefixed store in this function that
    // has no matching LoadGlobal can only originate from this function's own
    // locals/params or from the inlining expansion.  Both are safe to drop.
    let loaded_globals: HashSet<String> = func.body.iter()
        .filter_map(|i| {
            if let IrOp::LoadGlobal { addr_label, .. } = &i.op { Some(addr_label.clone()) }
            else { None }
        })
        .collect();

    // Labels referenced in inline asm text are also considered "loaded" —
    // the raw assembly accesses them directly, outside the IR's visibility.
    let asm_referenced: HashSet<String> = func.body.iter()
        .filter_map(|i| {
            if let IrOp::InlineAsm { code, .. } = &i.op { Some(code.as_str()) }
            else { None }
        })
        .flat_map(|code| {
            code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|word| word.starts_with("_l_") || word.starts_with("_g_"))
                .map(|w| w.to_string())
        })
        .collect();

    for (idx, instr) in func.body.iter().enumerate() {
        if let IrOp::StoreGlobal { addr_label, .. } = &instr.op {
            if addr_label.starts_with("_l_")
                && !loaded_globals.contains(addr_label)
                && !asm_referenced.contains(addr_label)
            {
                dead.insert(idx);
            }
        }
    }

    if dead.is_empty() {
        return false;
    }
    let old_len = func.body.len();
    let mut i = 0;
    func.body.retain(|_| {
        let keep = !dead.contains(&i);
        i += 1;
        keep
    });
    func.body.len() != old_len
}

// ---------------------------------------------------------------------------
// Dead-code elimination
// ---------------------------------------------------------------------------

/// Remove unreachable code after unconditional jumps and unused definitions.
fn dead_code_eliminate(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // Pass 1: Remove unreachable code after unconditional jumps, and remove
    // trivial fall-through jumps (`Jump L` immediately followed by `Label L`).
    let body = &func.body;
    // Pre-compute a set of indices that are trivial fall-through jumps.
    let fall_through_jumps: HashSet<usize> = body
        .windows(2)
        .enumerate()
        .filter_map(|(i, pair)| {
            if let (IrOp::Jump { target }, IrOp::Label { label }) =
                (&pair[0].op, &pair[1].op)
            {
                if target.0 == label.0 { Some(i) } else { None }
            } else {
                None
            }
        })
        .collect();

    let mut new_body: Vec<IrInstr> = Vec::with_capacity(body.len());
    let mut unreachable = false;
    for (idx, instr) in body.iter().enumerate() {
        match &instr.op {
            IrOp::Jump { .. } => {
                if unreachable {
                    changed = true;
                } else if fall_through_jumps.contains(&idx) {
                    // Trivial jump-to-next-label: skip it entirely.
                    changed = true;
                } else {
                    new_body.push(instr.clone());
                    unreachable = true;
                }
            }
            IrOp::Return { .. } => {
                if unreachable {
                    changed = true;
                } else {
                    new_body.push(instr.clone());
                    unreachable = true;
                }
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

/// Get the destination vreg of ANY instruction that writes a vreg, including
/// those with side effects (LoadGlobal, LoadLocal, LoadPtr, Call, etc.).
/// Used by dead_branch_eliminate to detect vregs that are mutated.
fn get_any_dst(op: &IrOp) -> Option<VReg> {
    match op {
        IrOp::LoadImm { dst, .. }
        | IrOp::LoadGlobal { dst, .. }
        | IrOp::LoadLocal { dst, .. }
        | IrOp::LoadPtr { dst, .. }
        | IrOp::Add { dst, .. }
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
        IrOp::Call { dst: Some(dst), .. } => Some(*dst),
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
            IrOp::InlineAsm { inputs, .. } => {
                for (vreg, _) in inputs {
                    used.insert(vreg.id);
                }
            }
        }
    }
    used
}

// ---------------------------------------------------------------------------
// Compare-with-zero simplification (Step 4)
// ---------------------------------------------------------------------------

/// Recognize `Eq/Ne(x, 0)` followed by `JumpIfTrue/JumpIfFalse` and simplify
/// to a direct `JumpIfTrue(x)` or `JumpIfFalse(x)`, eliminating the
/// unnecessary comparison vreg.
///
/// Patterns:
///   Eq(dst, x, 0)  + JumpIfTrue(dst, t)   → JumpIfFalse(x, t)
///   Eq(dst, x, 0)  + JumpIfFalse(dst, t)  → JumpIfTrue(x, t)
///   Ne(dst, x, 0)  + JumpIfTrue(dst, t)   → JumpIfTrue(x, t)
///   Ne(dst, x, 0)  + JumpIfFalse(dst, t)  → JumpIfFalse(x, t)
fn compare_zero_simplify(func: &mut IrFunction) -> bool {
    let mut changed = false;

    // Build a map: vreg id → known constant value (just from LoadImm).
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

    // Build use-count map for comparison destinations to ensure we only
    // eliminate comparisons whose result is used exactly once (by the branch).
    let mut use_count: HashMap<u32, u32> = HashMap::new();
    for instr in &func.body {
        for vreg_id in collect_src_vreg_ids(&instr.op) {
            *use_count.entry(vreg_id).or_insert(0) += 1;
        }
    }

    // Scan pairs of adjacent instructions.
    let mut i = 0;
    while i + 1 < func.body.len() {
        let (cmp_kind, cmp_dst_id, operand, cmp_width) = match &func.body[i].op {
            IrOp::Eq { dst, lhs, rhs, width } => {
                if let Some(&0) = constants.get(&rhs.id) {
                    ("eq", dst.id, *lhs, *width)
                } else if let Some(&0) = constants.get(&lhs.id) {
                    ("eq", dst.id, *rhs, *width)
                } else {
                    i += 1;
                    continue;
                }
            }
            IrOp::Ne { dst, lhs, rhs, width } => {
                if let Some(&0) = constants.get(&rhs.id) {
                    ("ne", dst.id, *lhs, *width)
                } else if let Some(&0) = constants.get(&lhs.id) {
                    ("ne", dst.id, *rhs, *width)
                } else {
                    i += 1;
                    continue;
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };

        // Only simplify if the comparison result is used exactly once (by the
        // following branch).
        if use_count.get(&cmp_dst_id).copied().unwrap_or(0) != 1 {
            i += 1;
            continue;
        }

        let line = func.body[i].line;
        match &func.body[i + 1].op {
            IrOp::JumpIfTrue { cond, target } if cond.id == cmp_dst_id => {
                let target = *target;
                // Eq(x,0) + JumpIfTrue → jump when x==0 → JumpIfFalse(x)
                // Ne(x,0) + JumpIfTrue → jump when x≠0 → JumpIfTrue(x)
                let new_op = if cmp_kind == "eq" {
                    IrOp::JumpIfFalse { cond: VReg::new(operand.id, cmp_width), target }
                } else {
                    IrOp::JumpIfTrue { cond: VReg::new(operand.id, cmp_width), target }
                };
                // Replace comparison with a dead LoadImm placeholder (value=0).
                // This makes cmp_dst_id dead; DCE will remove it in the next pass.
                func.body[i] = IrInstr { op: IrOp::LoadImm { dst: VReg::new(cmp_dst_id, Width::W8), value: 0 }, line };
                func.body[i + 1] = IrInstr { op: new_op, line: func.body[i + 1].line };
                changed = true;
                i += 2;
            }
            IrOp::JumpIfFalse { cond, target } if cond.id == cmp_dst_id => {
                let target = *target;
                // Eq(x,0) + JumpIfFalse → jump when x≠0 → JumpIfTrue(x)
                // Ne(x,0) + JumpIfFalse → jump when x==0 → JumpIfFalse(x)
                let new_op = if cmp_kind == "eq" {
                    IrOp::JumpIfTrue { cond: VReg::new(operand.id, cmp_width), target }
                } else {
                    IrOp::JumpIfFalse { cond: VReg::new(operand.id, cmp_width), target }
                };
                // Replace comparison with a dead LoadImm placeholder (value=0).
                // This makes cmp_dst_id dead; DCE will remove it in the next pass.
                func.body[i] = IrInstr { op: IrOp::LoadImm { dst: VReg::new(cmp_dst_id, Width::W8), value: 0 }, line };
                func.body[i + 1] = IrInstr { op: new_op, line: func.body[i + 1].line };
                changed = true;
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    changed
}

/// Collect source (non-destination) vreg IDs referenced by an IR operation.
/// (Note: the existing `collect_src_vregs` at module scope returns `Vec<u32>`;
///  this local variant returns `Vec<u32>` IDs for use in compare_zero_simplify.)
fn collect_src_vreg_ids(op: &IrOp) -> Vec<u32> {
    // Delegate to the existing function.
    let mut srcs = Vec::new();
    match op {
        IrOp::StoreGlobal { src, .. } | IrOp::StoreLocal { src, .. } => { srcs.push(src.id); }
        IrOp::LoadPtr { ptr, .. } => { srcs.push(ptr.id); }
        IrOp::StorePtr { ptr, src } => { srcs.push(ptr.id); srcs.push(src.id); }
        IrOp::Add { lhs, rhs, .. } | IrOp::Sub { lhs, rhs, .. }
        | IrOp::Mul { lhs, rhs, .. } | IrOp::Div { lhs, rhs, .. }
        | IrOp::Mod { lhs, rhs, .. } | IrOp::And { lhs, rhs, .. }
        | IrOp::Or { lhs, rhs, .. } | IrOp::Xor { lhs, rhs, .. }
        | IrOp::Shl { lhs, rhs, .. } | IrOp::Shr { lhs, rhs, .. }
        | IrOp::Eq { lhs, rhs, .. } | IrOp::Ne { lhs, rhs, .. }
        | IrOp::Lt { lhs, rhs, .. } | IrOp::Le { lhs, rhs, .. }
        | IrOp::Gt { lhs, rhs, .. } | IrOp::Ge { lhs, rhs, .. } => {
            srcs.push(lhs.id);
            srcs.push(rhs.id);
        }
        IrOp::Neg { src, .. } | IrOp::Not { src, .. } | IrOp::LogicalNot { src, .. } => {
            srcs.push(src.id);
        }
        IrOp::Copy { src, .. } | IrOp::Cast { src, .. } => { srcs.push(src.id); }
        IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => { srcs.push(cond.id); }
        IrOp::Call { args, .. } => {
            for a in args { srcs.push(a.id); }
        }
        IrOp::Return { value } => {
            if let Some(v) = value { srcs.push(v.id); }
        }
        IrOp::PtrAdd { ptr, offset, .. } => {
            srcs.push(ptr.id);
            srcs.push(offset.id);
        }
        _ => {}
    }
    srcs
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
// Jump threading
// ---------------------------------------------------------------------------

/// Resolve jump chains at the IR level.
///
/// When a jump targets a label that is immediately followed by another
/// unconditional jump, rewrite the first jump to target the final
/// destination.  Also handles conditional branches.
// ---------------------------------------------------------------------------
// Dead branch elimination
// ---------------------------------------------------------------------------

/// Remove conditional branches whose condition is a compile-time constant.
///
/// After `constant_fold_and_propagate` has run, any `LoadImm` whose destination
/// vreg feeds a `JumpIfTrue`/`JumpIfFalse` gives us a statically-known branch
/// direction:
///
/// * `JumpIfFalse { cond=0 }` → unconditional `Jump` (always taken).
/// * `JumpIfFalse { cond≠0 }` → removed (never taken, fall-through).
/// * `JumpIfTrue  { cond≠0 }` → unconditional `Jump`.
/// * `JumpIfTrue  { cond=0 }` → removed.
///
/// The resulting unreachable instructions between a now-unconditional `Jump`
/// and the next `Label` are cleaned up by the subsequent `dead_code_eliminate`
/// pass.  Any `LoadImm` that solely fed the removed branch condition will then
/// be pruned by that same pass's vreg-use scan.
fn dead_branch_eliminate(func: &mut IrFunction) -> bool {
    // Build a map of vreg id → constant value from LoadImm instructions.
    // After constant_fold_and_propagate all constants are already expressed
    // as LoadImm, so a single linear scan is sufficient.
    let mut constants: HashMap<u32, i64> = HashMap::new();
    for instr in &func.body {
        if let IrOp::LoadImm { dst, value } = &instr.op {
            constants.insert(dst.id, *value);
        }
    }

    // Remove any vreg from the constants map that is also the destination of
    // a non-LoadImm instruction (i.e. it is mutated, e.g. by an Add in a loop
    // increment).  Such vregs are not globally constant.
    for instr in &func.body {
        match &instr.op {
            IrOp::LoadImm { .. } => {}  // the source of the constant — keep
            _ => {
                // Any other instruction that writes a dst invalidates it.
                if let Some(dst) = get_any_dst(&instr.op) {
                    constants.remove(&dst.id);
                }
            }
        }
    }

    if constants.is_empty() {
        return false;
    }

    let mut changed = false;
    let new_body: Vec<IrInstr> = func
        .body
        .iter()
        .map(|instr| {
            match &instr.op {
                IrOp::JumpIfFalse { cond, target } => {
                    if let Some(&val) = constants.get(&cond.id) {
                        changed = true;
                        if val == 0 {
                            // Condition is always false → branch always taken.
                            IrInstr { op: IrOp::Jump { target: *target }, line: instr.line }
                        } else {
                            instr.clone() // never-taken; stripped in second pass
                        }
                    } else {
                        instr.clone()
                    }
                }
                IrOp::JumpIfTrue { cond, target } => {
                    if let Some(&val) = constants.get(&cond.id) {
                        changed = true;
                        if val != 0 {
                            // Condition is always true → branch always taken.
                            IrInstr { op: IrOp::Jump { target: *target }, line: instr.line }
                        } else {
                            instr.clone() // never-taken; stripped in second pass
                        }
                    } else {
                        instr.clone()
                    }
                }
                _ => instr.clone(),
            }
        })
        .collect();

    // Second pass: drop conditional jumps that are never taken.
    let final_body: Vec<IrInstr> = new_body
        .into_iter()
        .filter(|instr| {
            match &instr.op {
                IrOp::JumpIfFalse { cond, .. } => {
                    if let Some(&val) = constants.get(&cond.id) {
                        val == 0  // keep only if it's still the always-taken case
                    } else {
                        true
                    }
                }
                IrOp::JumpIfTrue { cond, .. } => {
                    if let Some(&val) = constants.get(&cond.id) {
                        val != 0
                    } else {
                        true
                    }
                }
                _ => true,
            }
        })
        .collect();

    if func.body.len() != final_body.len() {
        changed = true;
    }
    func.body = final_body;
    changed
}

/// Helper for loop_unrolling: find the Add/Sub in `body[start..end]` that defines
/// `target_id`, returning `(stride, lhs_vreg_id)` if the rhs is a known constant.
fn find_add_stride(
    body: &[IrInstr],
    start: usize,
    end: usize,
    target_id: u32,
    constants: &HashMap<u32, i64>,
) -> Option<(i64, u32)> {
    let add_pos = body[start..end]
        .iter()
        .rposition(|instr| match &instr.op {
            IrOp::Add { dst, .. } | IrOp::Sub { dst, .. } => dst.id == target_id,
            _ => false,
        })?;
    let add_idx = start + add_pos;
    match &body[add_idx].op {
        IrOp::Add { rhs, lhs, .. } => {
            Some((constants.get(&rhs.id).copied()?, lhs.id))
        }
        IrOp::Sub { rhs, lhs, .. } => {
            Some((-constants.get(&rhs.id).copied()?, lhs.id))
        }
        _ => None,
    }
}

fn jump_threading(func: &mut IrFunction) -> bool {
    // Build map: label → index in body
    let mut label_index: HashMap<u32, usize> = HashMap::new();
    for (i, instr) in func.body.iter().enumerate() {
        if let IrOp::Label { label } = &instr.op {
            label_index.insert(label.0, i);
        }
    }

    // For each label, find the first non-label instruction after it.
    // If it's an unconditional Jump, record the forwarding.
    let mut forward: HashMap<u32, u32> = HashMap::new();
    for (&label_id, &idx) in &label_index {
        let mut j = idx + 1;
        while j < func.body.len() {
            match &func.body[j].op {
                IrOp::Label { .. } => { j += 1; }
                IrOp::Jump { target } => {
                    if target.0 != label_id {
                        forward.insert(label_id, target.0);
                    }
                    break;
                }
                _ => break,
            }
        }
    }

    if forward.is_empty() {
        return false;
    }

    // Resolve transitive chains (limit iterations to prevent cycles).
    for _ in 0..16 {
        let mut any = false;
        let snapshot: Vec<(u32, u32)> = forward.iter().map(|(&k, &v)| (k, v)).collect();
        for (src, dst) in &snapshot {
            if let Some(&further) = forward.get(dst) {
                if further != *src {
                    forward.insert(*src, further);
                    any = true;
                }
            }
        }
        if !any { break; }
    }

    // Rewrite jump targets.
    let mut changed = false;
    for instr in &mut func.body {
        match &mut instr.op {
            IrOp::Jump { target } => {
                if let Some(&new_target) = forward.get(&target.0) {
                    target.0 = new_target;
                    changed = true;
                }
            }
            IrOp::JumpIfTrue { target, .. } | IrOp::JumpIfFalse { target, .. } => {
                if let Some(&new_target) = forward.get(&target.0) {
                    target.0 = new_target;
                    changed = true;
                }
            }
            _ => {}
        }
    }

    changed
}

// ---------------------------------------------------------------------------
// Dead function removal
// ---------------------------------------------------------------------------

/// Remove functions that are never called from any remaining function.
///
/// After inlining, some functions may have had all their call sites inlined
/// away.  These functions are dead and should not be emitted.
///
/// Reachability is seeded from every function that is never the target of a
/// `Call` instruction in any other function (i.e., all potential entry points,
/// Remove functions that are never reachable from `main`.
// ---------------------------------------------------------------------------
// Dedup identical function specializations (Step 3)
// ---------------------------------------------------------------------------

/// After function specialization, multiple `__spec_*` variants may end up with
/// identical IR bodies (e.g. when different call-sites supply the same constant
/// value for different argument positions but the specialized bodies simplify to
/// the same code).  This pass compares their bodies structurally (normalizing
/// vreg IDs and labels) and redirects duplicate call-sites to a single copy.
fn dedup_specializations(program: &mut IrProgram) {
    // Collect indices of __spec_* functions.
    let spec_indices: Vec<usize> = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name.contains("__spec_"))
        .map(|(i, _)| i)
        .collect();

    if spec_indices.len() < 2 {
        return;
    }

    // Build a map from normalized body → canonical name (first occurrence).
    let mut canonical: HashMap<Vec<NormalizedOp>, String> = HashMap::new();
    // Map from duplicate name → canonical name.
    let mut redirect: HashMap<String, String> = HashMap::new();

    for &idx in &spec_indices {
        let norm = normalize_body(&program.functions[idx].body);
        let name = program.functions[idx].name.clone();
        if let Some(existing) = canonical.get(&norm) {
            redirect.insert(name, existing.clone());
        } else {
            canonical.insert(norm, name);
        }
    }

    if redirect.is_empty() {
        return;
    }

    // Rewrite all Call instructions that reference duplicates.
    for func in &mut program.functions {
        for instr in &mut func.body {
            if let IrOp::Call { func_name, .. } = &mut instr.op {
                if let Some(canon) = redirect.get(func_name) {
                    *func_name = canon.clone();
                }
            }
        }
    }

    // The duplicate functions are now unreferenced and will be removed by
    // remove_dead_functions().
}

/// A normalized representation of an IR operation where vreg IDs and label IDs
/// are replaced with sequential indices based on first-occurrence order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NormalizedOp {
    /// String representation of the op with normalized IDs.
    repr: String,
}

/// Normalize an IR function body so that structurally identical functions
/// produce the same sequence regardless of absolute vreg/label IDs.
fn normalize_body(body: &[IrInstr]) -> Vec<NormalizedOp> {
    let mut vreg_map: HashMap<u32, u32> = HashMap::new();
    let mut label_map: HashMap<u32, u32> = HashMap::new();
    let mut next_vreg: u32 = 0;
    let mut next_label: u32 = 0;

    let mut map_vreg = |id: u32, vm: &mut HashMap<u32, u32>, nv: &mut u32| -> u32 {
        *vm.entry(id).or_insert_with(|| {
            let r = *nv;
            *nv += 1;
            r
        })
    };

    let mut map_label = |id: u32, lm: &mut HashMap<u32, u32>, nl: &mut u32| -> u32 {
        *lm.entry(id).or_insert_with(|| {
            let r = *nl;
            *nl += 1;
            r
        })
    };

    body.iter()
        .map(|instr| {
            let repr = normalize_op(
                &instr.op,
                &mut vreg_map,
                &mut label_map,
                &mut next_vreg,
                &mut next_label,
                &mut map_vreg,
                &mut map_label,
            );
            NormalizedOp { repr }
        })
        .collect()
}

fn normalize_op(
    op: &IrOp,
    vreg_map: &mut HashMap<u32, u32>,
    label_map: &mut HashMap<u32, u32>,
    next_vreg: &mut u32,
    next_label: &mut u32,
    map_vreg: &mut impl FnMut(u32, &mut HashMap<u32, u32>, &mut u32) -> u32,
    map_label: &mut impl FnMut(u32, &mut HashMap<u32, u32>, &mut u32) -> u32,
) -> String {
    // Helper closures for normalized vreg/label representation.
    let nv = |id: u32, vm: &mut HashMap<u32, u32>, nv: &mut u32, f: &mut dyn FnMut(u32, &mut HashMap<u32, u32>, &mut u32) -> u32| -> String {
        format!("v{}", f(id, vm, nv))
    };
    let nl = |id: u32, lm: &mut HashMap<u32, u32>, nl: &mut u32, f: &mut dyn FnMut(u32, &mut HashMap<u32, u32>, &mut u32) -> u32| -> String {
        format!("L{}", f(id, lm, nl))
    };

    match op {
        IrOp::LoadImm { dst, value } => {
            format!("LoadImm {} {} {:?} {}", nv(dst.id, vreg_map, next_vreg, map_vreg), value, dst.width, value)
        }
        IrOp::LoadGlobal { dst, addr_label } => {
            format!("LoadGlobal {} {:?} {}", nv(dst.id, vreg_map, next_vreg, map_vreg), dst.width, addr_label)
        }
        IrOp::StoreGlobal { addr_label, src } => {
            format!("StoreGlobal {} {} {:?}", addr_label, nv(src.id, vreg_map, next_vreg, map_vreg), src.width)
        }
        IrOp::Add { dst, lhs, rhs, width } => {
            format!("Add {} {} {} {:?}",
                nv(dst.id, vreg_map, next_vreg, map_vreg),
                nv(lhs.id, vreg_map, next_vreg, map_vreg),
                nv(rhs.id, vreg_map, next_vreg, map_vreg),
                width)
        }
        IrOp::Sub { dst, lhs, rhs, width } => {
            format!("Sub {} {} {} {:?}",
                nv(dst.id, vreg_map, next_vreg, map_vreg),
                nv(lhs.id, vreg_map, next_vreg, map_vreg),
                nv(rhs.id, vreg_map, next_vreg, map_vreg),
                width)
        }
        IrOp::Mul { dst, lhs, rhs, width, signed } => {
            format!("Mul {} {} {} {:?} {}",
                nv(dst.id, vreg_map, next_vreg, map_vreg),
                nv(lhs.id, vreg_map, next_vreg, map_vreg),
                nv(rhs.id, vreg_map, next_vreg, map_vreg),
                width, signed)
        }
        IrOp::Copy { dst, src } => {
            format!("Copy {} {}",
                nv(dst.id, vreg_map, next_vreg, map_vreg),
                nv(src.id, vreg_map, next_vreg, map_vreg))
        }
        IrOp::Label { label } => {
            format!("Label {}", nl(label.0, label_map, next_label, map_label))
        }
        IrOp::Jump { target } => {
            format!("Jump {}", nl(target.0, label_map, next_label, map_label))
        }
        IrOp::JumpIfTrue { cond, target } => {
            format!("JumpIfTrue {} {}",
                nv(cond.id, vreg_map, next_vreg, map_vreg),
                nl(target.0, label_map, next_label, map_label))
        }
        IrOp::JumpIfFalse { cond, target } => {
            format!("JumpIfFalse {} {}",
                nv(cond.id, vreg_map, next_vreg, map_vreg),
                nl(target.0, label_map, next_label, map_label))
        }
        IrOp::Return { value } => {
            if let Some(v) = value {
                format!("Return {}", nv(v.id, vreg_map, next_vreg, map_vreg))
            } else {
                "ReturnVoid".to_string()
            }
        }
        IrOp::Call { func_name, args, dst } => {
            let arg_strs: Vec<String> = args.iter().map(|a| nv(a.id, vreg_map, next_vreg, map_vreg)).collect();
            let dst_str = dst.map(|d| nv(d.id, vreg_map, next_vreg, map_vreg)).unwrap_or_default();
            format!("Call {} [{}] {}", func_name, arg_strs.join(","), dst_str)
        }
        // Fallback: use Debug representation.  This will NOT normalize vreg/label
        // IDs, so functions differing only in those IDs won't be deduplicated
        // through this branch.  In practice, uncommon ops (InlineAsm, LoadLocal,
        // StoreLocal, etc.) rarely appear in specialization clones.
        other => format!("{:?}", other),
    }
}

fn remove_dead_functions(program: &mut IrProgram) {
    // Build a call-graph adjacency list (caller → callees) indexed by name.
    let func_map: HashMap<String, usize> = program
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Seed the live set from "main" and walk the call graph.
    let mut live: HashSet<String> = HashSet::new();
    let mut worklist: Vec<String> = vec!["main".to_string()];

    while let Some(name) = worklist.pop() {
        if !live.insert(name.clone()) {
            continue; // already visited
        }
        if let Some(&idx) = func_map.get(&name) {
            for instr in &program.functions[idx].body {
                if let IrOp::Call { func_name, .. } = &instr.op {
                    if !live.contains(func_name) {
                        worklist.push(func_name.clone());
                    }
                }
            }
        }
    }

    program.functions.retain(|f| live.contains(&f.name));
}

// ---------------------------------------------------------------------------
// Inline expansion
// ---------------------------------------------------------------------------

/// Inline small functions at call sites.
///
/// A function is eligible for inlining if:
/// - It has ≤ `INLINE_THRESHOLD` IR instructions.
/// - It is not recursive (doesn't call itself directly or indirectly).
/// - It is not the "main" function.
///
/// When inlined, the call is replaced with:
/// 1. Store arguments to the callee's parameter labels.
/// 2. The callee's body with all vregs and labels remapped to fresh IDs.
/// 3. `Return` instructions replaced by a jump to a merge label.
fn inline_expand(program: &mut IrProgram) {
    // Build a map of function name → index for lookup.
    let func_map: HashMap<String, usize> = program
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Determine which functions are inline candidates.
    let inline_candidates: HashSet<String> = program
        .functions
        .iter()
        .filter(|f| {
            f.name != "main"
                && f.body.len() <= INLINE_THRESHOLD
                && !calls_self(f)
                && !f.is_asm_body
                && !has_inline_asm(f)
        })
        .map(|f| f.name.clone())
        .collect();

    if inline_candidates.is_empty() {
        return;
    }

    // We need a global vreg id counter and label id counter to avoid clashes.
    let mut next_vreg_id: u32 = program
        .functions
        .iter()
        .flat_map(|f| f.body.iter())
        .filter_map(|instr| get_dst_vreg(&instr.op))
        .map(|v| v.id + 1)
        .max()
        .unwrap_or(0);

    let mut next_label_id: u32 = program
        .functions
        .iter()
        .flat_map(|f| f.body.iter())
        .filter_map(|instr| {
            if let IrOp::Label { label } = &instr.op {
                Some(label.0 + 1)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);

    // Clone candidates for inlining (to avoid borrow issues).
    let callee_bodies: HashMap<String, IrFunction> = inline_candidates
        .iter()
        .filter_map(|name| {
            func_map.get(name).map(|&i| (name.clone(), program.functions[i].clone()))
        })
        .collect();

    // Record which inline candidates actually had at least one call site.
    // Only these can become "fully inlined away" — a function with no original
    // callers is an entry point and must NOT be removed.
    let had_callers: HashSet<String> = program
        .functions
        .iter()
        .flat_map(|f| f.body.iter())
        .filter_map(|instr| {
            if let IrOp::Call { func_name, .. } = &instr.op {
                if inline_candidates.contains(func_name) {
                    Some(func_name.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    // Process each function and inline call sites.
    for func in &mut program.functions {
        let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());
        let mut did_inline = false;

        for instr in &func.body {
            if let IrOp::Call { func_name, args, dst } = &instr.op {
                if let Some(callee) = callee_bodies.get(func_name) {
                    // Inline this call.
                    let (inlined, nv, nl) = inline_call_site(
                        callee, args, dst.as_ref(), next_vreg_id, next_label_id,
                    );
                    next_vreg_id = nv;
                    next_label_id = nl;
                    new_body.extend(inlined);
                    did_inline = true;
                    continue;
                }
            }
            new_body.push(instr.clone());
        }

        if did_inline {
            func.body = new_body;
        }
    }

    // Remove inline candidates that now have no remaining call sites.
    // After inlining, a small function that was fully inlined everywhere has
    // no more callers and should not be emitted.  We only remove functions
    // that were in inline_candidates (small, non-recursive) — larger functions
    // (not eligible for inlining) are intentionally kept regardless of whether
    // they are still called, because their removal may be surprising or break
    // external linkage assumptions.
    let still_called: HashSet<String> = program
        .functions
        .iter()
        .flat_map(|f| f.body.iter())
        .filter_map(|instr| {
            if let IrOp::Call { func_name, .. } = &instr.op {
                Some(func_name.clone())
            } else {
                None
            }
        })
        .collect();
    program.functions.retain(|f| {
        !inline_candidates.contains(&f.name)
            || !had_callers.contains(&f.name)
            || still_called.contains(&f.name)
    });
}

/// Check if a function calls itself (direct recursion).
fn calls_self(func: &IrFunction) -> bool {
    func.body.iter().any(|instr| {
        matches!(&instr.op, IrOp::Call { func_name, .. } if func_name == &func.name)
    })
}

/// Check if a function contains any raw inline asm blocks.
fn has_inline_asm(func: &IrFunction) -> bool {
    func.body.iter().any(|instr| matches!(&instr.op, IrOp::InlineAsm { .. }))
}

/// Get the destination vreg of any instruction (for computing max vreg ids).
fn get_dst_vreg(op: &IrOp) -> Option<VReg> {
    match op {
        IrOp::LoadImm { dst, .. }
        | IrOp::LoadGlobal { dst, .. }
        | IrOp::LoadLocal { dst, .. }
        | IrOp::LoadPtr { dst, .. }
        | IrOp::Add { dst, .. }
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
        IrOp::Call { dst, .. } => *dst,
        _ => None,
    }
}

/// Inline a single call site: produce a sequence of instructions that
/// replaces the Call instruction.
///
/// Returns (instructions, next_vreg_id, next_label_id).
fn inline_call_site(
    callee: &IrFunction,
    call_args: &[VReg],
    call_dst: Option<&VReg>,
    mut next_vreg: u32,
    mut next_label: u32,
) -> (Vec<IrInstr>, u32, u32) {
    let mut result = Vec::new();

    // Build vreg remapping: callee vreg id → new vreg id.
    let mut vreg_map: HashMap<u32, u32> = HashMap::new();

    // Build label remapping.
    let mut label_map: HashMap<u32, u32> = HashMap::new();

    // Merge label: where Return instructions in the callee jump to.
    let merge_label = Label::new(next_label);
    next_label += 1;

    // Vreg to hold the return value (if any).
    let ret_vreg = call_dst.map(|d| {
        let rv = VReg::new(next_vreg, d.width);
        next_vreg += 1;
        rv
    });

    // Step 1: Seed vreg_map with callee param vregs → caller arg vregs.
    // After load_store_forwarding runs on the callee, LoadGlobal _l_param
    // instructions become Copy vreg_x, vreg_param, so the body may reference
    // the parameter vreg directly.  If we don't pre-seed the mapping those
    // references get fresh ids disconnected from the actual argument values,
    // blocking constant propagation.
    for (i, param) in callee.params.iter().enumerate() {
        if i < call_args.len() {
            vreg_map.insert(param.vreg.id, call_args[i].id);
        }
    }

    // Step 2: Store arguments to callee's parameter labels.
    // This keeps correctness for any path where the callee body still accesses
    // the parameter via LoadGlobal _l_callee_param (e.g. unoptimised callees or
    // callees whose param slot is written to inside the body).
    let param_labels: std::collections::HashSet<String> = callee
        .params
        .iter()
        .map(|p| format!("_l_{}_{}", callee.name, p.name))
        .collect();

    for (i, param) in callee.params.iter().enumerate() {
        if i < call_args.len() {
            let label = format!("_l_{}_{}", callee.name, param.name);
            result.push(IrInstr::bare(IrOp::StoreGlobal {
                addr_label: label,
                src: call_args[i],
            }));
        }
    }

    // Step 3: Copy the callee body with remapped vregs/labels.
    // Skip leading StoreGlobal instructions for parameter labels (the param
    // preamble).  We detect this dynamically instead of using a fixed
    // skip_prefix count because redundant_store_eliminate may have already
    // removed some or all of those stores from the callee before we get here.
    let body_start = {
        let mut n = 0;
        for instr in &callee.body {
            if let IrOp::StoreGlobal { addr_label, .. } = &instr.op {
                if param_labels.contains(addr_label) {
                    n += 1;
                    continue;
                }
            }
            break;
        }
        n
    };
    for instr in callee.body.iter().skip(body_start) {
        // Handle Return specially: it needs to produce Copy + Jump.
        if let IrOp::Return { value } = &instr.op {
            if let (Some(v), Some(rv_dst)) = (value, ret_vreg) {
                let new_id = *vreg_map.entry(v.id).or_insert_with(|| {
                    let id = next_vreg;
                    next_vreg += 1;
                    id
                });
                let remapped_src = VReg::new(new_id, v.width);
                result.push(IrInstr::bare(IrOp::Copy { dst: rv_dst, src: remapped_src }));
            }
            result.push(IrInstr::bare(IrOp::Jump { target: merge_label }));
            continue;
        }
        let new_op = remap_op(
            &instr.op,
            &mut vreg_map,
            &mut label_map,
            &mut next_vreg,
            &mut next_label,
        );
        result.push(IrInstr { op: new_op, line: instr.line });
    }

    // Step 4: Emit merge label.
    result.push(IrInstr::bare(IrOp::Label { label: merge_label }));

    // Step 5: Copy return value to the call's destination.
    if let (Some(dst), Some(rv)) = (call_dst, ret_vreg) {
        result.push(IrInstr::bare(IrOp::Copy { dst: *dst, src: rv }));
    }

    (result, next_vreg, next_label)
}

/// Remap all vreg and label references in an IR operation.
/// `Return` is handled separately in the caller.
fn remap_op(
    op: &IrOp,
    vreg_map: &mut HashMap<u32, u32>,
    label_map: &mut HashMap<u32, u32>,
    next_vreg: &mut u32,
    next_label: &mut u32,
) -> IrOp {
    // Helper closures
    let mut rv = |v: VReg| -> VReg {
        let new_id = *vreg_map.entry(v.id).or_insert_with(|| {
            let id = *next_vreg;
            *next_vreg += 1;
            id
        });
        VReg::new(new_id, v.width)
    };
    let mut rl = |l: Label| -> Label {
        let new_id = *label_map.entry(l.0).or_insert_with(|| {
            let id = *next_label;
            *next_label += 1;
            id
        });
        Label::new(new_id)
    };

    match op {
        IrOp::LoadImm { dst, value } => IrOp::LoadImm { dst: rv(*dst), value: *value },
        IrOp::LoadGlobal { dst, addr_label } => IrOp::LoadGlobal { dst: rv(*dst), addr_label: addr_label.clone() },
        IrOp::StoreGlobal { addr_label, src } => IrOp::StoreGlobal { addr_label: addr_label.clone(), src: rv(*src) },
        IrOp::LoadLocal { dst, offset } => IrOp::LoadLocal { dst: rv(*dst), offset: *offset },
        IrOp::StoreLocal { offset, src } => IrOp::StoreLocal { offset: *offset, src: rv(*src) },
        IrOp::LoadPtr { dst, ptr } => IrOp::LoadPtr { dst: rv(*dst), ptr: rv(*ptr) },
        IrOp::StorePtr { ptr, src } => IrOp::StorePtr { ptr: rv(*ptr), src: rv(*src) },
        IrOp::Add { dst, lhs, rhs, width } => IrOp::Add { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Sub { dst, lhs, rhs, width } => IrOp::Sub { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Mul { dst, lhs, rhs, width, signed } => IrOp::Mul { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Div { dst, lhs, rhs, width, signed } => IrOp::Div { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Mod { dst, lhs, rhs, width, signed } => IrOp::Mod { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::And { dst, lhs, rhs, width } => IrOp::And { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Or { dst, lhs, rhs, width } => IrOp::Or { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Xor { dst, lhs, rhs, width } => IrOp::Xor { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Shl { dst, lhs, rhs, width } => IrOp::Shl { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Shr { dst, lhs, rhs, width, arithmetic } => IrOp::Shr { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, arithmetic: *arithmetic },
        IrOp::Eq { dst, lhs, rhs, width } => IrOp::Eq { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Ne { dst, lhs, rhs, width } => IrOp::Ne { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width },
        IrOp::Lt { dst, lhs, rhs, width, signed } => IrOp::Lt { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Le { dst, lhs, rhs, width, signed } => IrOp::Le { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Gt { dst, lhs, rhs, width, signed } => IrOp::Gt { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Ge { dst, lhs, rhs, width, signed } => IrOp::Ge { dst: rv(*dst), lhs: rv(*lhs), rhs: rv(*rhs), width: *width, signed: *signed },
        IrOp::Neg { dst, src, width } => IrOp::Neg { dst: rv(*dst), src: rv(*src), width: *width },
        IrOp::Not { dst, src, width } => IrOp::Not { dst: rv(*dst), src: rv(*src), width: *width },
        IrOp::LogicalNot { dst, src, width } => IrOp::LogicalNot { dst: rv(*dst), src: rv(*src), width: *width },
        IrOp::Copy { dst, src } => IrOp::Copy { dst: rv(*dst), src: rv(*src) },
        IrOp::Cast { dst, src, to_type } => IrOp::Cast { dst: rv(*dst), src: rv(*src), to_type: to_type.clone() },
        IrOp::Jump { target } => IrOp::Jump { target: rl(*target) },
        IrOp::JumpIfTrue { cond, target } => IrOp::JumpIfTrue { cond: rv(*cond), target: rl(*target) },
        IrOp::JumpIfFalse { cond, target } => IrOp::JumpIfFalse { cond: rv(*cond), target: rl(*target) },
        IrOp::Label { label } => IrOp::Label { label: rl(*label) },
        IrOp::AddrOfGlobal { dst, name } => IrOp::AddrOfGlobal { dst: rv(*dst), name: name.clone() },
        IrOp::PtrAdd { dst, ptr, offset, element_size } => IrOp::PtrAdd { dst: rv(*dst), ptr: rv(*ptr), offset: rv(*offset), element_size: *element_size },
        IrOp::Return { .. } => unreachable!("Return handled in caller"),
        IrOp::Call { func_name, args, dst } => IrOp::Call {
            func_name: func_name.clone(),
            args: args.iter().map(|a| rv(*a)).collect(),
            dst: dst.map(|d| rv(d)),
        },
        IrOp::InlineAsm { code, inputs, return_type, clobber_all } => IrOp::InlineAsm {
            code: code.clone(),
            inputs: inputs.iter().map(|(v, t)| (rv(*v), t.clone())).collect(),
            return_type: return_type.clone(),
            clobber_all: *clobber_all,
        },
    }
}

// ---------------------------------------------------------------------------
// Loop-invariant code motion (LICM)
// ---------------------------------------------------------------------------

/// Detect natural loops and hoist loop-invariant instructions to the
/// pre-header position (just before the loop header label).
///
/// A natural loop is identified by a back-edge: an unconditional or
/// conditional jump whose target label appears *before* the jump in the
/// linear instruction stream.  The loop body spans from the header label
/// up to (and including) the back-edge jump.
///
/// An instruction is loop-invariant if:
///  - It is "pure" (no side effects: no stores, calls, jumps, labels, returns).
///  - All of its source operands are defined *outside* the current loop body.
///
/// Such instructions are moved to the pre-header, i.e. just before the
/// header label.
fn loop_invariant_code_motion(func: &mut IrFunction) -> bool {
    // Build label → index map.
    let mut label_index: HashMap<u32, usize> = HashMap::new();
    for (i, instr) in func.body.iter().enumerate() {
        if let IrOp::Label { label } = &instr.op {
            label_index.insert(label.0, i);
        }
    }

    // Find back edges: instructions that jump to a label appearing earlier.
    // Each back edge identifies a natural loop: header..=back_edge.
    struct LoopInfo {
        header_idx: usize,
        back_edge_idx: usize,
    }
    let mut loops: Vec<LoopInfo> = Vec::new();

    for (i, instr) in func.body.iter().enumerate() {
        let target_id = match &instr.op {
            IrOp::Jump { target } => Some(target.0),
            IrOp::JumpIfTrue { target, .. } => Some(target.0),
            // JumpIfFalse is the loop-exit branch (while/for), not a back-edge.
            _ => None,
        };
        if let Some(tid) = target_id {
            if let Some(&header_idx) = label_index.get(&tid) {
                if header_idx < i {
                    loops.push(LoopInfo { header_idx, back_edge_idx: i });
                }
            }
        }
    }

    if loops.is_empty() {
        return false;
    }

    // Process loops from innermost (smallest span) first.
    loops.sort_by_key(|l| l.back_edge_idx - l.header_idx);

    let mut changed = false;

    for lp in &loops {
        // Collect all vregs *defined* inside the loop body.
        let mut loop_defs: HashSet<u32> = HashSet::new();
        for idx in lp.header_idx..=lp.back_edge_idx {
            if let Some(dst) = get_dst_vreg(&func.body[idx].op) {
                loop_defs.insert(dst.id);
            }
        }

        // Identify loop-invariant instructions: pure instructions whose
        // source operands are ALL defined outside the loop.
        let mut hoist_indices: Vec<usize> = Vec::new();
        for idx in lp.header_idx..=lp.back_edge_idx {
            let op = &func.body[idx].op;
            // Only hoist pure instructions (no side effects).
            if get_pure_dst(op).is_none() {
                continue;
            }
            // Never hoist bare LoadImm instructions.  They have an empty
            // source list, so the loop-invariance check is vacuously true,
            // but hoisting them is pointless (constants are free to
            // rematerialise) and harmful: placing a LoadImm before the loop
            // header label prevents constant_fold_and_propagate from folding
            // uses inside the loop (the label clears the constant map), so
            // the loop body materialises a fresh copy via a different register
            // while the hoisted one becomes dead in the assembly output.
            if matches!(op, IrOp::LoadImm { .. }) {
                continue;
            }
            // Check that all source operands are defined outside the loop.
            let sources = collect_src_vregs(op);
            if sources.iter().all(|s| !loop_defs.contains(s)) {
                hoist_indices.push(idx);
                // Remove this definition from loop_defs since we're
                // hoisting it — this allows dependent invariant instructions
                // to be hoisted too in subsequent iterations.
                if let Some(dst) = get_dst_vreg(op) {
                    loop_defs.remove(&dst.id);
                }
            }
        }

        if hoist_indices.is_empty() {
            continue;
        }

        // Build new body: insert hoisted instructions just before the header
        // label, and remove them from their original positions.
        let hoist_set: HashSet<usize> = hoist_indices.iter().copied().collect();
        let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());

        for (i, instr) in func.body.iter().enumerate() {
            if i == lp.header_idx {
                // Insert hoisted instructions before the header label.
                for &hi in &hoist_indices {
                    new_body.push(func.body[hi].clone());
                }
            }
            if !hoist_set.contains(&i) {
                new_body.push(instr.clone());
            }
        }

        func.body = new_body;
        changed = true;

        // Note: after modifying func.body, indices for subsequent loops are
        // invalidated, but since we're in a fixed-point loop in
        // optimize_function, we'll re-detect loops next iteration.
        break;
    }

    changed
}

/// Collect all vreg IDs that are *read* (source operands) by an instruction.
fn collect_src_vregs(op: &IrOp) -> Vec<u32> {
    let mut srcs = Vec::new();
    match op {
        IrOp::LoadImm { .. } | IrOp::LoadGlobal { .. } | IrOp::LoadLocal { .. }
        | IrOp::Label { .. } | IrOp::Jump { .. } | IrOp::AddrOfGlobal { .. } => {}
        IrOp::StoreGlobal { src, .. } | IrOp::StoreLocal { src, .. } => { srcs.push(src.id); }
        IrOp::LoadPtr { ptr, .. } => { srcs.push(ptr.id); }
        IrOp::StorePtr { ptr, src } => { srcs.push(ptr.id); srcs.push(src.id); }
        IrOp::Add { lhs, rhs, .. } | IrOp::Sub { lhs, rhs, .. }
        | IrOp::Mul { lhs, rhs, .. } | IrOp::Div { lhs, rhs, .. }
        | IrOp::Mod { lhs, rhs, .. } | IrOp::And { lhs, rhs, .. }
        | IrOp::Or { lhs, rhs, .. } | IrOp::Xor { lhs, rhs, .. }
        | IrOp::Shl { lhs, rhs, .. } | IrOp::Shr { lhs, rhs, .. }
        | IrOp::Eq { lhs, rhs, .. } | IrOp::Ne { lhs, rhs, .. }
        | IrOp::Lt { lhs, rhs, .. } | IrOp::Le { lhs, rhs, .. }
        | IrOp::Gt { lhs, rhs, .. } | IrOp::Ge { lhs, rhs, .. } => {
            srcs.push(lhs.id);
            srcs.push(rhs.id);
        }
        IrOp::Neg { src, .. } | IrOp::Not { src, .. } | IrOp::LogicalNot { src, .. } => {
            srcs.push(src.id);
        }
        IrOp::Copy { src, .. } | IrOp::Cast { src, .. } => { srcs.push(src.id); }
        IrOp::JumpIfTrue { cond, .. } | IrOp::JumpIfFalse { cond, .. } => { srcs.push(cond.id); }
        IrOp::Call { args, .. } => {
            for a in args { srcs.push(a.id); }
        }
        IrOp::Return { value } => {
            if let Some(v) = value { srcs.push(v.id); }
        }
        IrOp::PtrAdd { ptr, offset, .. } => {
            srcs.push(ptr.id);
            srcs.push(offset.id);
        }
        IrOp::InlineAsm { inputs, .. } => {
            for (v, _) in inputs { srcs.push(v.id); }
        }
    }
    srcs
}

// ---------------------------------------------------------------------------
// Induction variable optimization
// ---------------------------------------------------------------------------

/// Maximum number of loop body instructions we consider for unrolling.
const UNROLL_MAX_BODY: usize = 30;

/// Maximum static trip count for full unrolling.
const UNROLL_MAX_TRIPS: u64 = 16;

/// Detect basic induction variables of the form `iv = iv + C` (or
/// `iv = iv - C`) and replace loop-body multiply expressions that depend
/// on the IV with a derived induction variable incremented by `C * K` each
/// iteration.
///
/// Example:
/// ```text
///   // before                   // after
///   iv = iv + 1                 iv = iv + 1
///   t = iv * 10                 div = div + 10   // derived IV
///                               t = copy div
/// ```
///
/// This eliminates expensive multiplications inside tight loops.
fn induction_variable_optimization(func: &mut IrFunction) -> bool {
    // --- Detect natural loops (same algorithm as LICM) ---
    let mut label_index: HashMap<u32, usize> = HashMap::new();
    for (i, instr) in func.body.iter().enumerate() {
        if let IrOp::Label { label } = &instr.op {
            label_index.insert(label.0, i);
        }
    }

    struct LoopRange {
        header_idx: usize,
        back_edge_idx: usize,
    }

    let mut loops: Vec<LoopRange> = Vec::new();
    for (i, instr) in func.body.iter().enumerate() {
        let target_id = match &instr.op {
            IrOp::Jump { target } => Some(target.0),
            IrOp::JumpIfTrue { target, .. } => Some(target.0),
            _ => None,
        };
        if let Some(tid) = target_id {
            if let Some(&header_idx) = label_index.get(&tid) {
                if header_idx < i {
                    loops.push(LoopRange { header_idx, back_edge_idx: i });
                }
            }
        }
    }

    if loops.is_empty() {
        return false;
    }

    // Sort innermost first.
    loops.sort_by_key(|l| l.back_edge_idx - l.header_idx);

    let mut changed = false;

    for lp in &loops {
        // Collect known constants defined *before* the loop.
        let mut constants: HashMap<u32, i64> = HashMap::new();
        for idx in 0..lp.header_idx {
            if let IrOp::LoadImm { dst, value } = &func.body[idx].op {
                constants.insert(dst.id, *value);
            }
        }

        // --- Detect basic induction variables ---
        // An IV is a vreg `iv` such that there is exactly one definition of
        // `iv` inside the loop, and that definition is `iv = iv + C` or
        // `iv = iv - C` where C is a loop-invariant constant.
        struct BasicIV {
            vreg_id: u32,
            stride: i64,    // +C or -C
            width: Width,
            def_idx: usize, // index of the Add/Sub instruction
        }

        let mut basic_ivs: Vec<BasicIV> = Vec::new();

        // Count definitions of each vreg inside the loop.
        let mut def_counts: HashMap<u32, usize> = HashMap::new();
        for idx in lp.header_idx..=lp.back_edge_idx {
            if let Some(dst) = get_dst_vreg(&func.body[idx].op) {
                *def_counts.entry(dst.id).or_insert(0) += 1;
            }
        }

        for idx in lp.header_idx..=lp.back_edge_idx {
            match &func.body[idx].op {
                IrOp::Add { dst, lhs, rhs, width } => {
                    // iv = iv + C  or  iv = C + iv
                    if dst.id == lhs.id && def_counts.get(&dst.id) == Some(&1) {
                        if let Some(&c) = constants.get(&rhs.id) {
                            basic_ivs.push(BasicIV {
                                vreg_id: dst.id,
                                stride: c,
                                width: *width,
                                def_idx: idx,
                            });
                        }
                    } else if dst.id == rhs.id && def_counts.get(&dst.id) == Some(&1) {
                        if let Some(&c) = constants.get(&lhs.id) {
                            basic_ivs.push(BasicIV {
                                vreg_id: dst.id,
                                stride: c,
                                width: *width,
                                def_idx: idx,
                            });
                        }
                    }
                }
                IrOp::Sub { dst, lhs, rhs, width } => {
                    // iv = iv - C
                    if dst.id == lhs.id && def_counts.get(&dst.id) == Some(&1) {
                        if let Some(&c) = constants.get(&rhs.id) {
                            basic_ivs.push(BasicIV {
                                vreg_id: dst.id,
                                stride: -c,
                                width: *width,
                                def_idx: idx,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if basic_ivs.is_empty() {
            continue;
        }

        // --- Find multiply expressions using the IV and replace them ---
        // Look for `t = iv * K` or `t = K * iv` where K is a loop constant.
        // Replace with a derived IV: `div += stride * K` each iteration,
        // plus an initialisation `div = iv_init * K` before the loop.

        // We need to know the maximum vreg id so we can allocate fresh ones.
        let mut max_vreg_id: u32 = 0;
        for instr in &func.body {
            if let Some(dst) = get_dst_vreg(&instr.op) {
                if dst.id >= max_vreg_id {
                    max_vreg_id = dst.id + 1;
                }
            }
            for s in collect_src_vregs(&instr.op) {
                if s >= max_vreg_id {
                    max_vreg_id = s + 1;
                }
            }
        }

        struct DerivedIV {
            mul_idx: usize,     // index of the Mul instruction inside the loop
            mul_dst: VReg,      // original destination of the Mul
            iv_vreg_id: u32,    // the basic IV vreg id
            factor: i64,        // the constant K
            derived_vreg: VReg, // new vreg for the derived IV
            stride_vreg: VReg,  // new vreg for `stride * K` constant
            init_vreg: VReg,    // vreg for initial value of derived IV
            width: Width,
        }

        let mut derived_ivs: Vec<DerivedIV> = Vec::new();

        for idx in lp.header_idx..=lp.back_edge_idx {
            if let IrOp::Mul { dst, lhs, rhs, width, .. } = &func.body[idx].op {
                for biv in &basic_ivs {
                    let factor = if lhs.id == biv.vreg_id {
                        constants.get(&rhs.id).copied()
                    } else if rhs.id == biv.vreg_id {
                        constants.get(&lhs.id).copied()
                    } else {
                        None
                    };
                    if let Some(k) = factor {
                        let derived_vreg = VReg::new(max_vreg_id, *width);
                        max_vreg_id += 1;
                        let stride_vreg = VReg::new(max_vreg_id, *width);
                        max_vreg_id += 1;
                        let init_vreg = VReg::new(max_vreg_id, *width);
                        max_vreg_id += 1;

                        derived_ivs.push(DerivedIV {
                            mul_idx: idx,
                            mul_dst: *dst,
                            iv_vreg_id: biv.vreg_id,
                            factor: k,
                            derived_vreg,
                            stride_vreg,
                            init_vreg,
                            width: *width,
                        });
                        break;
                    }
                }
            }
        }

        if derived_ivs.is_empty() {
            continue;
        }

        // Now build the transformed body.
        let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len() + derived_ivs.len() * 4);

        let mul_indices: HashSet<usize> = derived_ivs.iter().map(|d| d.mul_idx).collect();

        for (i, instr) in func.body.iter().enumerate() {
            // Before the header label, insert the derived IV initialisations.
            if i == lp.header_idx {
                for div in &derived_ivs {
                    // Load the stride constant: stride * factor
                    let biv = basic_ivs.iter().find(|b| b.vreg_id == div.iv_vreg_id).unwrap();
                    let stride_val = wrap_result(biv.stride * div.factor, div.width);
                    new_body.push(IrInstr::bare(IrOp::LoadImm {
                        dst: div.stride_vreg,
                        value: stride_val,
                    }));
                    // Initialise derived IV: div_vreg = iv_init * K.
                    // The initial value of the IV is whatever was loaded into
                    // iv_vreg before the loop.  We express this as a Mul that
                    // constant-folding will handle in a subsequent pass.
                    // For simplicity, we emit Copy of the init_vreg which
                    // we'll set by computing iv * K.
                    // Actually, let's just emit a Mul here — it executes once
                    // (before the loop), so it's fine.  Constant folding will
                    // clean it up if the init value is known.
                    // We need a vreg holding K.
                    let k_vreg = VReg::new(max_vreg_id, div.width);
                    max_vreg_id += 1;
                    new_body.push(IrInstr::bare(IrOp::LoadImm {
                        dst: k_vreg,
                        value: wrap_result(div.factor, div.width),
                    }));
                    new_body.push(IrInstr::bare(IrOp::Mul {
                        dst: div.derived_vreg,
                        lhs: VReg::new(div.iv_vreg_id, div.width),
                        rhs: k_vreg,
                        width: div.width,
                        signed: true,
                    }));
                }
            }

            // Replace Mul with Copy from derived IV, then add the increment.
            if mul_indices.contains(&i) {
                let div = derived_ivs.iter().find(|d| d.mul_idx == i).unwrap();
                // t = copy derived_vreg
                new_body.push(IrInstr::bare(IrOp::Copy {
                    dst: div.mul_dst,
                    src: div.derived_vreg,
                }));
                continue;
            }

            new_body.push(instr.clone());

            // After the IV increment instruction, add the derived IV increment.
            for div in &derived_ivs {
                let biv = basic_ivs.iter().find(|b| b.vreg_id == div.iv_vreg_id).unwrap();
                if i == biv.def_idx {
                    new_body.push(IrInstr::bare(IrOp::Add {
                        dst: div.derived_vreg,
                        lhs: div.derived_vreg,
                        rhs: div.stride_vreg,
                        width: div.width,
                    }));
                }
            }
        }

        func.body = new_body;
        changed = true;
        break; // Indices invalidated; re-enter via fixed-point loop.
    }

    changed
}

// ---------------------------------------------------------------------------
// Loop unrolling
// ---------------------------------------------------------------------------

/// Unroll small loops whose trip count is statically known and small.
///
/// Recognises the canonical pattern emitted by the IR generator for
/// counted loops:
///
/// ```text
///   iv = LoadImm init
///   ...
/// L_header:
///   cond = Lt/Le/Gt/Ge iv, limit
///   JumpIfFalse cond, L_exit
///   <loop body>
///   iv = Add iv, stride
///   Jump L_header
/// L_exit:
/// ```
///
/// If the trip count is ≤ [`UNROLL_MAX_TRIPS`] and the body size is
/// ≤ [`UNROLL_MAX_BODY`] instructions, the loop is fully unrolled into
/// straight-line code.
fn loop_unrolling(func: &mut IrFunction) -> bool {
    if func.unroll_loop_headers.is_empty() {
        return false;
    }

    let unroll_headers: HashSet<u32> = func
        .unroll_loop_headers
        .iter()
        .map(|l| l.0)
        .collect();

    // --- Detect natural loops ---
    let mut label_index: HashMap<u32, usize> = HashMap::new();
    for (i, instr) in func.body.iter().enumerate() {
        if let IrOp::Label { label } = &instr.op {
            label_index.insert(label.0, i);
        }
    }

    struct LoopCandidate {
        header_idx: usize,     // index of the Label instruction
        back_edge_idx: usize,  // index of the Jump back to header
    }

    let mut loops: Vec<LoopCandidate> = Vec::new();
    for (i, instr) in func.body.iter().enumerate() {
        // Only unconditional back-edges form canonical counted loops.
        if let IrOp::Jump { target } = &instr.op {
            if let Some(&header_idx) = label_index.get(&target.0) {
                if header_idx < i {
                    loops.push(LoopCandidate { header_idx, back_edge_idx: i });
                }
            }
        }
    }

    if loops.is_empty() {
        return false;
    }

    // Sort innermost first.
    loops.sort_by_key(|l| l.back_edge_idx - l.header_idx);

    // Collect all known constants.
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

    let mut changed = false;

    for lp in &loops {
        let header_label_id = match &func.body[lp.header_idx].op {
            IrOp::Label { label } => label.0,
            _ => continue,
        };
        if !unroll_headers.contains(&header_label_id) {
            continue;
        }

        let span = lp.back_edge_idx - lp.header_idx;
        if span < 3 {
            continue; // Too small to be a real loop.
        }

        // --- Find the exit JumpIfFalse by scanning from header+1 ---
        // The header block may contain LoadLocal/LoadImm before the comparison,
        // so we cannot assume a fixed offset.
        let exit_idx = match func.body[lp.header_idx + 1..lp.back_edge_idx]
            .iter()
            .position(|i| matches!(&i.op, IrOp::JumpIfFalse { .. }))
        {
            Some(rel) => lp.header_idx + 1 + rel,
            None => continue,
        };

        let (cond_id, exit_label) = match &func.body[exit_idx].op {
            IrOp::JumpIfFalse { cond, target } => (cond.id, target.0),
            _ => continue,
        };

        // --- Find the comparison that defines cond_id ---
        // Search backward from exit_idx within the header block.
        let cmp_idx = match func.body[lp.header_idx + 1..exit_idx]
            .iter()
            .rposition(|i| matches!(&i.op,
                IrOp::Lt { .. } | IrOp::Le { .. } | IrOp::Gt { .. } | IrOp::Ge { .. }))
        {
            Some(rel) => lp.header_idx + 1 + rel,
            None => continue,
        };

        // Parse the comparison; limit must be a known constant.
        let (iv_vreg_id, limit_val, is_lt, is_signed) = match &func.body[cmp_idx].op {
            IrOp::Lt { dst, lhs, rhs, signed, .. } if dst.id == cond_id => {
                if let Some(&lim) = constants.get(&rhs.id) { (lhs.id, lim, true, *signed) }
                else { continue; }
            }
            IrOp::Le { dst, lhs, rhs, signed, .. } if dst.id == cond_id => {
                if let Some(&lim) = constants.get(&rhs.id) { (lhs.id, lim + 1, true, *signed) }
                else { continue; }
            }
            IrOp::Gt { dst, lhs, rhs, signed, .. } if dst.id == cond_id => {
                if let Some(&lim) = constants.get(&rhs.id) { (lhs.id, lim, false, *signed) }
                else { continue; }
            }
            IrOp::Ge { dst, lhs, rhs, signed, .. } if dst.id == cond_id => {
                if let Some(&lim) = constants.get(&rhs.id) { (lhs.id, lim - 1, false, *signed) }
                else { continue; }
            }
            _ => continue,
        };

        // --- Determine IV initial value ---
        // Case A: iv_vreg_id is a LoadImm constant (SSA-style IV).
        // Case B: iv_vreg_id is a LoadLocal result (stack-local IV).
        // Case C: iv_vreg_id is a LoadGlobal result (local promoted to global).

        enum IvKind { Ssa, Local(i32), Global(String) }

        let (init_val, iv_kind): (i64, IvKind) =
            if let Some(&v) = constants.get(&iv_vreg_id) {
                // SSA case: verify the LoadImm is before the loop header.
                let defined_before = func.body[..lp.header_idx]
                    .iter()
                    .any(|instr| matches!(&instr.op, IrOp::LoadImm { dst, .. } if dst.id == iv_vreg_id));
                if !defined_before { continue; }
                (v, IvKind::Ssa)
            } else {
                // Memory-resident case: iv_vreg_id must come from a load in the
                // header block (between header label and comparison).
                let source = func.body[lp.header_idx + 1..cmp_idx]
                    .iter()
                    .rev()
                    .find_map(|instr| match &instr.op {
                        IrOp::LoadLocal { dst, offset } if dst.id == iv_vreg_id =>
                            Some(IvKind::Local(*offset)),
                        IrOp::LoadGlobal { dst, addr_label } if dst.id == iv_vreg_id =>
                            Some(IvKind::Global(addr_label.clone())),
                        _ => None,
                    });
                match source {
                    None => continue,
                    Some(IvKind::Local(offset)) => {
                        let init = func.body[..lp.header_idx]
                            .iter().rev()
                            .find_map(|instr| {
                                if let IrOp::StoreLocal { offset: o, src } = &instr.op {
                                    if *o == offset { return constants.get(&src.id).copied(); }
                                }
                                None
                            });
                        match init { Some(v) => (v, IvKind::Local(offset)), None => continue }
                    }
                    Some(IvKind::Global(ref label)) => {
                        let label = label.clone();
                        let init = func.body[..lp.header_idx]
                            .iter().rev()
                            .find_map(|instr| {
                                if let IrOp::StoreGlobal { addr_label, src } = &instr.op {
                                    if addr_label == &label { return constants.get(&src.id).copied(); }
                                }
                                None
                            });
                        match init { Some(v) => (v, IvKind::Global(label)), None => continue }
                    }
                    Some(IvKind::Ssa) => unreachable!(),
                }
            };

        // --- Find the IV increment before the back edge ---
        // Case A (SSA IV): `Add/Sub { dst=iv_id, lhs=iv_id }` at back_edge_idx - 1.
        // Case B/C (memory IV): scan backward for the store back to the IV slot, trace
        //   it through the Add/Sub to find stride and increment range start.
        let (stride, inc_start_idx): (i64, usize) = match &iv_kind {
            IvKind::Ssa => {
                let inc_idx = lp.back_edge_idx - 1;
                let s = match &func.body[inc_idx].op {
                    IrOp::Add { dst, lhs, rhs, .. } if dst.id == iv_vreg_id && lhs.id == iv_vreg_id => {
                        if let Some(&c) = constants.get(&rhs.id) { c } else { continue }
                    }
                    IrOp::Sub { dst, lhs, rhs, .. } if dst.id == iv_vreg_id && lhs.id == iv_vreg_id => {
                        if let Some(&c) = constants.get(&rhs.id) { -c } else { continue }
                    }
                    _ => continue,
                };
                (s, inc_idx)
            }
            IvKind::Local(local_offset) => {
                let local_offset = *local_offset;
                let store_pos = func.body[exit_idx + 1..lp.back_edge_idx]
                    .iter()
                    .rposition(|instr| {
                        matches!(&instr.op, IrOp::StoreLocal { offset, .. } if *offset == local_offset)
                    });
                let store_idx = match store_pos {
                    Some(rel) => exit_idx + 1 + rel, None => continue,
                };
                let v_inc_id = match &func.body[store_idx].op {
                    IrOp::StoreLocal { src, .. } => src.id, _ => continue,
                };
                let (stride_val, lhs_id) = match find_add_stride(&func.body, exit_idx + 1, store_idx, v_inc_id, &constants) {
                    Some(v) => v, None => continue,
                };
                let load_pos = func.body[exit_idx + 1..store_idx + 1]
                    .iter()
                    .rposition(|instr| {
                        matches!(&instr.op, IrOp::LoadLocal { dst, offset }
                            if dst.id == lhs_id && *offset == local_offset)
                    });
                let inc_start = load_pos.map(|rel| exit_idx + 1 + rel).unwrap_or(store_idx);
                (stride_val, inc_start)
            }
            IvKind::Global(iv_label) => {
                let iv_label = iv_label.clone();
                let store_pos = func.body[exit_idx + 1..lp.back_edge_idx]
                    .iter()
                    .rposition(|instr| {
                        matches!(&instr.op, IrOp::StoreGlobal { addr_label, .. } if addr_label == &iv_label)
                    });
                let store_idx = match store_pos {
                    Some(rel) => exit_idx + 1 + rel, None => continue,
                };
                let v_inc_id = match &func.body[store_idx].op {
                    IrOp::StoreGlobal { src, .. } => src.id, _ => continue,
                };
                let (stride_val, lhs_id) = match find_add_stride(&func.body, exit_idx + 1, store_idx, v_inc_id, &constants) {
                    Some(v) => v, None => continue,
                };
                let load_pos = func.body[exit_idx + 1..store_idx + 1]
                    .iter()
                    .rposition(|instr| {
                        matches!(&instr.op, IrOp::LoadGlobal { dst, addr_label }
                            if dst.id == lhs_id && addr_label == &iv_label)
                    });
                let inc_start = load_pos.map(|rel| exit_idx + 1 + rel).unwrap_or(store_idx);
                (stride_val, inc_start)
            }
        };

        if stride == 0 {
            continue; // Infinite loop, don't touch.
        }

        // Compute trip count.
        let trip_count: u64 = if is_lt {
            // Counting up: trips = ceil((limit - init) / stride)
            if stride <= 0 { continue; }
            let diff = if is_signed {
                sign_extend(limit_val, Width::W16) - sign_extend(init_val, Width::W16)
            } else {
                (to_unsigned(limit_val, Width::W16) as i64)
                    - (to_unsigned(init_val, Width::W16) as i64)
            };
            if diff <= 0 { 0 } else { ((diff + stride - 1) / stride) as u64 }
        } else {
            // Counting down: trips = ceil((init - limit) / (-stride))
            if stride >= 0 { continue; }
            let neg_stride = -stride;
            let diff = if is_signed {
                sign_extend(init_val, Width::W16) - sign_extend(limit_val, Width::W16)
            } else {
                (to_unsigned(init_val, Width::W16) as i64)
                    - (to_unsigned(limit_val, Width::W16) as i64)
            };
            if diff <= 0 { 0 } else { ((diff + neg_stride - 1) / neg_stride) as u64 }
        };

        if trip_count == 0 || trip_count > UNROLL_MAX_TRIPS {
            continue;
        }

        // Body: instructions from after the exit JumpIfFalse to before the increment.
        let body_start = exit_idx + 1;
        let body_end = inc_start_idx; // exclusive
        if body_end <= body_start {
            continue;
        }
        let body_len = body_end - body_start;
        if body_len > UNROLL_MAX_BODY {
            continue;
        }

        // --- Perform full unrolling ---
        //
        // Replace the loop with `trip_count` copies of the body.  We
        // need to allocate fresh vregs/labels for each unrolled copy
        // to avoid definition conflicts.

        let mut max_vreg_id: u32 = 0;
        let mut max_label_id: u32 = 0;
        for instr in &func.body {
            if let Some(dst) = get_dst_vreg(&instr.op) {
                if dst.id >= max_vreg_id { max_vreg_id = dst.id + 1; }
            }
            for s in collect_src_vregs(&instr.op) {
                if s >= max_vreg_id { max_vreg_id = s + 1; }
            }
            if let IrOp::Label { label } = &instr.op {
                if label.0 >= max_label_id { max_label_id = label.0 + 1; }
            }
        }

        // Collect the body and increment instructions to replicate.
        let body_instrs: Vec<IrInstr> = func.body[body_start..body_end].to_vec();
        // Increment range: from inc_start_idx to back_edge (exclusive).
        let inc_instrs: Vec<IrInstr> = func.body[inc_start_idx..lp.back_edge_idx].to_vec();

        // Compute vregs that are defined WITHIN the loop region (body + inc).
        // Vregs defined outside (e.g. the stride constant from a pre-loop LoadImm,
        // or the limit value) are "external" and must keep their original IDs across
        // all iterations so constant folding can still resolve them.
        let internally_defined: HashSet<u32> = body_instrs.iter()
            .chain(inc_instrs.iter())
            .filter_map(|i| get_any_dst(&i.op).map(|v| v.id))
            .collect();

        let external_live_vregs: HashSet<u32> = body_instrs.iter()
            .chain(inc_instrs.iter())
            .flat_map(|i| collect_src_vregs(&i.op))
            .filter(|id| !internally_defined.contains(id))
            .collect();

        let mut new_body: Vec<IrInstr> = Vec::with_capacity(func.body.len());

        // Emit everything before the loop (excluding the header label).
        for idx in 0..lp.header_idx {
            new_body.push(func.body[idx].clone());
        }

        // Emit the unrolled copies.
        for iter_no in 0..trip_count {
            if iter_no == 0 {
                // First iteration uses the original instructions unchanged.
                for instr in body_instrs.iter().chain(inc_instrs.iter()) {
                    new_body.push(instr.clone());
                }
            } else {
                // Subsequent iterations get fresh vregs/labels for internally-defined
                // vregs to avoid SSA definition conflicts.  External vregs (stride,
                // limit, etc.) keep their original IDs so constant folding sees them.
                let mut vreg_map: HashMap<u32, u32> = HashMap::new();
                let mut label_map: HashMap<u32, u32> = HashMap::new();

                for &vid in &external_live_vregs {
                    vreg_map.insert(vid, vid);
                }

                // For SSA-style IV, the IV vreg itself threads values across iterations.
                if matches!(iv_kind, IvKind::Ssa) {
                    vreg_map.insert(iv_vreg_id, iv_vreg_id);
                }

                // Body and increment share the same vreg_map within one iteration
                // so any vreg defined in the body is available to the increment.
                for instr in body_instrs.iter().chain(inc_instrs.iter()) {
                    let new_op = remap_op(
                        &instr.op,
                        &mut vreg_map,
                        &mut label_map,
                        &mut max_vreg_id,
                        &mut max_label_id,
                    );
                    new_body.push(IrInstr { op: new_op, line: instr.line });
                }
            }
        }

        // Emit the exit label and everything after it.
        // Find the exit label position.
        let exit_label_idx = label_index.get(&exit_label).copied();
        if let Some(eidx) = exit_label_idx {
            for idx in eidx..func.body.len() {
                new_body.push(func.body[idx].clone());
            }
        } else {
            // No exit label found; emit everything after back edge.
            for idx in (lp.back_edge_idx + 1)..func.body.len() {
                new_body.push(func.body[idx].clone());
            }
        }

        func.body = new_body;
        changed = true;
        break; // Indices invalidated; re-enter via fixed-point loop.
    }

    changed
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
// Sink loads — move single-use pure instructions closer to their consumer
// ---------------------------------------------------------------------------
//
// After constant folding and DCE, instructions like `AddrOfGlobal`,
// `LoadImm`, `LoadPtr` (W8), and `LoadGlobal` (W8) may be defined far from
// their single use.  On the 8080, long live ranges cause unnecessary spills
// because there are very few registers (e.g. W8 values all share A).
//
// This pass sinks such instructions to just before their consumer within the
// same basic block.  It follows single-use dependency chains: if a LoadPtr's
// pointer operand is also single-use (e.g. an AddrOfGlobal), both are sunk
// together.
//
// This enables the codegen's deferred-M mechanism and avoids register
// conflicts between addresses that share HL.

fn sink_w8_loads(func: &mut IrFunction) -> bool {
    // Build def-site map and use-site lists.
    let mut def_site: HashMap<u32, usize> = HashMap::new();
    let mut use_sites: HashMap<u32, Vec<usize>> = HashMap::new();

    for (idx, instr) in func.body.iter().enumerate() {
        if let Some(dst) = get_dst_vreg(&instr.op) {
            def_site.insert(dst.id, idx);
        }
        for src_id in collect_src_vregs(&instr.op) {
            use_sites.entry(src_id).or_default().push(idx);
        }
    }

    /// Check if an instruction is "sinkable": pure, cheap, and produces a
    /// single result with no side effects.
    fn is_sinkable(op: &IrOp) -> bool {
        matches!(
            op,
            IrOp::LoadImm { .. }
                | IrOp::AddrOfGlobal { .. }
                | IrOp::LoadPtr { .. }
                | IrOp::LoadGlobal { .. }
        )
    }

    /// Check if no intervening instruction between `from`+1 and `to` is a
    /// BB boundary or memory-writing op that could alias loads.
    fn safe_to_sink(body: &[IrInstr], from: usize, to: usize) -> bool {
        body[from + 1..to].iter().all(|i| {
            !matches!(
                i.op,
                IrOp::StoreGlobal { .. }
                    | IrOp::StoreLocal { .. }
                    | IrOp::StorePtr { .. }
                    | IrOp::Call { .. }
                    | IrOp::Label { .. }
                    | IrOp::Jump { .. }
                    | IrOp::JumpIfTrue { .. }
                    | IrOp::JumpIfFalse { .. }
                    | IrOp::Return { .. }
            )
        })
    }

    struct SinkCandidate {
        /// Indices of instructions to remove (in original order).
        remove_indices: Vec<usize>,
        /// Instructions to insert (in order), just before `use_idx`.
        insert_instrs: Vec<IrInstr>,
        /// Where to insert them.
        use_idx: usize,
    }
    let mut candidates: Vec<SinkCandidate> = Vec::new();

    for (idx, instr) in func.body.iter().enumerate() {
        if !is_sinkable(&instr.op) {
            continue;
        }
        let dst = match get_dst_vreg(&instr.op) {
            Some(d) => d,
            None => continue,
        };

        // Must have exactly one use.
        let uses = match use_sites.get(&dst.id) {
            Some(u) if u.len() == 1 => u,
            _ => continue,
        };
        let use_idx = uses[0];

        // Must be more than 1 instruction away.
        if use_idx <= idx + 1 {
            continue;
        }

        // Safety: no memory-writing/control-flow barriers in between.
        if !safe_to_sink(&func.body, idx, use_idx) {
            continue;
        }

        // Collect the chain of single-use dependencies to also sink.
        // Walk backwards from instr: if each source operand is defined by
        // a sinkable, single-use instruction just before, include it.
        let mut chain: Vec<usize> = vec![idx];
        let mut cur_idx = idx;
        loop {
            let src_ids = collect_src_vregs(&func.body[cur_idx].op);
            if src_ids.len() != 1 {
                break;
            }
            let src_id = src_ids[0];
            let dep_idx = match def_site.get(&src_id) {
                Some(&d) if d + 1 == cur_idx => d,
                _ => break,
            };
            if !is_sinkable(&func.body[dep_idx].op) {
                break;
            }
            // Dep must also be single-use.
            match use_sites.get(&src_id) {
                Some(u) if u.len() == 1 => {}
                _ => break,
            }
            // Dep must also be safe to sink across the same range.
            if !safe_to_sink(&func.body, dep_idx, use_idx) {
                break;
            }
            chain.push(dep_idx);
            cur_idx = dep_idx;
        }

        // chain is [idx, dep1, dep2, ...] — reverse so deps come first.
        chain.reverse();

        candidates.push(SinkCandidate {
            remove_indices: chain.clone(),
            insert_instrs: chain.iter().map(|&i| func.body[i].clone()).collect(),
            use_idx,
        });
    }

    if candidates.is_empty() {
        return false;
    }

    // Deduplicate: if an instruction index appears in multiple candidates'
    // remove sets, skip the redundant candidate.
    let mut remove_set: HashSet<usize> = HashSet::new();
    let mut insert_before: HashMap<usize, Vec<IrInstr>> = HashMap::new();
    for cand in &candidates {
        // Check for conflicts: if any index is already claimed, skip.
        if cand.remove_indices.iter().any(|i| remove_set.contains(i)) {
            continue;
        }
        for &i in &cand.remove_indices {
            remove_set.insert(i);
        }
        insert_before
            .entry(cand.use_idx)
            .or_default()
            .extend(cand.insert_instrs.iter().cloned());
    }

    let mut new_body = Vec::with_capacity(func.body.len());
    for (idx, instr) in func.body.iter().enumerate() {
        if let Some(inserts) = insert_before.get(&idx) {
            new_body.extend(inserts.iter().cloned());
        }
        if !remove_set.contains(&idx) {
            new_body.push(instr.clone());
        }
    }

    func.body = new_body;
    true
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
        let mut program = IrProgram::new();
        program.functions.push(func);
        optimize(&mut program);
        program.functions.pop().unwrap().body
    }

    fn opt_body_with_unroll_headers(body: Vec<IrInstr>, headers: &[u32]) -> Vec<IrInstr> {
        let mut func = IrFunction::new("test", CType::Void);
        func.body = body;
        func.unroll_loop_headers = headers.iter().copied().map(Label::new).collect();
        let mut program = IrProgram::new();
        program.functions.push(func);
        optimize(&mut program);
        program.functions.pop().unwrap().body
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
        let result = opt_body_with_unroll_headers(body, &[0]);
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
        let result = opt_body_with_unroll_headers(body, &[0]);
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
        let result = opt_body_with_unroll_headers(body, &[0]);
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
        // CSE replaces the second Add with Copy { dst:3, src:2 }.  copy_propagate
        // then substitutes v3→v2 in StoreGlobal "_g_d" and DCE removes the Copy.
        // Accept either form: the Copy is still present, or _g_d already uses v2.
        assert!(result.iter().any(|i| {
            matches!(&i.op,
                IrOp::Copy { dst, src } if dst.id == 3 && src.id == 2)
            || matches!(&i.op,
                IrOp::StoreGlobal { addr_label, src } if addr_label == "_g_d" && src.id == 2)
        }));
    }

    #[test]
    fn cse_cleared_at_label() {
        // A label that IS reached from two different paths (a real merge point)
        // must clear the CSE table, because the label can be reached with
        // different values in flight.  We construct this with a conditional jump:
        // the label is the target of a JumpIfTrue, so it is NOT a trivial
        // fall-through label and is preserved by remove_dead_labels.
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
            // Conditional jump makes Label(0) a real merge point (two predecessors:
            // the JumpIfTrue path and the fall-through path).
            IrInstr::bare(IrOp::JumpIfTrue {
                cond: VReg::new(0, Width::W16),
                target: Label::new(0),
            }),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            // Same computation after a live label — should NOT be CSE'd because
            // the label is a real merge point (CSE table cleared at Label).
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
        // After a real convergence label, CSE is cleared, so the second Add should remain.
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

    // -- Loop-invariant code motion ---------------------------------------

    #[test]
    fn licm_hoists_invariant_computation() {
        // Simulate:
        //   a = load_global "_g_a"              // vreg 0
        //   b = load_global "_g_b"              // vreg 1
        // L0:                                    // loop header
        //   c = add a, b                         // invariant! (vreg 2)
        //   i = load_global "_g_i"               // vreg 3 (changes each iter)
        //   store_global "_g_x", c
        //   jump_if_true i L0                    // back edge
        //
        // After LICM, the Add should be before L0.
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_a".into(),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_b".into(),
            }),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(3, Width::W16),
                addr_label: "_g_i".into(),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::JumpIfTrue {
                cond: VReg::new(3, Width::W16),
                target: Label::new(0),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body(body);

        // The Add of vreg 0 + vreg 1 producing vreg 2 should appear
        // before the Label(0), i.e., it was hoisted out of the loop.
        let label_pos = result
            .iter()
            .position(|i| matches!(&i.op, IrOp::Label { label } if label.0 == 0))
            .expect("label L0 must exist");
        let add_pos = result
            .iter()
            .position(|i| matches!(&i.op, IrOp::Add { dst, .. } if dst.id == 2))
            .expect("add must exist");
        assert!(
            add_pos < label_pos,
            "invariant Add should be hoisted before the loop header (add at {}, label at {})",
            add_pos, label_pos,
        );
    }

    #[test]
    fn licm_does_not_hoist_loop_dependent() {
        // Instruction that depends on a loop-defined vreg should NOT be hoisted.
        //   a = load_global "_g_a"              // vreg 0
        // L0:
        //   i = load_global "_g_i"               // vreg 1 (loop-variant)
        //   c = add a, i                         // depends on i → NOT invariant
        //   store_global "_g_x", c
        //   jump_if_true i L0
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_a".into(),
            }),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_i".into(),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::JumpIfTrue {
                cond: VReg::new(1, Width::W16),
                target: Label::new(0),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body(body);

        // The Add should still be AFTER the label (not hoisted).
        let label_pos = result
            .iter()
            .position(|i| matches!(&i.op, IrOp::Label { label } if label.0 == 0))
            .expect("label L0 must exist");
        let add_pos = result
            .iter()
            .position(|i| matches!(&i.op, IrOp::Add { dst, .. } if dst.id == 2))
            .expect("add must exist");
        assert!(
            add_pos > label_pos,
            "loop-dependent Add should NOT be hoisted (add at {}, label at {})",
            add_pos, label_pos,
        );
    }

    // -- Induction variable optimization ----------------------------------

    #[test]
    fn iv_opt_replaces_mul_with_derived_iv() {
        // Simulate:
        //   iv (v0) = 0               // init
        //   stride (v1) = 1           // constant stride
        //   factor (v3) = 10          // constant factor
        // L0: (loop header)
        //   t (v2) = iv * factor      // should be replaced
        //   store_global "_g_x", t
        //   iv = iv + stride          // IV increment
        //   jump_if_true iv, L0       // back edge
        //
        // After IV opt: the Mul should be gone, replaced by a Copy from
        // a derived IV that is incremented by stride * factor = 10.
        let body = vec![
            load_imm(0, Width::W16, 0),           // iv = 0
            load_imm(1, Width::W16, 1),           // stride = 1
            load_imm(3, Width::W16, 10),          // factor = 10
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::Mul {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(3, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(2, Width::W16),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(0, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::JumpIfTrue {
                cond: VReg::new(0, Width::W16),
                target: Label::new(0),
            }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body(body);

        // The loop body should no longer contain a Mul instruction.
        let label_pos = result
            .iter()
            .position(|i| matches!(&i.op, IrOp::Label { label } if label.0 == 0))
            .expect("label L0 must exist");
        let has_mul_in_loop = result[label_pos..]
            .iter()
            .any(|i| matches!(&i.op, IrOp::Mul { .. }));
        assert!(
            !has_mul_in_loop,
            "Mul inside the loop should be replaced by IV opt"
        );

        // Should have a Copy for the replaced Mul destination, OR copy_propagate
        // already propagated the derived-IV value into the StoreGlobal so the
        // Copy was eliminated.  Either way, the _g_x store must not use the
        // original Mul destination vreg (v2) directly.
        let has_copy = result[label_pos..]
            .iter()
            .any(|i| matches!(&i.op, IrOp::Copy { dst, .. } if dst.id == 2));
        let store_avoids_mul_dst = !result[label_pos..]
            .iter()
            .any(|i| matches!(&i.op, IrOp::StoreGlobal { addr_label, src } if addr_label == "_g_x" && src.id == 2));
        assert!(
            has_copy || store_avoids_mul_dst,
            "Mul should be replaced with a Copy from the derived IV (or the derived IV used directly)"
        );
    }

    // -- Loop unrolling ---------------------------------------------------

    #[test]
    fn loop_unrolling_fully_unrolls_small_loop() {
        // Simulate:
        //   iv (v0) = 0
        //   limit (v1) = 4
        //   stride (v2) = 1
        // L0: (header)
        //   cond (v3) = Lt iv, limit
        //   JumpIfFalse cond, L1
        //   store_global "_g_x", iv   // body
        //   iv = Add iv, stride       // increment
        //   Jump L0                   // back edge
        // L1: (exit)
        //   Return
        //
        // Trip count = 4, should fully unroll.
        let body = vec![
            load_imm(0, Width::W16, 0),
            load_imm(1, Width::W16, 4),
            load_imm(2, Width::W16, 1),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::Lt {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::JumpIfFalse {
                cond: VReg::new(3, Width::W16),
                target: Label::new(1),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(0, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(2, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::Jump { target: Label::new(0) }),
            IrInstr::bare(IrOp::Label { label: Label::new(1) }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body_with_unroll_headers(body, &[0]);

        // After full unrolling, there should be no Jump back to L0
        // (the loop is eliminated).
        let has_jump_to_l0 = result
            .iter()
            .any(|i| matches!(&i.op, IrOp::Jump { target } if target.0 == 0));
        assert!(
            !has_jump_to_l0,
            "Fully unrolled loop should not have a back-edge jump"
        );

        // After unrolling + redundant-store elimination only the last store to
        // _g_x remains (the first 3 are overwritten before being read).
        let store_count = result
            .iter()
            .filter(|i| matches!(&i.op, IrOp::StoreGlobal { addr_label, .. } if addr_label == "_g_x"))
            .count();
        assert_eq!(
            store_count, 1,
            "Expected 1 remaining store (last value), got {}", store_count
        );
    }

    #[test]
    fn loop_unrolling_skips_large_trip_count() {
        // Trip count = 100, exceeds UNROLL_MAX_TRIPS → should NOT unroll.
        let body = vec![
            load_imm(0, Width::W16, 0),
            load_imm(1, Width::W16, 100),
            load_imm(2, Width::W16, 1),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::Lt {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::JumpIfFalse {
                cond: VReg::new(3, Width::W16),
                target: Label::new(1),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(0, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(2, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::Jump { target: Label::new(0) }),
            IrInstr::bare(IrOp::Label { label: Label::new(1) }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body(body);

        // Loop should still exist (not unrolled).
        let has_jump_to_l0 = result
            .iter()
            .any(|i| matches!(&i.op, IrOp::Jump { target } if target.0 == 0));
        assert!(
            has_jump_to_l0,
            "Loop with trip count > UNROLL_MAX_TRIPS should NOT be unrolled"
        );
    }

    #[test]
    fn loop_unrolling_requires_hint() {
        // Canonical 4-iteration loop, but without unroll hint metadata.
        let body = vec![
            load_imm(0, Width::W16, 0),
            load_imm(1, Width::W16, 4),
            load_imm(2, Width::W16, 1),
            IrInstr::bare(IrOp::Label { label: Label::new(0) }),
            IrInstr::bare(IrOp::Lt {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
                signed: true,
            }),
            IrInstr::bare(IrOp::JumpIfFalse {
                cond: VReg::new(3, Width::W16),
                target: Label::new(1),
            }),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::Add {
                dst: VReg::new(0, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(2, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::Jump { target: Label::new(0) }),
            IrInstr::bare(IrOp::Label { label: Label::new(1) }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let result = opt_body(body);
        let has_jump_to_l0 = result
            .iter()
            .any(|i| matches!(&i.op, IrOp::Jump { target } if target.0 == 0));
        assert!(has_jump_to_l0, "Loop should not be unrolled without hint");
    }

    // -- Phase 2: forwarding and narrowing -------------------------------

    #[test]
    fn forwarding_replaces_load_after_store_global() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_x".into(),
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];

        let result = opt_body(body);
        // load_store_forwarding replaces the LoadGlobal with Copy { dst:1, src:0 }.
        // copy_propagate then substitutes v1→v0 in all uses and DCE removes the
        // Copy.  Either form is correct; accept both.
        assert!(result.iter().any(|i| {
            matches!(
                &i.op,
                IrOp::Copy { dst, src } if dst.id == 1 && src.id == 0
            ) || matches!(
                &i.op,
                IrOp::LoadImm { dst, value: 42 } if dst.id == 1
            ) || matches!(
                // copy_propagate propagated v1→v0; Return now references v0 directly.
                &i.op,
                IrOp::Return { value: Some(v) } if v.id == 0
            )
        }));
    }

    #[test]
    fn forwarding_invalidated_by_call() {
        let body = vec![
            load_imm(0, Width::W16, 42),
            IrInstr::bare(IrOp::StoreGlobal {
                addr_label: "_g_x".into(),
                src: VReg::new(0, Width::W16),
            }),
            IrInstr::bare(IrOp::call("ext", vec![], None)),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_x".into(),
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(1, Width::W16)))),
        ];

        let result = opt_body(body);
        assert!(result.iter().any(|i| matches!(&i.op, IrOp::LoadGlobal { dst, .. } if dst.id == 1)));
    }

    #[test]
    fn narrow_compare_to_w8_when_operands_are_byte_range() {
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(0, Width::W16),
                addr_label: "_g_a".into(),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: VReg::new(1, Width::W16),
                addr_label: "_g_b".into(),
            }),
            IrInstr::bare(IrOp::And {
                dst: VReg::new(3, Width::W16),
                lhs: VReg::new(0, Width::W16),
                rhs: VReg::new(1, Width::W16),
                width: Width::W16,
            }),
            load_imm(4, Width::W16, 255),
            IrInstr::bare(IrOp::And {
                dst: VReg::new(5, Width::W16),
                lhs: VReg::new(3, Width::W16),
                rhs: VReg::new(4, Width::W16),
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::Lt {
                dst: VReg::new(2, Width::W16),
                lhs: VReg::new(5, Width::W16),
                rhs: VReg::new(4, Width::W16),
                width: Width::W16,
                signed: false,
            }),
            IrInstr::bare(IrOp::ret(Some(VReg::new(2, Width::W16)))),
        ];

        let result = opt_body(body);
        assert!(result.iter().any(|i| {
            matches!(
                &i.op,
                IrOp::Lt { width: Width::W8, .. }
            )
        }));
    }

    #[test]
    fn narrow_promoted_add_consumed_by_w8_cast() {
        // Model: data3 += data4 + 1 where chars are promoted to int for add,
        // then truncated back to char on assignment.
        let s1 = VReg::new(0, Width::W8);
        let s2 = VReg::new(1, Width::W8);
        let x1 = VReg::new(2, Width::W16);
        let k1 = VReg::new(3, Width::W16);
        let x2 = VReg::new(4, Width::W16);
        let n1 = VReg::new(5, Width::W8);
        let x3 = VReg::new(6, Width::W16);
        let x4 = VReg::new(7, Width::W16);
        let x5 = VReg::new(8, Width::W16);

        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal {
                dst: s1,
                addr_label: "_g_data3".into(),
            }),
            IrInstr::bare(IrOp::LoadGlobal {
                dst: s2,
                addr_label: "_g_data4".into(),
            }),
            IrInstr::bare(IrOp::cast(x1, s2, CType::int_signed())),
            load_imm(k1.id, Width::W16, 1),
            IrInstr::bare(IrOp::Add {
                dst: x2,
                lhs: x1,
                rhs: k1,
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::cast(n1, x2, CType::char_signed())),
            IrInstr::bare(IrOp::cast(x3, s1, CType::int_signed())),
            IrInstr::bare(IrOp::cast(x4, n1, CType::int_signed())),
            IrInstr::bare(IrOp::Add {
                dst: x5,
                lhs: x3,
                rhs: x4,
                width: Width::W16,
            }),
            IrInstr::bare(IrOp::ret(Some(x5))),
        ];

        let result = opt_body(body);

        let narrowed_first_add = result.iter().any(|i| {
            matches!(
                &i.op,
                IrOp::Add {
                    dst,
                    lhs,
                    rhs,
                    width: Width::W8,
                } if dst.id == x2.id && (lhs.id == s2.id || rhs.id == s2.id)
            )
        });
        assert!(
            narrowed_first_add,
            "expected first promoted add to narrow to W8; got:\n{:#?}",
            result
        );

        let has_old_widen = result.iter().any(|i| {
            matches!(
                &i.op,
                IrOp::Cast { dst, src, .. } if dst.id == x1.id && src.id == s2.id
            )
        });
        assert!(
            !has_old_widen,
            "expected dead widen cast to be removed; got:\n{:#?}",
            result
        );
    }

    // -- Dedup specializations (Step 3) -----------------------------------

    #[test]
    fn dedup_identical_specializations() {
        // Two call sites calling `add_one(x, 1)` with the same constant.
        // function_specialization creates two __spec_ variants, but they are
        // identical after optimization.  dedup_specializations should redirect
        // both call sites to a single copy.
        let mut callee = IrFunction::new("add_one", CType::int_signed());
        let p0 = VReg::new(100, Width::W16);
        let p1 = VReg::new(101, Width::W16);
        let r  = VReg::new(102, Width::W16);
        callee.params = vec![
            crate::ir::IrParam { name: "a".into(), ty: CType::int_signed(), vreg: VReg::new(100, Width::W16) },
            crate::ir::IrParam { name: "b".into(), ty: CType::int_signed(), vreg: VReg::new(101, Width::W16) },
        ];
        callee.body = vec![
            IrInstr::bare(IrOp::LoadGlobal { dst: p0, addr_label: "_l_add_one_a".into() }),
            IrInstr::bare(IrOp::LoadGlobal { dst: p1, addr_label: "_l_add_one_b".into() }),
            IrInstr::bare(IrOp::Add { dst: r, lhs: p0, rhs: p1, width: Width::W16 }),
            IrInstr::bare(IrOp::ret(Some(r))),
        ];

        let mut main_fn = IrFunction::new("main", CType::Void);
        let c1  = VReg::new(0, Width::W16);
        let a1  = VReg::new(1, Width::W16);
        let r1  = VReg::new(2, Width::W16);
        let c2  = VReg::new(3, Width::W16);
        let a2  = VReg::new(4, Width::W16);
        let r2  = VReg::new(5, Width::W16);
        main_fn.body = vec![
            // First call: add_one(10, 1)
            IrInstr::bare(IrOp::LoadImm { dst: a1, value: 10 }),
            IrInstr::bare(IrOp::LoadImm { dst: c1, value: 1 }),
            IrInstr::bare(IrOp::Call { func_name: "add_one".into(), args: vec![a1, c1], dst: Some(r1) }),
            // Second call: add_one(20, 1) — same constant arg at position 1
            IrInstr::bare(IrOp::LoadImm { dst: a2, value: 20 }),
            IrInstr::bare(IrOp::LoadImm { dst: c2, value: 1 }),
            IrInstr::bare(IrOp::Call { func_name: "add_one".into(), args: vec![a2, c2], dst: Some(r2) }),
            IrInstr::bare(IrOp::ret(None)),
        ];

        let mut program = IrProgram::new();
        program.functions.push(main_fn);
        program.functions.push(callee);
        optimize(&mut program);

        // After optimization: at most one __spec_ function should survive.
        let spec_count = program.functions.iter()
            .filter(|f| f.name.contains("__spec_"))
            .count();
        assert!(
            spec_count <= 1,
            "expected at most 1 __spec_ function after dedup, got {}: {:?}",
            spec_count,
            program.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    // -- Compare-with-zero simplification (Step 4) ------------------------

    #[test]
    fn compare_zero_eq_jump_if_true_simplified() {
        // Eq(x, 0) + JumpIfTrue → JumpIfFalse(x)
        let x = VReg::new(0, Width::W16);
        let zero = VReg::new(1, Width::W16);
        let cmp = VReg::new(2, Width::W16);
        let lbl = Label::new(10);
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal { dst: x, addr_label: "_g_x".into() }),
            load_imm(1, Width::W16, 0),
            IrInstr::bare(IrOp::Eq { dst: cmp, lhs: x, rhs: zero, width: Width::W16 }),
            IrInstr::bare(IrOp::JumpIfTrue { cond: cmp, target: lbl }),
            IrInstr::bare(IrOp::Label { label: lbl }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        // Should have JumpIfFalse(x) instead of Eq+JumpIfTrue
        let has_jump_if_false = result.iter().any(|i| {
            matches!(&i.op, IrOp::JumpIfFalse { cond, .. } if cond.id == x.id)
        });
        assert!(has_jump_if_false, "expected JumpIfFalse(x) after simplification; got:\n{:#?}", result);
    }

    #[test]
    fn compare_zero_ne_jump_if_true_simplified() {
        // Ne(x, 0) + JumpIfTrue → JumpIfTrue(x)
        let x = VReg::new(0, Width::W16);
        let zero = VReg::new(1, Width::W16);
        let cmp = VReg::new(2, Width::W16);
        let lbl = Label::new(10);
        let body = vec![
            IrInstr::bare(IrOp::LoadGlobal { dst: x, addr_label: "_g_x".into() }),
            load_imm(1, Width::W16, 0),
            IrInstr::bare(IrOp::Ne { dst: cmp, lhs: x, rhs: zero, width: Width::W16 }),
            IrInstr::bare(IrOp::JumpIfTrue { cond: cmp, target: lbl }),
            IrInstr::bare(IrOp::Label { label: lbl }),
            IrInstr::bare(IrOp::ret(None)),
        ];
        let result = opt_body(body);
        let has_jump_if_true = result.iter().any(|i| {
            matches!(&i.op, IrOp::JumpIfTrue { cond, .. } if cond.id == x.id)
        });
        assert!(has_jump_if_true, "expected JumpIfTrue(x) after simplification; got:\n{:#?}", result);
    }
}