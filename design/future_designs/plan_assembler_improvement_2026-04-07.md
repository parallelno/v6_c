# Comprehensive Assembler Improvement Implementation Plan

Date: 2026-04-07

## Overview

Based on analysis of `plan_optimization_2026-03-27.md` (architectural roadmap from
c8080 comparison) and `report_optimization_2026-03-30.md` (concrete analysis of
generated code for sieve.c, dhrystone.c, fannkuch.c, loop.c).

All cycle counts use **Vector 06c (КР580ВМ80)** timings.

**Ordering:** simplest to most complex. Each step must include:
1. Implementation
2. Test update and run
3. Documentation update

---

## Step 1: PtrAdd Fast-Path for element_size 2/4/8 ✅ COMPLETE

**Difficulty:** Easy  
**Est. savings:** ~200–1800 cycles per array access  
**Source:** Report #2

**Current state:** In `codegen.rs::gen_ptr_add()`, all element_size > 1 calls
`CALL __mul16` (~350+ cycles even for multiply-by-2). The existing `gen_mul()`
already has fast-paths for constant `*2`, `*4`, `*8` using `DAD H`, but
`gen_ptr_add` does not use them.

**Changes:**
- `codegen.rs`: In `gen_ptr_add`, before the `__mul16` fallback, add match arms:
  - element_size 2: one `DAD H` (12 cycles)
  - element_size 4: two `DAD H`
  - element_size 8: three `DAD H`
  - Then `XCHG` + `ensure_hl(ptr)` + `DAD D`
- **Test:** Add test case: `int arr[10]; arr[i] = 5;` — verify no
  `CALL __mul16` in output. Also test `long arr[4]; arr[i] = 1;` for
  element_size=4.
- **Doc:** Update `docs/ir_optimization.md` with PtrAdd fast-path description.

---

## Step 2: MVI M,n for Constant Stores ✅ COMPLETE

**Difficulty:** Easy  
**Est. savings:** ~4–8 cycles per store  
**Source:** Report #10, #15

**Current state:** In `codegen.rs::gen_store_ptr()`, storing constants to
memory through a pointer loads the value into DE first (`LXI D,val` + two
`MOV M,R` + `INX H`). For known small immediates, `MVI M,n` is cheaper and
frees the DE register pair.

**Changes:**
- `codegen.rs`: In `gen_store_ptr` for W16 general-case path, check
  `known_imm(src)`. If it's a compile-time constant, emit `MVI M,low` /
  `INX H` / `MVI M,high` instead of going through DE.
- For zero specifically: prefer `XRA A` / `MOV M,A` / `INX H` / `MOV M,A`
  (28 cycles vs 36).
- **Test:** Add test storing 0 and 1 to array elements; verify no `LXI D,0`
  or `LXI D,1` appears in output.
- **Doc:** Add to codegen optimization notes.

---

## Step 3: Dedup Identical Function Specializations ✅ COMPLETE

**Difficulty:** Easy  
**Est. savings:** Code size only  
**Source:** Report #6

**Current state:** After `function_specialization()` in `ir_opt.rs`, identical
specializations like `__spec_add_asm_0` and `__spec_add_asm_1` both survive.
Dead originals also survive when they shouldn't.

**Changes:**
- `ir_opt.rs` or new helper: After specialization, compare function IR bodies.
  If two `__spec_*` bodies are identical, redirect all call references to one
  and remove the other. Also verify `remove_dead_functions()` eliminates
  unreferenced originals.
- **Test:** Add test with a function called at two call sites with the same
  constant argument. Verify only one `__spec_*` version appears in output.
- **Doc:** Update specialization section in docs.

---

## Step 4: Compare-with-Zero Fast Path ✅ COMPLETE

**Difficulty:** Easy  
**Est. savings:** ~12 cycles per test  
**Source:** Report #5

**Current state:** `if (x)` where x is not already a comparison result still
goes through full W16 comparison machinery before testing. The test itself
(`MOV A,H` / `ORA L` / `JZ`) is fine, but an unnecessary comparison vreg is
produced first.

**Changes:**
- `ir_opt.rs`: In constant folding, recognize `Eq { lhs: x, rhs: 0 }` or
  `Ne { lhs: x, rhs: 0 }` followed by `JumpIfTrue`/`JumpIfFalse` and simplify
  to direct `JumpIfTrue(x)` / `JumpIfFalse(x)` (or inverted).
- `codegen.rs`: In `gen_jump_if_true` and `gen_jump_if_false`, ensure the
  non-comparison case emits minimal `ORA L` test.
- **Test:** Add tests for `if (x) { ... }` and `if (x == 0) { ... }` patterns.
  Verify no full comparison sequence for zero-checks.
- **Doc:** Document the zero-comparison optimization.

---

## Step 5: Peephole — W16 Compare-Branch Collapse (Safety Net) ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** Safety net for Step 7 (covers cases fusion misses)  
**Source:** Report #12

**Current state:** No peephole rule recognizes the materialized boolean pattern
(`JM/JZ __cg_0` → `LXI H,0` → `JMP __cg_1` → `__cg_0:` → `LXI H,1` →
`__cg_1:` → `MOV B,H` → `MOV C,L` → `MOV A,H` → `ORA L` → `JZ target`)
and collapses it.

**Changes:**
- `peephole.rs`: Add a new multi-line pattern rule that detects the boolean
  materialisation + re-test pattern. Transform into direct conditional jumps:
  ```asm
  JM  skip
  JZ  skip
  JMP target
  skip:
  ```
- Must handle all comparison operators (each has a different conditional jump
  prefix: JM/JZ for LE, JC for LT unsigned, etc.).
- **Test:** Add test with W16 comparison `if (a > b)` and verify the
  materialisation sequence is collapsed.
- **Doc:** Add rule to peephole documentation.

---

## Step 6: Eliminate Register Shuffles Around Compares ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~28 cycles per comparison (32 cycles for 4-MOV → 4 cycles for XCHG)  
**Source:** Report #3

**Current state:** The shuffle `MOV B,D; MOV C,E; XCHG; MOV H,B; MOV L,C`
happens because `ensure_de(rhs)` then `ensure_hl(lhs)` conflict: placing rhs
in DE, then requesting lhs in HL evicts DE's value to BC.

**Changes:**
- `codegen.rs`: In `gen_compare()` W16 path, change load order:
  1. Load RHS into HL first, then `XCHG` to DE
  2. Load LHS into HL directly
  3. No conflict — both are where they need to be
- Fallback: When LHS is in DE and RHS is in HL, emit single `XCHG` (4 cycles)
  instead of 4-MOV shuffle through BC (32 cycles).
- **Test:** Add test comparing two variables `if (a <= b)` and verify no
  `MOV B,D; MOV C,E` shuffle sequence in output.
- **Doc:** Update codegen comparison documentation.

---

## Step 7: W16 Compare-Branch Fusion (CRITICAL) ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~70–100 cycles per comparison (~50% cycle reduction)  
**Source:** Report #1

**Current state:** W8 CPI-branch fusion exists (codegen peeks at next IR op to
fuse compare + conditional jump). W16 comparisons always materialise a boolean
(0 or 1) into HL, then re-test with `ORA L` + conditional jump. This adds
~12 extra instructions and ~100 cycles per W16 comparison.

**Changes:**
- `codegen.rs`: In `gen_compare()` W16 path, peek at next IR instruction.
  If it's `JumpIfTrue(cond, target)` or `JumpIfFalse(cond, target)` consuming
  this comparison's destination vreg:
  1. Emit the subtraction sequence (high-byte SUB + optional low-byte SUB)
  2. Emit direct conditional jumps to target or fall-through
  3. Add cond_vreg to `consumed_cmp` set (same mechanism as W8 fusion)
  4. Skip boolean materialisation entirely
- Handle all 6 comparison operators (Eq, Ne, Lt, Le, Gt, Ge) for both signed
  and unsigned variants.
- Also resolves Report #9 (dead BC copy) automatically.
- **Test:** Comprehensive tests for all W16 comparison operators with branches.
  Verify no `LXI H,0` / `LXI H,1` materialisation. Verify correct behavior
  for boundary values (0, -1, 32767, -32768).
- **Doc:** Document W16 fusion in codegen and optimization docs.

---

## Step 8: Strength Reduction for `2*i + k` Patterns ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~40 cycles per occurrence  
**Source:** Report #11

**Current state:** `prime = i + i + 3` generates two separate `Add` IR ops,
loading `i` twice. This should become `DAD H` + `LXI D,3` + `DAD D`.

**Changes:**
- `ir_opt.rs`: In `strength_reduce()` or constant folding, recognize
  `Add(x, x)` and convert to `Mul(x, 2)` or `Shl(x, 1)`. The existing
  `gen_mul` for constant 2 already emits `DAD H`.
- Also ensure `Add(Shl(x, 1), const)` chains remain fused.
- **Test:** Add test with `prime = i + i + 3` pattern. Verify `DAD H` appears
  and no `CALL __mul16` or double-load of `i`.
- **Doc:** Update strength reduction docs.

---

## Step 9: LICM for `_l_` Variables in Loops ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~20 cycles per loop iteration  
**Source:** Report #13

**Current state:** `loop_invariant_code_motion()` conservatively treats
`LoadGlobal` for `_l_*` labels as side-effectful and refuses to hoist. But
compiler-local variables (`_l_*`) are not aliased by external code.

**Changes:**
- `ir_opt.rs`: In `loop_invariant_code_motion()`, modify the
  invariant-candidate filter to include `LoadGlobal` operations when:
  1. The label starts with `_l_`
  2. No `StoreGlobal` to the same label exists inside the loop body
  3. No calls inside the loop might alias the variable (safe for `_l_*` since
     they are known not address-taken)
- **Test:** Add test with a loop that reads a loop-invariant local every
  iteration. Verify `LHLD _l_*` appears only once, before the loop.
- **Doc:** Update LICM documentation.

---

## Step 10: CSE for Repeated PtrAdd ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~200–1800 cycles per repeated occurrence  
**Source:** Report #14

**Current state:** `a[i] = a[i-1] + a[i]` generates three separate
`CALL __mul16` for `&a[i]`, `&a[i-1]`, and `&a[i]` again. CSE does not
recognize `PtrAdd` as a common sub-expression.

**Changes:**
- `ir_opt.rs`: In `cse()`, add a `CseKey` variant for
  `PtrAdd { ptr, offset, element_size }`. Ensure repeated array index
  computations within the same basic block reuse computed addresses.
- Also consider: `&a[i-1]` can be derived from `&a[i] - element_size`
  (strength reduction opportunity).
- **Test:** Add test with `a[i] = a[i-1] + a[i]` pattern. Verify `&a[i]`
  address is computed only once.
- **Doc:** Update CSE documentation.

---

## Step 11: Cross-Block Store-Reload Forwarding ✅ COMPLETE

**Difficulty:** Medium  
**Est. savings:** ~40 cycles per eliminated reload  
**Source:** Report #8

**Current state:** `SHLD _g_count` / ... / `LHLD _g_count` across loop
back-edges is not forwarded. Peephole Rule 1 (`SHLD addr` / `LHLD addr` →
`SHLD addr`) only handles adjacent instructions.

**Changes:**
- Option A (IR level): Extend `load_store_forwarding()` to track stores across
  back-edges for `_l_*` labels using a "last store" map that persists within
  a loop.
- Option B (peephole level): Track HL contents after `SHLD addr`. When
  encountering `LHLD addr` and HL hasn't been clobbered since, eliminate the
  reload.
- **Test:** Add test with loop that stores and reloads a variable across
  iterations. Verify redundant `LHLD` is eliminated.
- **Doc:** Update load-store forwarding docs.

---

## Step 12: Expanded Peephole Control-Flow Rules ✅ COMPLETE

**Difficulty:** Medium  
**Source:** Plan Phase 1.4

**Changes — add 4–6 new peephole rules:**
1. Conditional inversion over jump (complement of existing Rule 19)
2. Jump-to-ret folding for conditional jumps (extend Rule 34)
3. CALL+RET → JMP in additional forms (extend Rule 2)
4. Collapse double conditional jumps to same target
5. Remove conditional jumps that always/never fire based on preceding
   instruction (e.g., `JNZ` after `XRA A` is dead)
- `peephole.rs`: Add each rule with its own function, integrate into
  `apply_rules()`.
- **Test:** Targeted tests for each new rule pattern.
- **Doc:** Update peephole rules catalog.

---

## Step 13: Interprocedural Function-Effect Summaries

**Difficulty:** Medium (architectural)  
**Source:** Plan Phase 1.2

**Current state:** `FunctionEffects` struct exists in `callgraph.rs` with
`leaf`, `pure`, `readonly`, `noreturn`, and `clobbers` fields. These are
inferred but not fully utilised.

**Changes:**
- `callgraph.rs`: Add hardcoded clobber sets for all runtime helpers
  (`__mul16`, `__div16s`, `__div16u`, `__mod16s`, `__mod16u`, `__shl16`,
  `__shr16u`, `__shr16s`, `__mul32`, etc.).
- Infer `pure`/`readonly` for user functions by scanning IR bodies for
  `StoreGlobal`/`StorePtr` operations.
- `codegen.rs`: Consume clobber information during register spilling.
- **Test:** Verify calling a `pure` leaf function does not trigger unnecessary
  spills.
- **Doc:** Document the effect summary system.

---

## Step 14: Call-Site Selective Save/Restore

**Difficulty:** Medium-Hard  
**Source:** Plan Phase 1.3

**Current state:** `spill_live_before_call` saves all live registers before
every call. Many spills are unnecessary — the callee may not clobber those
registers, or the value may be dead after the call.

**Changes:**
- `codegen.rs`: In `spill_live_before_call`, compute set of vregs live after
  the call point. Intersect with callee's clobber set. Only spill the
  intersection.
- `regalloc.rs`: May need to expose liveness information.
- **Test:** Add test calling a leaf function that only clobbers A/HL. Verify
  BC and DE are not spilled.
- **Doc:** Document selective spilling strategy.

---

## Step 15: Benchmark Gate for Sieve, Dhrystone, Fannkuch

**Difficulty:** Medium (infrastructure)  
**Source:** Plan Phase 1.5

**Changes:**
- Add a script/test that compiles `tests/sieve.c`, `tests/dhrystone.c`,
  `tests/fannkuch.c` and records:
  - Total text size (instruction count / byte count)
  - Number of `CALL __mul16` / `CALL __div16*` / runtime helper calls
  - Estimated cycles in hot loops (if feasible)
- Store baseline values; compare against regressions.
- **Test:** The benchmark gate itself is the test.
- **Doc:** Add benchmark baseline document.

---

## Step 16: Call-Tree Static-Slot Reuse

**Difficulty:** Hard  
**Est. savings:** RAM usage  
**Source:** Plan Phase 1.1

**Current state:** Each function gets non-overlapping static memory for locals.
Functions that are never simultaneously on the call stack could share memory.

**Changes:**
- `callgraph.rs`: After building the call graph, compute interference graph.
  Functions that cannot coexist on the stack (no path from one to another) can
  share addresses.
- Implement coloring/allocation algorithm. Keep fallback for recursive/unknown
  edges.
- **Test:** Multiple non-recursive functions — verify reduced `.STORAGE` count.
- **Doc:** Document slot-reuse algorithm.

---

## Step 17: Loop Induction Variable Register Promotion

**Difficulty:** Hard  
**Est. savings:** ~56 cycles per loop iteration  
**Source:** Report #4

**Current state:** Every loop iteration reloads/re-stores the counter:
`LHLD _l_main_i` / `INX H` / `SHLD _l_main_i` (60 cycles). Could be kept
in a register pair across the loop body (20 cycles).

**Changes:**
- `codegen.rs` or new loop-aware codegen pass: Identify inner loops with
  single induction variable. Assign IV to register pair at loop entry, use
  across body, spill only at exit.
- `regalloc.rs`: Support pinning a vreg to a register for loop duration.
- **Test:** Simple counting loop — verify no `LHLD _l_*_i` / `SHLD _l_*_i`
  inside loop body.
- **Doc:** Document register promotion strategy.

---

## Step 18: Sparse Value-Range and Demanded-Bits Analysis

**Difficulty:** Hard (architectural)  
**Source:** Plan Phase 2.1

**Changes:**
- New analysis pass (e.g. `value_range.rs`): For each vreg, track known range
  [min, max] and known-zero/known-one bit masks.
- Propagate through arithmetic, loads of known-range globals, loop bounds.
- Feed results to narrowing, compare simplification, 8-bit codegen paths.
- **Test:** Operations on values known 0–255 generate 8-bit code.
- **Doc:** Document analysis framework.

---

## Step 19: 8-Bit Value Narrowing in IR and Codegen

**Difficulty:** Hard  
**Source:** Plan Phase 2.2

**Changes:**
- `ir_opt.rs`: Extend `narrow_byte_ops()` to use value-range information for
  more aggressive narrowing.
- `codegen.rs`: Add 8-bit paths for comparison, arithmetic, and store
  operations when operands are known-narrow.
- **Test:** `char` loop counters and small-range integers → verify 8-bit codegen.
- **Doc:** Document narrowing strategy.

---

## Step 20: Call Argument Lowering Strategy

**Difficulty:** Hard  
**Source:** Plan Phase 2.3

**Changes:**
- `codegen.rs`: In `gen_call()`, evaluate which argument is already in HL or
  DE. Assign register args to minimise moves.
- Add cost estimation for different evaluation orders and pick cheapest.
- **Test:** Function calls where reordering avoids spills — count `PUSH`/`POP`.
- **Doc:** Document argument lowering strategy.

---

## Step 21: Load/Store Forwarding with Memory Versioning

**Difficulty:** Hard  
**Source:** Plan Phase 2.4

**Changes:**
- `codegen.rs` or new pass: Maintain "memory version" map tracking which
  `SHLD`/`STA` values are still valid. Reuse register value for `LHLD`/`LDA`
  when version matches.
- Invalidate on calls (unless callee is proven pure) and unknown-location
  stores.
- **Test:** Repeated loads of same variable → verify reduced `LHLD` count.
- **Doc:** Document memory versioning.

---

## Step 22: Rematerialization in Register Allocation

**Difficulty:** Hard  
**Source:** Plan Phase 2.5

**Current state:** `regalloc.rs` already supports `RematImm` and `RematLabel`
variants. Usage may not be aggressive enough.

**Changes:**
- `regalloc.rs`: When a spilled vreg needs reloading, check if
  rematerialisation is cheaper (`LXI H,const` = 12 cycles vs
  `LHLD __spill_N` = 20 cycles).
- Track remat candidates for simple arithmetic results (e.g. `addr + offset`).
- **Test:** Verify reduced `__spill_*` memory usage.
- **Doc:** Document rematerialisation strategy.

---

## Step 23: CFG Block Layout and Jump-Chain Compaction

**Difficulty:** Hard  
**Source:** Plan Phase 2.6

**Changes:**
- New pass or extend `peephole.rs`: Build block-level CFG from assembly.
  Reorder blocks to maximise fallthroughs. Merge single-pred/single-succ
  pairs. Collapse A→B→C jump chains.
- **Test:** Complex control flow — verify reduced `JMP` count.
- **Doc:** Document block layout algorithm.

---

## Step 24: Inter-Block Register Allocation

**Difficulty:** Very Hard  
**Source:** Plan Phase 3.1

**Changes:**
- `regalloc.rs` / `codegen.rs`: Implement predecessor merge heuristics to keep
  values in the same register across basic block boundaries.
- **Test:** Values not redundantly reloaded at block entries.
- **Doc:** Document inter-block allocation.

---

## Step 25: Function Specialization for Constant Arguments

**Difficulty:** Very Hard  
**Source:** Plan Phase 3.2

**Changes:**
- `ir_opt.rs`: Extend `function_specialization()` heuristics — body size
  threshold, call frequency, constant argument profitability.
- Ensure deduplication (Step 3) prevents code bloat.
- **Test:** Function called with two different constant args → two simplified
  specialized versions.
- **Doc:** Document specialisation heuristics.

---

## Step 26: Runtime Helper Specialization

**Difficulty:** Very Hard  
**Source:** Plan Phase 3.3

**Changes:**
- `codegen.rs`: Before `CALL __div16s`, check if divisor is constant. For
  powers of 2, use SHR. For small constants, inline.
- Similarly for `__shl16`/`__shr16` with constant shift amounts.
- **Test:** Constant division/shift avoids runtime calls.
- **Doc:** Document runtime specialisation.

---

## Step 27: Profile-Guided Pass Ordering

**Difficulty:** Very Hard (research)  
**Source:** Plan Phase 3.4

**Changes:**
- `ir_opt.rs`: Make pass order configurable. Create benchmark-specific profiles.
- Add infrastructure to measure and compare optimisation effectiveness.
- **Test:** Benchmark profile ≥ default profile quality.
- **Doc:** Document profiling methodology.

---

## Summary Table

| Step | Optimization | Difficulty | Est. Impact (Vector cycles) | Status |
|------|-------------|------------|----------------------------|--------|
| 1 | PtrAdd fast-path (×2/4/8) | Easy | ~200–1800/access | ✅ DONE |
| 2 | MVI M,n for constant stores | Easy | ~4–8/store | ✅ DONE |
| 3 | Dedup identical specializations | Easy | Code size | ✅ DONE |
| 4 | Compare-with-zero fast path | Easy | ~12/test | ✅ DONE |
| 5 | Peephole W16 compare-branch collapse | Medium | Safety net | ✅ DONE |
| 6 | Register shuffle elimination | Medium | ~28/compare | ✅ DONE |
| 7 | W16 compare-branch fusion | Medium | ~70–100/compare | |
| 8 | Strength reduction 2*i+k | Medium | ~40/occurrence | |
| 9 | LICM for _l_ variables | Medium | ~20/loop iter | |
| 10 | CSE for repeated PtrAdd | Medium | ~200–1800/occurrence | |
| 11 | Cross-block store-reload forwarding | Medium | ~40/reload | |
| 12 | Expanded peephole rules | Medium | Variable | |
| 13 | Function-effect summaries | Medium | Architectural | |
| 14 | Selective save/restore | Medium-Hard | Variable | |
| 15 | Benchmark gate | Medium | Infrastructure | |
| 16 | Call-tree static-slot reuse | Hard | RAM savings | |
| 17 | Loop IV register promotion | Hard | ~56/loop iter | |
| 18 | Value-range analysis | Hard | Architectural | |
| 19 | 8-bit narrowing | Hard | Variable | |
| 20 | Call argument lowering | Hard | Variable | |
| 21 | Load/store forwarding + versioning | Hard | ~40/reload | |
| 22 | Rematerialization | Hard | Variable | |
| 23 | CFG block layout | Hard | Variable | |
| 24 | Inter-block register allocation | Very Hard | Variable | |
| 25 | Function specialization | Very Hard | Variable | |
| 26 | Runtime helper specialization | Very Hard | Variable | |
| 27 | Profile-guided pass ordering | Very Hard | Research | |

## Recommended First Slice (highest ROI)

Steps **1, 4, 7, and 6** together would dramatically improve every loop-heavy
program:

- **Step 1** (PtrAdd): Near-trivial change, massive payoff — `CALL __mul16`
  for multiply-by-2 costs ~350 cycles vs `DAD H` at 12 cycles.
- **Step 4** (zero compare): Easy IR optimization with immediate codegen
  benefit.
- **Step 7** (W16 fusion): The single highest-impact optimization — every loop
  guard benefits.
- **Step 6** (shuffle elimination): Especially impactful on Vector where
  `MOV R,R` costs 8 cycles (not 5), making the 4-MOV shuffle 32 cycles vs 4
  for `XCHG`.
