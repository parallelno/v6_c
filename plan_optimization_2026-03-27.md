# v6c Optimization Improvement Plan (from c8080 analysis)

Date: 2026-03-27

## Scope

This plan is based on direct inspection of:
- c8080 reference compiler backend and optimizer pipeline
- current v6c IR/codegen/register allocation/peephole implementation

Goal: improve generated i8080 performance first, then code size, while preserving correctness.

## Key Findings from c8080

1. Multi-stage optimization pipeline:
   - AST/tree prepare passes before codegen (const folding, dead branch removal, 8-bit narrowing, inc/dec and operator canonicalization).
   - Assembly-level fixed-point optimizer after codegen.

2. Static stack model is a major 8080-specific win:
   - Default global/static-frame mode for non-recursive functions.
   - Recursion falls back to stack mode.
   - Call-tree-based static frame offset reuse and recursion checks.

3. Register-aware call and expression shaping:
   - Last argument in registers (A/HL/DE:HL) for eligible signatures.
   - Cost-measured argument ordering/placement around constrained registers.

4. Dataflow-driven asm peephole:
   - Tracks known register/value and saved-variable state across basic blocks.
   - Removes redundant loads/stores and rewrites to shorter equivalents.

## Current v6c State (relevant)

Strengths already present:
- IR optimizer includes constant folding/propagation, DCE, CSE, jump threading, LICM, induction variable optimization, loop unrolling, and inlining.
- Call graph + static local/global allocation exists for non-recursive functions.
- Register allocator and peephole pass are in place.

Main gaps vs c8080 techniques:
- Static local allocation is non-overlapping per function (no call-tree lifetime reuse).
- Call lowering always spills all live regs before calls, then loads args from memory; no measured argument-order strategy.
- Peephole is mostly local pattern matching, not register/value dataflow-based.
- Limited target-aware 8-bit narrowing from high-level IR patterns into specialized codegen paths.
- No interprocedural function-effect summaries (pure/readonly/leaf/noreturn/mod-ref/clobber sets) to drive call optimization.
- No sparse value-range or demanded-bits analysis to prove byte-sized values and eliminate unnecessary high-byte work.
- Register allocation is spill-oriented; it does not rematerialize cheap values such as constants or static addresses.
- No explicit CFG block-layout or jump-chain compaction pass before asm peephole.
- No profile-guided or benchmark-driven optimization budget (passes are static and generic).

## Prioritized Roadmap

### Phase 1 (High impact, low-to-medium risk)

1. Implement call-tree static-slot reuse for non-recursive functions.
   - Replace flat per-function local allocation with compatibility-by-lifetime allocation.
   - Keep current fallback for recursive/unknown call edges.
   - Expected: lower RAM usage, better locality, fewer address labels.

2. Add interprocedural function-effect summaries.
   - Infer or annotate `leaf`, `pure`, `readonly`, `noreturn`, mod/ref scope, and target-specific clobber sets.
   - Use summaries for runtime helpers first, then extend to normal calls.
   - Expected: enables selective save/restore, safer call motion, and dead-call cleanup.

3. Add call-site selective save/restore instead of save-all.
   - Use live-vreg-at-call analysis to spill only values live after call.
   - Consume function-effect/clobber summaries instead of assuming broad call damage.
   - Expected: fewer memory ops around every call.

4. Expand peephole with safe branch/call and compare cleanups.
   - Add c8080-style rules: conditional inversion over jump, jump-to-ret folding, call+ret to jump in more forms.
   - Add strict label/reference bookkeeping tests.
   - Expected: immediate size and cycle wins in control-heavy code.

5. Formalize benchmark gate for Sieve, Dhrystone, Fannkuch.
   - Track generated asm size and estimated cycles per benchmark.
   - Treat regressions as CI failures after baseline capture, rather than relying only on compile-success tests.

### Phase 2 (High impact, medium risk)

1. Introduce sparse value-range and demanded-bits analysis.
   - Track facts such as known-zero high byte, non-negative ranges, and small masks through SSA-like value flow.
   - Feed narrowing, compare simplification, helper-call avoidance, and dead high-byte elimination from one analysis.

2. Introduce demand-driven 8-bit value narrowing in IR and codegen.
   - Propagate known 8-bit ranges into compare/bitwise/shift/div/mod cases where semantics are preserved.
   - Generate 8-bit instruction sequences directly when legal.

3. Improve call argument lowering strategy.
   - Reorder evaluation when safe to reduce spills.
   - Prefer already-resident registers for arg0/arg1 assignment.
   - Add a simple cost model similar to c8080 argument case selection.

4. Add load/store forwarding with memory versioning.
   - Track versioned state for static locals and globals within a block or region, invalidating on unknown stores and impure calls.
   - Replace repeated loads from unchanged slots with register reuse and turn store-followed-by-load into a no-op.
   - Expected: major reduction in redundant LHLD/LDA/SHLD/STA traffic in the current static-slot model.

5. Add rematerialization to register allocation.
   - Recreate cheap values such as constants, addresses of static slots, and simple boolean materializations instead of reloading spill slots.
   - Prefer rematerialization when it is shorter or cheaper than memory traffic on 8080.

6. Add CFG block layout and jump-chain compaction before peephole.
   - Place hot successors in fallthrough position, merge trivial blocks, and collapse unconditional jump chains earlier.
   - Expected: better branch locality and better exposure for assembly-level cleanup rules.

### Phase 3 (Medium impact, medium-to-high risk)

1. Inter-block register allocation improvement.
   - Keep selected values in registers across block boundaries using predecessor merge heuristics.
   - Avoid immediate spill/reload churn around labels.

2. Function specialization for constant arguments.
   - Clone small functions when call sites provide constant flags, masks, pointer bases, or fixed small trip counts.
   - Let existing constant folding and DCE aggressively simplify specialized bodies.

3. Runtime helper specialization.
   - Add fast paths for constant divisors/shifts and common multiply cases.
   - Avoid generic helper calls when cheaper inline sequences exist.

4. Profile-guided pass ordering.
   - Tune pass order and thresholds per benchmark family.
   - Keep generic default for non-benchmark builds.

## Validation Plan

For each completed item:
1. Run full test suite (correctness).
2. Compile benchmark set and record:
   - text size
   - helper call counts
   - estimated cycles in hot loops
3. Compare against previous baseline and reject regressions unless justified.

## Suggested First Implementation Slice (1-2 weeks)

1. Function clobber/effect summaries for runtime helpers and normal calls.
2. Live-at-call analysis + selective spills in codegen.
3. 4-6 new control-flow peephole rules with tests.
4. Baseline harness for size/cycle reports on tests/sieve.c, tests/dhrystone.c, tests/fannkuch.c.

This slice is intentionally scoped to deliver measurable speedups without large IR redesign, while also laying groundwork for range analysis and memory-versioned load/store forwarding.
