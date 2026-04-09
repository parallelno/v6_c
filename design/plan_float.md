# Plan: Floating-Point Verification & Hardening

**Date:** 2026-04-09
**Design reference:** [design_float.md](design_float.md)

## Current State

The float subsystem is substantially implemented:

| Layer | Status |
|-------|--------|
| Lexer / Parser / AST | Done — `float` keyword, `FloatLiteral` token, `ExprKind::FloatLiteral(f64)` |
| Type system (`CType::Float`) | Done — 4 bytes, `is_float()`, `common_type()` promotion |
| IR generation (`gen_float_binop`) | Done — maps all 10 binary ops + 2 casts to runtime calls |
| Code generation (`gen_float_call`) | Done — `__op1`/`__op2` marshalling, int↔float casts |
| Runtime library (`runtime/float.asm`) | Done — ~1,200 bytes, 12 public + 6 internal routines |
| Runtime linker (`runtime.rs`) | Done — selective inclusion when float symbols referenced |
| Formal test suite | **Missing** — smoke tests exist in `temp/` but are not in the harness |

**Goal of this plan:** promote float support from "works on smoke tests" to
"verified correct and regression-protected".

---

## Phase 1 — Formal Test Suite

Move float testing into the project's standard test infrastructure so
`cargo test` and the PowerShell harness catch regressions automatically.

### 1.1 Core Arithmetic Tests

Create `tests/unit/float/float_arith.c`:

```c
float a, b, c;
int r;

int main(void) {
    /* --- addition --- */
    a = 3.0f; b = 2.0f; c = a + b;
    r = (int)c; if (r != 5) return 1;

    /* positive + negative (same magnitude) = 0 */
    a = 7.0f; b = -7.0f; c = a + b;
    r = (int)c; if (r != 0) return 2;

    /* positive + negative (different magnitude) */
    a = 10.0f; b = -3.0f; c = a + b;
    r = (int)c; if (r != 7) return 3;

    /* --- subtraction --- */
    a = 10.0f; b = 4.0f; c = a - b;
    r = (int)c; if (r != 6) return 4;

    a = 3.0f; b = 8.0f; c = a - b;
    r = (int)c; if (r != -5) return 5;

    /* --- multiplication --- */
    a = 6.0f; b = 7.0f; c = a * b;
    r = (int)c; if (r != 42) return 6;

    a = -4.0f; b = 5.0f; c = a * b;
    r = (int)c; if (r != -20) return 7;

    a = -3.0f; b = -3.0f; c = a * b;
    r = (int)c; if (r != 9) return 8;

    /* --- division --- */
    a = 20.0f; b = 4.0f; c = a / b;
    r = (int)c; if (r != 5) return 9;

    a = -15.0f; b = 3.0f; c = a / b;
    r = (int)c; if (r != -5) return 10;

    return 0;
}
```

### 1.2 Comparison Tests

Create `tests/unit/float/float_cmp.c`:

```c
float a, b;

int main(void) {
    /* equal */
    a = 5.0f; b = 5.0f;
    if (!(a == b)) return 1;
    if (a != b)    return 2;

    /* not equal */
    a = 5.0f; b = 3.0f;
    if (a == b)    return 3;
    if (!(a != b)) return 4;

    /* less than */
    a = 2.0f; b = 8.0f;
    if (!(a < b))  return 5;
    if (a > b)     return 6;
    if (!(a <= b)) return 7;
    if (a >= b)    return 8;

    /* greater than */
    a = 9.0f; b = 1.0f;
    if (!(a > b))  return 9;
    if (a < b)     return 10;
    if (!(a >= b)) return 11;
    if (a <= b)    return 12;

    /* negative comparisons */
    a = -3.0f; b = 2.0f;
    if (!(a < b))  return 13;
    if (a >= b)    return 14;

    a = -1.0f; b = -5.0f;
    if (!(a > b))  return 15;
    if (a <= b)    return 16;

    return 0;
}
```

### 1.3 Conversion Tests

Create `tests/unit/float/float_conv.c`:

```c
float f;
int i;

int main(void) {
    /* int → float → int round-trip */
    i = 42;
    f = (float)i;
    i = (int)f;
    if (i != 42) return 1;

    /* zero */
    i = 0;
    f = (float)i;
    i = (int)f;
    if (i != 0) return 2;

    /* negative */
    i = -100;
    f = (float)i;
    i = (int)f;
    if (i != -100) return 3;

    /* 1 */
    i = 1;
    f = (float)i;
    i = (int)f;
    if (i != 1) return 4;

    /* -1 */
    i = -1;
    f = (float)i;
    i = (int)f;
    if (i != -1) return 5;

    /* truncation: 0.5 → 0 */
    f = 0.5f;
    i = (int)f;
    if (i != 0) return 6;

    /* truncation: 7.9 → 7 */
    f = 7.9f;
    i = (int)f;
    if (i != 7) return 7;

    /* truncation: -2.8 → -2 */
    f = -2.8f;
    i = (int)f;
    if (i != -2) return 8;

    /* implicit int→float promotion */
    f = 10.0f;
    i = 3;
    f = f + (float)i;
    i = (int)f;
    if (i != 13) return 9;

    return 0;
}
```

### 1.4 Zero & Edge-Case Tests

Create `tests/unit/float/float_edge.c`:

```c
float a, b, c;
int r;

int main(void) {
    /* 0 + x = x */
    a = 0.0f; b = 5.0f; c = a + b;
    r = (int)c; if (r != 5) return 1;

    /* x + 0 = x */
    a = 5.0f; b = 0.0f; c = a + b;
    r = (int)c; if (r != 5) return 2;

    /* 0 * x = 0 */
    a = 0.0f; b = 99.0f; c = a * b;
    r = (int)c; if (r != 0) return 3;

    /* x * 0 = 0 */
    a = 99.0f; b = 0.0f; c = a * b;
    r = (int)c; if (r != 0) return 4;

    /* 0 / x = 0 */
    a = 0.0f; b = 5.0f; c = a / b;
    r = (int)c; if (r != 0) return 5;

    /* x - x = 0 */
    a = 123.0f; b = 123.0f; c = a - b;
    r = (int)c; if (r != 0) return 6;

    /* +0 == -0 */
    a = 0.0f; b = -0.0f;
    if (!(a == b)) return 7;

    /* multiply by 1 */
    a = 42.0f; b = 1.0f; c = a * b;
    r = (int)c; if (r != 42) return 8;

    /* large exponent difference — small operand contributes nothing */
    a = 10000.0f; b = 0.001f; c = a + b;
    r = (int)c; if (r != 10000) return 9;

    return 0;
}
```

### 1.5 Chained Expression Tests

Create `tests/unit/float/float_chain.c`:

```c
float a, b, c, d;
int r;

int main(void) {
    /* chained arithmetic: (a + b) * c */
    a = 2.0f; b = 3.0f; c = 4.0f;
    d = (a + b) * c;
    r = (int)d;
    if (r != 20) return 1;

    /* division + subtraction */
    a = 100.0f; b = 5.0f; c = 3.0f;
    d = a / b - c;
    r = (int)d;
    if (r != 17) return 2;

    /* mixed int/float: int compared against cast */
    a = 6.0f; b = 7.0f;
    r = (int)(a * b);
    if (r != 42) return 3;

    /* multiple casts */
    r = 25;
    a = (float)r;
    b = 5.0f;
    c = a / b;
    r = (int)c;
    if (r != 5) return 4;

    return 0;
}
```

---

## Phase 2 — Golden Test Pipeline & Harness Integration

Every float test follows the full **golden pipeline** end-to-end:

```
 source.c ──(v6c)──► source.asm ──(v6asm)──► source.rom ──(v6emul)──► result
    │                    │                      │                      │
  compile            assemble               execute                verify
  v6c.exe            v6asm.exe              v6emul.exe            HL == 0
```

A test **passes** when `main()` returns 0 (HL = 0x0000 in the CPU dump).
A test **fails** when `main()` returns N, indicating check N failed.

### v6emul CLI Reference (Test Mode)

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--rom <path>` | string | *(none)* | Path to a ROM file to load into emulator memory |
| `--load-addr <addr>` | int | `0` | Memory address to load the ROM at. Supports hex (`0x100`) and decimal (`256`) |
| `--halt-exit` | flag | — | Exit on first HLT instruction |
| `--speed <speed>` | string | `normal` | Execution speed: `max` for testing |
| `--dump-cpu` | flag | — | Print full CPU state on exit (registers, flags, PC, SP, cycles) |
| `--dump-memory` | flag | — | Print full 64K memory dump (hex) on exit |
| `--run-cycles <n>` | int | — | Safety timeout; exit after N cycles (guards against infinite loops) |

**Example:** `v6emul --rom test.rom --halt-exit --dump-cpu --load-addr 0x100 --speed max --run-cycles 5000000`

The default load/execution address is `0x100` (matching `crt0.asm`'s `.ORG`).
Use `0x00` for standalone ASM-level unit tests.

### 2.1 Register Test Directory

Add `tests/unit/float/` to the Rust execution test discovery in
`tests/execution.rs`.  The existing harness recursively discovers `.c`
files under `tests/unit/`, so creating the directory and files is
sufficient — no Rust code changes needed unless the discovery is
explicitly scoped.

**Verification command:**
```powershell
cargo test --test execution -- float
```

### 2.2 PowerShell Runner Support

Confirm `scripts/run_optimization_unit_checks.ps1` can run float tests
with the `-RunExecution` flag.  If the script only targets
`tests/unit/optimization/` and `tests/unit/optimization_small/`, add a
`-TestDir` parameter or extend the discovery path to include
`tests/unit/float/`.

**Verification command:**
```powershell
.\scripts\run_optimization_unit_checks.ps1 -RunExecution -Filter "float_*"
```

### 2.3 Manual Golden Test (before harness)

Each test file can be validated manually through the full golden pipeline:

```powershell
# Step 1: Compile  C → ASM
.\target\debug\v6c.exe tests\unit\float\float_arith.c -o temp\float_arith.asm -l temp\float_arith.lst

# Step 2: Assemble  ASM → ROM
.\tools\v6asm\v6asm.exe temp\float_arith.asm -o temp\float_arith.rom

# Step 3: Execute  ROM → verify HL == 0
.\tools\v6emul\v6emul.exe --rom temp\float_arith.rom --load-addr 0x100 --halt-exit --dump-cpu --speed max --run-cycles 5000000
# Pass: H=00 L=00    Fail: H=00 L=NN (check NN failed)
```

The `--run-cycles` limit is set higher than usual (5M vs 1M) because
float operations are cycle-expensive (~1,500–4,000 cycles per operation).

To inspect memory state on failure, add `--dump-memory` to see the full
64K hex dump (useful for checking `__op1`/`__op2` contents).

---

## Phase 3 — Runtime Verification

Systematically run each test and investigate any failures.

### 3.1 Execution Order

| # | Test File | Focus | Expected Difficulty |
|---|-----------|-------|---------------------|
| 1 | `float_arith.c` | add, sub, mul, div — positive, negative, mixed | Low — smoke tests already cover basics |
| 2 | `float_conv.c` | int↔float casts, truncation, round-trip | Low-Medium — truncation direction |
| 3 | `float_cmp.c` | all 6 relational operators, sign edges | Medium — comparison derivations from `__flt` |
| 4 | `float_edge.c` | zero identity, ±0 equality, exponent cutoff | Medium — exercises internal early-exit paths |
| 5 | `float_chain.c` | multi-step expressions, mixed types | Medium — exercises operand marshalling sequences |
| 6 | `float_neg.c` | unary negation, double-negate, negate+arith | Low — sign-bit flip in codegen and constant folder |
| 7 | `float_loop.c` | float accumulation in while-loops (sum, product, divide) | Medium — stress-tests spill/reload across iterations |
| 8 | `float_cond.c` | if/else driven by float comparisons, nested branches | Medium — comparison result feeding control flow |
| 9 | `float_pow2.c` | powers of two: exact squares, division, add/sub | Low — exact representation, no rounding |
| 10 | `float_range.c` | large values (10 000+), negative large, chained large ops | Medium — wide exponent range, multi-step accuracy |

### 3.2 Bug Triage Process

For each failing check (`return N`):

1. **Reproduce** — compile + assemble + execute to confirm failure return code.
2. **Inspect listing** — open `.lst` file, find the failing comparison,
   trace the operand loading sequence backward.
3. **Reference test** — write a minimal standalone `.asm` test that calls
   the specific runtime function directly, bypassing the compiler. Assemble
   and execute to isolate whether the bug is in codegen or in `float.asm`.
4. **Fix** — apply the fix to the identified component (codegen or runtime).
5. **Re-run** — confirm all checks in the file pass, plus re-run all
   previously passing test files to check for regressions.

### 3.3 Known Risk Areas

| Area | Risk | Mitigation |
|------|------|------------|
| `__fadd` subtraction path | Magnitude comparison is 4-byte big-endian; wrong byte order → wrong result sign | `float_arith.c` checks 3 and 5 cover both mixed-sign directions |
| `__fmul` / `__fdiv` exponent arithmetic | Off-by-one in bias adjustment (±127) | `float_arith.c` checks 6–10 with known products/quotients |
| `__ftoi` truncation direction | Design says truncation toward zero; verify negative truncation direction | `float_conv.c` check 8 (`-2.8f → -2`) |
| `__fle` / `__fgt` operand swap | 16-byte memory copy to swap `__op1`↔`__op2`; must not corrupt `__fa_tmp` | `float_cmp.c` checks 7–12 exercise all derived comparisons |
| `__op1`/`__op2` ownership | `div32.asm` defines `__op1`/`__op2`; if float module is linked alone, symbols may be missing | `float_edge.c` exercises float-only programs |

---

## Phase 4 — ASM-Level Optimization Audit

After correctness is verified, review `runtime/float.asm` for performance
improvements.  These are **not blockers** for the test phase.

### 4.1 Audit Checklist

| # | Area | Question | Potential Saving |
|---|------|----------|-----------------|
| O1 | `__funpack` register use | Can exponent extraction avoid the B-register detour? The `ADD A; MOV B,A; RLC; ACI 0` sequence could potentially be simplified. | ~8–12 cycles per unpack |
| O2 | Exponent alignment loop | The per-iteration 4-byte right shift through memory costs ~64 cycles. For shifts > 8, a byte-move fast path (like `__itof` uses) saves ~56 cycles per 8 positions. | ~50–200 cycles for large exponent diffs |
| O3 | `__fmul` accumulator shift | The 6-byte shift goes through memory every iteration (24 LDA/STA pairs × 24 iterations). Partial unrolling or keeping the top bytes in registers could help. | ~200–500 cycles total |
| O4 | `__fdiv` comparison | The 3-byte comparison in the division loop loads from memory each iteration. Keeping the divisor in registers (if possible given 8080 constraints) would save ~24 cycles/iteration. | ~300–500 cycles total |
| O5 | `__fle`/`__fgt` operand swap | 16 LDA/STA instructions to swap 8 bytes. A stack-based swap (PUSH/POP via LHLD/SHLD) could be shorter. | ~30–50 cycles per call |
| O6 | `__fne` | Currently calls `__feq` then XOR. Inlining the comparison with inverted branch saves the CALL/RET overhead. | ~24 cycles |

### 4.2 Optimization Constraints

- `__op1`/`__op2` must remain at the same addresses (shared with `div32`/`mul32`/`shift32`).
- Scratch area layout (`__fa_s1`..`__fa_tmp`) is internal and may be reorganized.
- All optimizations must pass the full Phase 1 test suite before being committed.

---

## Phase 5 — Future Tests (Deferred)

These tests are not required for the current milestone but should be
added when the corresponding features mature.

| Test | Depends On | Description |
|------|-----------|-------------|
| `float_long_conv.c` | `long` type support | `(long)float_expr`, `(float)long_expr` — tests `__ltof`/`__ftol` |
| `float_const_fold.c` | Compile-time float eval | Verify `float x = 2.0f * 3.0f;` folds to `6.0f` at compile time |
| `float_array.c` | Float array support | `float arr[4]; arr[i] = ...;` — tests 4-byte indexed access codegen |
| `float_struct.c` | Float in structs | `struct { float x, y; }` — tests field offset computation |
| `float_printf.c` | `printf("%f", ...)` | Float-to-decimal conversion in stdio runtime |

---

## Execution Checklist

| Step | Action | Done |
|------|--------|------|
| 1 | Create `tests/unit/float/` directory | ☑ |
| 2 | Write `float_arith.c` | ☑ |
| 3 | Write `float_cmp.c` | ☑ |
| 4 | Write `float_conv.c` | ☑ |
| 5 | Write `float_edge.c` | ☑ |
| 6 | Write `float_chain.c` | ☑ |
| 7 | Write `float_neg.c` | ☑ |
| 8 | Write `float_loop.c` | ☑ |
| 9 | Write `float_cond.c` | ☑ |
| 10 | Write `float_pow2.c` | ☑ |
| 11 | Write `float_range.c` | ☑ |
| 12 | Manual compile + execute all 10 files | ☑ |
| 13 | Fix failures: `__fmul` exponent bias (SUI 126), carry propagation, `__fdiv` exponent bias (ADI 126) + overflow pre-check | ☑ |
| 14 | Verify `cargo test --test execution` picks up all 10 float tests | ☑ |
| 15 | Re-run full test suite — no regressions (552 pass) | ☑ |
| 16 | (Optional) Audit `float.asm` per Phase 4 checklist | ☐ |
