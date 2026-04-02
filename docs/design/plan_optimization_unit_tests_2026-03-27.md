# Plan: Optimization Unit Tests as C Files Only

Date: 2026-03-27

## Policy (Updated)

1. Do not add new optimization unit tests inside Rust source modules.
2. Store optimization unit tests as C files only under `tests/unit/optimization/`.
3. Keep each C file focused on one optimization family.
4. Keep tests deterministic and self-checking through final global result variables.

## Folder Layout

All new optimization tests will live here:

- `tests/unit/optimization/`

Proposed file set:

1. `tests/unit/optimization/opt_ir_const_fold.c`
2. `tests/unit/optimization/opt_ir_lsf.c`
3. `tests/unit/optimization/opt_ir_narrow.c`
4. `tests/unit/optimization/opt_ir_strength.c`
5. `tests/unit/optimization/opt_ir_dce.c`
6. `tests/unit/optimization/opt_ir_cse.c`
7. `tests/unit/optimization/opt_ir_jump_thread.c`
8. `tests/unit/optimization/opt_ir_loop.c`
9. `tests/unit/optimization/opt_ir_inline_specialize.c`
10. `tests/unit/optimization/opt_codegen_compact_cfg.c`
11. `tests/unit/optimization/opt_codegen_fastpaths.c`
12. `tests/unit/optimization/opt_regalloc_pressure.c`
13. `tests/unit/optimization/opt_peephole_ctrl.c`
14. `tests/unit/optimization/opt_peephole_data.c`
15. `tests/unit/optimization/opt_pipeline_mixed.c`

## Test Style for C Files

1. Each test file has one `main(void)`.
2. Use global output variables only, for easy post-build inspection in generated ASM/LST.
3. Include three categories per file:
   - positive case: pattern expected to optimize
   - negative case: pattern must not optimize
   - safety case: control/alias/signedness edge
4. Use stable constants to avoid UB and undefined optimizer behavior.
5. Do not rely on runtime I/O.

Example structure:

```c
int status;
int out0;
int out1;

void main(void)
{
    /* positive */
    out0 = 8 * 4;

    /* negative */
    out1 = 7 * 3;

    /* status aggregation for harness checks */
    status = out0 + out1;
}
```

## Coverage Matrix by C File

### IR constants and propagation

- File: `opt_ir_const_fold.c`
- Covers:
1. arithmetic folding (add/sub/mul/div/mod)
2. bitwise folding
3. shift folding (logical/arithmetic)
4. compare folding
5. cast folding and width wrap behavior
6. non-fold guard for divide-by-zero path

### Load/store forwarding

- File: `opt_ir_lsf.c`
- Covers:
1. store-then-load same global slot
2. non-forward for different slot
3. invalidation around pointer/local stores and calls
4. control-flow boundary reset behavior

### Byte narrowing

- File: `opt_ir_narrow.c`
- Covers:
1. byte-proven arithmetic/bitwise narrowing
2. unsigned div/mod narrowing with nonzero rhs
3. compare narrowing
4. no narrowing when signed/unknown

### Strength reduction

- File: `opt_ir_strength.c`
- Covers:
1. mul by power of two
2. unsigned div by power of two
3. unsigned mod by power of two
4. no rewrite for non-power/signed cases

### Dead-code elimination

- File: `opt_ir_dce.c`
- Covers:
1. dead path after unconditional transfer
2. reachable code retention
3. side-effect preservation

### CSE

- File: `opt_ir_cse.c`
- Covers:
1. repeated expression reuse in a block
2. non-reuse across clobbering boundaries

### IR jump threading

- File: `opt_ir_jump_thread.c`
- Covers:
1. simple and transitive jump chain cleanup
2. non-threadable edge preservation

### Loop passes

- File: `opt_ir_loop.c`
- Covers:
1. loop-invariant hoist opportunities
2. induction-style updates
3. unroll-eligible loop shapes

### Inlining and specialization

- File: `opt_ir_inline_specialize.c`
- Covers:
1. tiny function call sites likely to inline
2. constant-argument call sites for specialization opportunities
3. non-eligible call shapes retained

### Codegen CFG compaction

- File: `opt_codegen_compact_cfg.c`
- Covers:
1. jump-chain heavy control flow
2. jump-to-next-label opportunities

### Codegen helper fast paths

- File: `opt_codegen_fastpaths.c`
- Covers:
1. mul constants 0/1/2/3/4/8
2. div by 1
3. mod by 1 and 2
4. constant shift variants

### Regalloc under pressure

- File: `opt_regalloc_pressure.c`
- Covers:
1. multi-live 16-bit values forcing displacement/spills
2. frequent constant rematerialization opportunities

### Peephole control-flow rules

- File: `opt_peephole_ctrl.c`
- Covers:
1. branch inversion patterns
2. jump-to-next-label patterns
3. jump-to-ret style patterns
4. jump-chain cleanup scenarios

### Peephole data-movement rules

- File: `opt_peephole_data.c`
- Covers:
1. redundant load/store pairs
2. self moves
3. nop removal
4. xchg/cma and inx/dcx cancellation
5. compiler-label cleanup safety

### Mixed pipeline interaction

- File: `opt_pipeline_mixed.c`
- Covers:
1. expression + control-flow + call mix
2. verifies combined pass interaction remains stable

## Execution Plan

1. Create all files above under `tests/unit/optimization/`.
2. Implement positive/negative/safety cases per file.
3. Add compile/run script in `tests/compare/` to build each file and capture ASM/LST.
4. Add expected-pattern checks for key optimizations in generated ASM.
5. Promote stable checks into CI once baselines are accepted.

## Definition of Done

1. Every optimization family has at least one dedicated C file in `tests/unit/optimization/`.
2. No new optimization unit tests were added under Rust `#[cfg(test)]` modules.
3. All C optimization test files compile successfully with v6c.
4. Generated ASM confirms expected optimization patterns for each file.

## Implemented Artifacts

1. Test directory: `tests/unit/optimization/`
2. Runner script: `tests/compare/run_optimization_unit_checks.ps1`
3. Directory guide: `tests/unit/optimization/README.md`
