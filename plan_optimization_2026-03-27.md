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
- No profile-guided or benchmark-driven optimization budget (passes are static and generic).

## Prioritized Roadmap

### Phase 1 (High impact, low-to-medium risk)

1. Implement call-tree static-slot reuse for non-recursive functions.
   - Replace flat per-function local allocation with compatibility-by-lifetime allocation.
   - Keep current fallback for recursive/unknown call edges.
   - Expected: lower RAM usage, better locality, fewer address labels.

2. Add call-site selective save/restore instead of save-all.
   - Use live-vreg-at-call analysis to spill only values live after call.
   - Mark clobber sets for runtime helpers and normal calls.
   - Expected: fewer memory ops around every call.

3. Expand peephole with safe branch/call and compare cleanups.
   - Add c8080-style rules: conditional inversion over jump, jump-to-ret folding, call+ret to jump in more forms.
   - Add strict label/reference bookkeeping tests.
   - Expected: immediate size and cycle wins in control-heavy code.

4. Add benchmark gate for Sieve, Dhrystone, Fannkuch.
   - Track generated asm size and estimated cycles per benchmark.
   - Treat regressions as CI failures after baseline capture.

### Phase 2 (High impact, medium risk)

1. Introduce demand-driven 8-bit value narrowing in IR and codegen.
   - Propagate known 8-bit ranges into compare/bitwise/shift/div/mod cases where semantics are preserved.
   - Generate 8-bit instruction sequences directly when legal.

2. Improve call argument lowering strategy.
   - Reorder evaluation when safe to reduce spills.
   - Prefer already-resident registers for arg0/arg1 assignment.
   - Add a simple cost model similar to c8080 argument case selection.

3. Add a tiny memory-state tracker in late codegen.
   - Track "reg already has var X" and "var X already saved" across straight-line blocks.
   - Remove redundant LHLD/LDA/SHLD/STA before peephole.

### Phase 3 (Medium impact, medium-to-high risk)

1. Inter-block register allocation improvement.
   - Keep selected values in registers across block boundaries using predecessor merge heuristics.
   - Avoid immediate spill/reload churn around labels.

2. Runtime helper specialization.
   - Add fast paths for constant divisors/shifts and common multiply cases.
   - Avoid generic helper calls when cheaper inline sequences exist.

3. Profile-guided pass ordering.
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

1. Live-at-call analysis + selective spills in codegen.
2. 4-6 new control-flow peephole rules with tests.
3. Baseline harness for size/cycle reports on tests/sieve.c, tests/dhrystone.c, tests/fannkuch.c.

This slice is intentionally scoped to deliver measurable speedups without large IR redesign.
