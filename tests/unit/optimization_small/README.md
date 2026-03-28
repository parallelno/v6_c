# Optimization Small Tests (C-only)

This directory contains minimal per-feature optimization tests for `v6c`/`v6asm`.
Each file should test a single optimization behavior and be easy to review by hand.

## Naming conventions

- Files are named `opt_small_<feature>.c`.
- Each file contains:
  - A detailed comment describing the feature, benefit, and target patterns.
  - A small C code snippet containing the pre-optimization pattern.

## Features covered in first 10 tests

1. Constant folding
2. Dead store elimination
3. Common subexpression elimination (CSE)
4. Loop unrolling
5. Strength reduction
6. Inline specialization
7. Jump threading
8. Integer narrowing
9. Redundant move elimination
10. CFG compaction / branch simplification

## Running tests

From repository root:

```powershell
.\	ests\run_optimization_unit_checks.ps1 -UseSmall
```

Filter one test:

```powershell
.\	ests\run_optimization_unit_checks.ps1 -UseSmall -Filter opt_small_const_fold.c
```

### Optional runner flags

- `-NoAutoBuildV6asm`: skip automatic `v6asm` clone/build
- `-RequireV6asm`: fail if `v6asm` missing
- `-AllowAsmFailure`: allow 8080 assembly failure and continue

## Outputs

Generated outputs are stored under:

- `out/tests/unit/optimization_small`:
  - `<test>.asm` (v6c assembly)
  - `<test>.v6c.lst` (v6c generated listing)
  - `<test>.rom` (v6asm ROM binary output)
  - `<test>.lst` (v6asm listing output)

## Memory map expectations (Vector 06C)

- 0x0000-0x00FF: system/interrupt vectors
- 0x0100-0x7FFF: program code + data + heap
- 0x8000-0xFFFF: stack and video memory region
- runtime CRT0 sets `SP = 0x8000` at startup

## Inspecting optimized output

1. Confirm v6c produced the expected assembly and label patterns: open `out/tests/unit/optimization_small/<test>.asm`.
2. Confirm v6asm assembled successfully with matching code in `out/tests/unit/optimization_small/<test>.lst`.
3. For regression bugs, compare pre-/post-optimization output with `git diff` or `code`.

## Adding new tests

1. Add `tests/unit/optimization_small/opt_small_newfeature.c`.
2. Include feature comment and tiny code block for the optimization scenario.
3. Run the script with `-Filter opt_small_newfeature.c`.
4. Verify `asm/lst/rom` artifacts are generated and correct.
